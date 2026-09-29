//! `tessaro-ctl printer`: the printers in `tessaro.db` and the CUPS queues
//! they are set up as (`crate::printer`).
//!
//! A printer is set up in CUPS before it is saved, so one that cannot be
//! reached is refused rather than kept broken. After that the `printers`
//! table is the truth: the queues are reconciled with it when the agent starts,
//! once a minute from `watch_printers` (a printer that did not answer at
//! boot is set up once it does), and after every change here. Like the
//! schedules, printers stay through an unclaim and go with a factory reset.

use std::sync::Arc;
use std::time::Duration;

use protocol::keys;
use protocol::{
    Done, PrintJob, PrintQueued, PrinterFound, PrinterInfo, PrinterList, PrinterSpec,
    PRINT_DATA_MAX,
};

use super::{Caller, Control};
use crate::deadline::blocking;
use crate::printer::{self, Printers};
use crate::state;

/// The largest stored file `printer-print` prints by its path. Read whole
/// into memory, then handed to CUPS.
const PRINT_FILE_MAX: u64 = 64 * 1024 * 1024;

impl Control {
    pub(super) async fn printer_list(&self) -> Result<PrinterList, String> {
        let wanted = self.read_printers().await?;
        let enabled = self.printing_enabled().await;
        let (present, problem) = self.printers_present().await;
        let jobs = self
            .printer_jobs_of(&wanted, None)
            .await
            .unwrap_or_default();
        let printers = wanted
            .printers
            .iter()
            .map(|spec| {
                let mut info = printer::info(spec, &wanted, &present, problem.as_deref());
                info.queued = jobs.iter().filter(|job| job.printer == spec.name).count() as u32;
                info
            })
            .collect();
        Ok(PrinterList { enabled, printers })
    }

    pub(super) async fn printer_show(&self, name: String) -> Result<PrinterInfo, String> {
        let wanted = self.read_printers().await?;
        let spec = wanted.find(&name)?.clone();
        let (present, problem) = self.printers_present().await;
        let mut info = printer::info(&spec, &wanted, &present, problem.as_deref());
        let jobs = self
            .printer_jobs_of(&wanted, Some(&spec.name))
            .await
            .unwrap_or_default();
        info.queued = jobs.len() as u32;
        let (markers, model) = self.cups.markers(&spec).await;
        info.markers = markers;
        info.model = model;
        Ok(info)
    }

    /// Look for printers; the server keeps what it finds as a job.
    pub(super) fn printer_discover(
        &self,
        caller: &Caller,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<PrinterFound, String>>, String> {
        let lock = Arc::clone(&self.printer_discovering)
            .try_lock_owned()
            .map_err(|_| "the device is already looking for printers".to_string())?;
        self.log.info(format!(
            "printer discovery requested by {}",
            caller.describe()
        ));
        let (send, steps) = tokio::sync::mpsc::channel(64);
        let cups = Arc::clone(&self.cups);
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        tokio::spawn(async move {
            let _lock = lock;
            let have = match blocking("reading the printers", move || {
                Ok(db.read::<Printers>(&log).printers)
            })
            .await
            {
                Ok(have) => have,
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                    return;
                }
            };
            // naked: discover runs lpinfo under printer::DISCOVER
            match cups.discover(&have).await {
                Ok(found) => {
                    for printer in found {
                        // naked: the receiver is the job's drain, itself bounded
                        if send.send(Ok(printer)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                }
            }
        });
        Ok(steps)
    }

    pub(super) async fn printer_create(
        &self,
        caller: &Caller,
        spec: PrinterSpec,
    ) -> Result<PrinterInfo, String> {
        let _writes = self.writes.lock().await;
        let existing = self.read_printers().await?;
        let spec = printer::validate(spec, &existing.printers)?;
        if self.cups.managed {
            // Set up first: a driverless printer that does not answer is
            // refused now, not kept as a queue that never prints.
            let _reconciling = self.printing.lock().await;
            self.cups.setup(&spec).await?;
        }

        let db = self.db.clone();
        let saved = spec.clone();
        blocking("saving the printer", move || {
            db.update(|all: &mut Printers| {
                let spec = printer::validate(saved, &all.printers)?;
                // The first printer is the default until someone says
                // otherwise: window.print() has somewhere to go.
                if all.default.is_none() {
                    all.default = Some(spec.name.clone());
                }
                all.printers.push(spec);
                Ok(())
            })
        })
        .await?;
        self.log.info(format!(
            "printer {} ({}, {}) created by {}",
            spec.name,
            spec.kind.name(),
            spec.uri,
            caller.describe()
        ));
        self.apply_printers().await?;
        self.printer_show(spec.name).await
    }

    pub(super) async fn printer_remove(
        &self,
        caller: &Caller,
        name: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let target = name.clone();
        let default = blocking("removing the printer", move || {
            db.update(|all: &mut Printers| {
                let at = all
                    .printers
                    .iter()
                    .position(|printer| printer.name == target)
                    .ok_or_else(|| printer::missing(&target))?;
                all.printers.remove(at);
                if all.default.as_deref() == Some(target.as_str()) {
                    all.default = all.printers.first().map(|printer| printer.name.clone());
                }
                Ok(all.default.clone())
            })
        })
        .await?;
        self.log
            .info(format!("printer {name} removed by {}", caller.describe()));
        let applied = self.apply_printers().await;
        let default = match default {
            Some(default) => format!("; {default} is the default printer"),
            None => String::new(),
        };
        let message = match applied {
            Ok(()) => format!("removed printer {name} and its jobs{default}"),
            Err(err) => format!("removed printer {name}{default}; CUPS: {err}"),
        };
        Ok(Done::new(message))
    }

    pub(super) async fn printer_default(
        &self,
        caller: &Caller,
        name: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let target = name.clone();
        blocking("saving the default printer", move || {
            db.update(|all: &mut Printers| {
                all.find(&target)?;
                all.default = Some(target);
                Ok(())
            })
        })
        .await?;
        self.log.info(format!(
            "printer {name} made the default by {}",
            caller.describe()
        ));
        self.apply_printers().await?;
        let note = if self.printing_enabled().await {
            ""
        } else {
            "; the page prints once printer.enable is on"
        };
        Ok(Done::new(format!("window.print() prints on {name}{note}")))
    }

    pub(super) async fn printer_test(
        &self,
        caller: &Caller,
        name: String,
    ) -> Result<PrintQueued, String> {
        let wanted = self.read_printers().await?;
        let spec = wanted.find(&name)?.clone();
        let device = self.identity.name.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or_default();
        let when = blocking("reading the local time", move || {
            Ok(crate::schedules::moment(now).local)
        })
        .await?;
        let page = printer::test_page(&spec, &device, &when);
        self.print_to(caller, &spec, &page, 1, None, "Tessaro test page")
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn printer_print(
        &self,
        caller: &Caller,
        name: Option<String>,
        data: Option<String>,
        path: Option<String>,
        copies: Option<u32>,
        media: Option<String>,
        title: Option<String>,
    ) -> Result<PrintQueued, String> {
        let wanted = self.read_printers().await?;
        let spec = wanted.pick(name.as_deref())?.clone();
        let copies = printer::copies(copies)?;
        let media = media
            .map(|media| media.trim().to_string())
            .filter(|media| !media.is_empty());
        if let Some(media) = &media {
            // The same check a printer's own paper gets.
            let probe = PrinterSpec {
                media: Some(media.clone()),
                ..spec.clone()
            };
            printer::validate(probe, &[])?;
        }
        let document = match (data, path) {
            (Some(_), Some(_)) => return Err("print data or a stored file, not both".to_string()),
            (Some(data), None) => {
                let bytes = openssl::base64::decode_block(&data)
                    .map_err(|_| "the document is not base64".to_string())?;
                if bytes.len() > PRINT_DATA_MAX {
                    return Err(format!(
                        "a document sent with the request is at most {PRINT_DATA_MAX} bytes; \
                         upload a larger one with `tessaro-ctl files upload` and print it by path"
                    ));
                }
                bytes
            }
            (None, Some(path)) => self.files.read_whole(&path, PRINT_FILE_MAX).await?,
            (None, None) => return Err("nothing to print".to_string()),
        };
        if document.is_empty() {
            return Err("the document is empty".to_string());
        }
        let title = printer::title(title.as_deref());
        self.print_to(caller, &spec, &document, copies, media.as_deref(), &title)
            .await
    }

    async fn print_to(
        &self,
        caller: &Caller,
        spec: &PrinterSpec,
        document: &[u8],
        copies: u32,
        media: Option<&str>,
        title: &str,
    ) -> Result<PrintQueued, String> {
        let cups = &self.cups;
        // naked: Cups::print runs lp under run_async()
        let job = cups.print(spec, document, copies, media, title).await?;
        self.log.info(format!(
            "printer {}: job {job} ({} bytes{}) from {}",
            spec.name,
            document.len(),
            if copies > 1 {
                format!(", {copies} copies")
            } else {
                String::new()
            },
            caller.describe()
        ));
        Ok(PrintQueued {
            message: format!("sent to {} as job {job}", spec.name),
            printer: spec.name.clone(),
            job,
        })
    }

    pub(super) async fn printer_jobs(&self, name: Option<String>) -> Result<Vec<PrintJob>, String> {
        let wanted = self.read_printers().await?;
        if let Some(name) = &name {
            wanted.find(name)?;
        }
        self.printer_jobs_of(&wanted, name.as_deref()).await
    }

    async fn printer_jobs_of(
        &self,
        wanted: &Printers,
        name: Option<&str>,
    ) -> Result<Vec<PrintJob>, String> {
        let known: Vec<String> = wanted
            .printers
            .iter()
            .map(|printer| printer.name.clone())
            .collect();
        if known.is_empty() {
            return Ok(Vec::new());
        }
        self.cups.jobs(name, &known).await
    }

    pub(super) async fn printer_cancel(
        &self,
        caller: &Caller,
        job: String,
    ) -> Result<Done, String> {
        let wanted = self.read_printers().await?;
        let jobs = self.printer_jobs_of(&wanted, None).await?;
        if !jobs.iter().any(|queued| queued.job == job) {
            return Err(format!(
                "no job {job:?} waiting; `tessaro-ctl printer jobs` shows them"
            ));
        }
        self.cups.cancel(&job).await?;
        self.log.info(format!(
            "print job {job} cancelled by {}",
            caller.describe()
        ));
        Ok(Done::new(format!("cancelled job {job}")))
    }

    /// printer.enable, set or defaulted.
    pub(super) async fn printing_enabled(&self) -> bool {
        match self.read_state().await {
            Ok(state) => {
                state::setting(&state.settings, &self.defaults, keys::PRINTER_ENABLE).as_deref()
                    == Some("1")
            }
            Err(_) => false,
        }
    }

    /// Keeps the CUPS queues on the stored printers, from the agent's start and
    /// then once a minute.
    pub fn watch_printers(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(60);

        if !self.cups.managed {
            return;
        }
        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut reported: Option<String> = None;
            loop {
                // naked: apply_printers waits only on blocking() and CUPS's clients, each bounded
                let problem = control.apply_printers().await.err();
                if problem != reported {
                    match &problem {
                        Some(err) => control.log.info(format!("printers: {err}")),
                        None if reported.is_some() => control
                            .log
                            .info("printers: every printer is set up".to_string()),
                        None => {}
                    }
                    reported = problem;
                }
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// Reconcile CUPS with the stored printers. One at a time, whoever asks;
    /// what failed is kept for `printer-list` to say.
    async fn apply_printers(&self) -> Result<(), String> {
        if !self.cups.managed {
            return Ok(());
        }
        // naked: held only by another apply_printers or a create, each bounded
        let _reconciling = self.printing.lock().await;
        let wanted = self.read_printers().await?;
        let outcome = self.cups.reconcile(&wanted).await;
        *crate::sync::lock(&self.printer_problem) = outcome.as_ref().err().cloned();
        outcome
    }

    /// What CUPS has, and why a printer may be missing from it. Nothing,
    /// and why, when CUPS does not answer.
    async fn printers_present(&self) -> (printer::Present, Option<String>) {
        if !self.cups.managed {
            return (
                printer::Present::default(),
                Some("this host's printers are not managed (KIOSK_MANAGE_PRINTERS=0)".to_string()),
            );
        }
        match self.cups.present().await {
            Ok(present) => {
                let problem = crate::sync::lock(&self.printer_problem).clone();
                (present, problem)
            }
            Err(err) => (printer::Present::default(), Some(format!("CUPS: {err}"))),
        }
    }

    /// The printers, or why they cannot be read: unlike `Db::read`, a
    /// command must say so rather than answer as if there were none.
    pub(super) async fn read_printers(&self) -> Result<Printers, String> {
        let db = self.db.clone();
        blocking("reading the printers", move || {
            db.transaction(crate::db::load::<Printers>)
        })
        .await
    }

    /// A factory reset: every printer gone, and its CUPS queue with it. The
    /// caller holds `writes`.
    pub(super) async fn clear_printers(&self) -> Result<(), String> {
        let db = self.db.clone();
        blocking("removing the printers", move || db.clear::<Printers>()).await?;
        if let Err(err) = self.apply_printers().await {
            // CUPS's state is on /data, which the reset's reboot empties
            // anyway; until then the watcher tries again.
            self.log
                .info(format!("factory reset: the printers' queues: {err}"));
        }
        Ok(())
    }
}
