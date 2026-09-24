//! A network change as one transaction the device keeps or undoes by itself.
//!
//! The operator's client sends one request and may never hear back - the
//! change can take its own connection away - so nothing here waits for it.
//! The order is what makes that safe:
//!
//! 1. **Snapshot** every profile the change touches, secrets included, to
//!    `/data/tessaro/network/snapshot.json` (0600), and write the record of
//!    what is under way next to it.
//! 2. **Checkpoint** the devices involved in NetworkManager, with a rollback
//!    timer of its own: if this process dies, NetworkManager undoes the
//!    runtime half by itself.
//! 3. **Apply in memory only** - `Update2` with `IN_MEMORY`, a new profile
//!    with `persist: memory` - so a power cut at any point boots the old
//!    configuration from disk.
//! 4. **Verify on the device**: the connection comes up, a default route is
//!    still there if there was one, and the `--verify` target answers.
//! 5. Only then **save** to disk and drop the checkpoint. On any failure,
//!    **roll back**: the checkpoint, then every snapshot (which covers
//!    profiles that were not active, which a checkpoint does not), then
//!    whatever was created.
//!
//! An agent that stops half way leaves the record behind, and the next one
//! rolls it back at startup (`recover`).

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::{ChangeOutcome, NetChange, NetCheck, Verify};
use serde::{Deserialize, Serialize};

use crate::control::blocking;
use crate::log::Log;
use crate::nm::settings::Dict;

/// How long a connection has to come up: association, the handshake, DHCP.
pub const ACTIVATE: Duration = Duration::from_secs(45);
/// After it is up, how long the route and the addresses have to settle.
const SETTLE: Duration = Duration::from_secs(20);
/// Tries at the `--verify` target, a second or two apart.
const REACH_TRIES: u32 = 5;
/// NetworkManager's own timer on the checkpoint, the backstop if the agent
/// dies: longer than anything above, so it never fires on a live change.
pub const BACKSTOP: Duration = Duration::from_secs(150);

const RECORD: &str = "txn.json";
const SNAPSHOT: &str = "snapshot.json";
const LAST: &str = "last.json";

/// A profile as it was before the change: its settings with its secrets, in
/// D-Bus's own encoding, and whether it was on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub uuid: String,
    pub saved: bool,
    pub settings: Vec<u8>,
}

/// What is under way. No secrets: those are in the snapshot file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub action: String,
    pub profile: Option<String>,
    pub checkpoint: Option<String>,
    /// Profiles that existed before and were changed or removed.
    pub touched: Vec<String>,
    /// Profiles this change created.
    pub created: Vec<String>,
    /// The WiFi radio before a `wifi on|off`.
    pub radio_was: Option<bool>,
}

/// One change, already resolved to uuids and settings.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// New settings for a profile, in memory. With a device, it is brought
    /// up there: reapplied if it is active on it, activated if not.
    Update {
        uuid: String,
        settings: Dict,
        device: Option<String>,
    },
    Activate {
        uuid: String,
        device: Option<String>,
    },
    Deactivate {
        uuid: String,
    },
    Delete {
        uuid: String,
    },
    /// A new profile, in memory, activated on `device`.
    Add {
        settings: Dict,
        device: String,
    },
    Radio {
        on: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub action: String,
    pub profile: Option<String>,
    pub uuid: Option<String>,
    /// Interfaces the checkpoint covers.
    pub devices: Vec<String>,
    /// Existing profiles to snapshot, and to save once it holds.
    pub touched: Vec<String>,
    pub step: Step,
    pub verify: Verify,
    pub note: Option<String>,
}

/// What applying the step started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// The active connection to wait for.
    pub active: Option<String>,
    /// The interface it is coming up on.
    pub device: Option<String>,
    /// The uuid of a profile the step created.
    pub created: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub interface: String,
    pub gateway: Option<IpAddr>,
}

/// Everything the transaction asks of NetworkManager and the kernel. The
/// real one is `nm::Live`; the tests drive a fake that can fail anywhere.
#[async_trait::async_trait]
pub trait Ops: Send + Sync {
    async fn checkpoint(&self, devices: &[String], backstop: Duration) -> Result<String, String>;
    async fn rollback(&self, checkpoint: &str) -> Result<(), String>;
    async fn destroy(&self, checkpoint: &str) -> Result<(), String>;
    /// `None` when there is no such profile.
    async fn snapshot(&self, uuid: &str) -> Result<Option<Snapshot>, String>;
    async fn restore(&self, snapshot: &Snapshot) -> Result<(), String>;
    async fn apply(&self, step: &Step) -> Result<Applied, String>;
    /// Until the active connection is up, or why it will not be.
    async fn activated(&self, active: &str, limit: Duration) -> Result<(), String>;
    async fn save(&self, uuid: &str) -> Result<(), String>;
    async fn delete(&self, uuid: &str) -> Result<(), String>;
    async fn radio(&self) -> Result<bool, String>;
    async fn set_radio(&self, on: bool) -> Result<(), String>;
    /// The kernel's IPv4 default route.
    async fn route(&self) -> Option<Route>;
    async fn has_address(&self, interface: &str) -> bool;
    /// The `--verify` target once: what answered, or why nothing did.
    async fn reach(&self, verify: &Verify, route: Option<&Route>) -> Result<String, String>;
    /// A pause between tries. Real time on a device, none in the tests.
    async fn pause(&self, pause: Duration);
}

/// The transaction's files under `/data/tessaro/network`.
#[derive(Debug, Clone)]
pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    async fn write<T: Serialize + Send + 'static>(
        &self,
        name: &str,
        value: T,
    ) -> Result<(), String> {
        let dir = self.dir.clone();
        let path = self.path(name);
        blocking("writing a network change", move || {
            create_private_dir(&dir)?;
            update::fsutil::write_json(&path, &value)
                .map_err(|err| format!("{}: {err}", path.display()))
        })
        .await
    }

    async fn read<T: for<'de> Deserialize<'de> + Send + 'static>(
        &self,
        name: &str,
    ) -> Result<Option<T>, String> {
        let path = self.path(name);
        blocking("reading a network change", move || {
            if !path.exists() {
                return Ok(None);
            }
            update::fsutil::read_json(&path).map(Some)
        })
        .await
    }

    /// Both working files, in the order that leaves no record without its
    /// snapshot: the record goes first.
    async fn clear(&self) -> Result<(), String> {
        let record = self.path(RECORD);
        let snapshot = self.path(SNAPSHOT);
        blocking("clearing a network change", move || {
            for path in [record, snapshot] {
                update::fsutil::remove_if_exists(&path)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
            }
            Ok(())
        })
        .await
    }

    pub async fn last(&self) -> Result<Option<NetChange>, String> {
        self.read(LAST).await
    }

    /// The record of a change that never finished, if one is there.
    pub async fn unfinished(&self) -> Result<Option<Record>, String> {
        self.read(RECORD).await
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|err| format!("{}: {err}", dir.display()))
}

fn pass(name: &str, detail: impl Into<String>) -> NetCheck {
    NetCheck {
        name: name.to_string(),
        passed: true,
        detail: detail.into(),
    }
}

fn fail(name: &str, detail: impl Into<String>) -> NetCheck {
    NetCheck {
        name: name.to_string(),
        passed: false,
        detail: detail.into(),
    }
}

/// A failed step: why, and the checks that ran before it.
struct Failed {
    reason: String,
    checks: Vec<NetCheck>,
}

impl Failed {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            checks: Vec::new(),
        }
    }
}

/// Run `plan` to the end: committed, or rolled back. `Err` only when it
/// could not even start - nothing was changed then.
pub async fn run(ops: &dyn Ops, files: &Files, plan: Plan, log: &Log) -> Result<NetChange, String> {
    let route_before = ops.route().await;
    let radio_was = match plan.step {
        Step::Radio { .. } => Some(ops.radio().await?),
        _ => None,
    };

    let mut snapshots = Vec::new();
    for uuid in &plan.touched {
        if let Some(snapshot) = ops.snapshot(uuid).await? {
            snapshots.push(snapshot);
        }
    }
    let mut record = Record {
        action: plan.action.clone(),
        profile: plan.profile.clone(),
        checkpoint: None,
        touched: plan.touched.clone(),
        created: Vec::new(),
        radio_was,
    };
    files.write(SNAPSHOT, snapshots.clone()).await?;
    files.write(RECORD, record.clone()).await?;

    let checkpoint = match ops.checkpoint(&plan.devices, BACKSTOP).await {
        Ok(checkpoint) => checkpoint,
        Err(err) => {
            // Nothing has moved yet.
            let _ = files.clear().await;
            return Err(format!("NetworkManager refused a checkpoint: {err}"));
        }
    };
    record.checkpoint = Some(checkpoint.clone());
    files.write(RECORD, record.clone()).await?;
    log.info(format!(
        "network: {} {} started",
        plan.action,
        plan.profile.as_deref().unwrap_or("")
    ));

    // naked: attempt() and commit() wait only through ops. and files.
    let outcome = attempt(ops, files, &plan, &mut record, route_before.as_ref()).await;
    let outcome = match outcome {
        // naked: see above
        Ok(checks) => match commit(ops, &plan, &record, &checkpoint).await {
            Ok(()) => Ok(checks),
            Err(reason) => Err(Failed { reason, checks }),
        },
        Err(failed) => Err(failed),
    };

    let change = match outcome {
        Ok(checks) => {
            log.info(format!(
                "network: {} {} committed",
                plan.action,
                plan.profile.as_deref().unwrap_or("")
            ));
            NetChange {
                outcome: ChangeOutcome::Committed,
                action: plan.action.clone(),
                profile: plan.profile.clone(),
                uuid: plan
                    .uuid
                    .clone()
                    .or_else(|| record.created.first().cloned()),
                reason: None,
                checks,
                note: plan.note.clone(),
            }
        }
        Err(failed) => {
            log.info(format!(
                "network: {} {} rolled back: {}",
                plan.action,
                plan.profile.as_deref().unwrap_or(""),
                failed.reason
            ));
            // naked: undo() waits only through ops.
            undo(ops, &record, &snapshots, log).await;
            NetChange {
                outcome: ChangeOutcome::RolledBack,
                action: plan.action.clone(),
                profile: plan.profile.clone(),
                uuid: plan.uuid.clone(),
                reason: Some(failed.reason),
                checks: failed.checks,
                note: None,
            }
        }
    };

    let _ = files.clear().await;
    if let Err(err) = files.write(LAST, change.clone()).await {
        log.info(format!("network: {err}"));
    }
    Ok(change)
}

/// Apply and verify. Every check that ran is in the answer either way.
async fn attempt(
    ops: &dyn Ops,
    files: &Files,
    plan: &Plan,
    record: &mut Record,
    route_before: Option<&Route>,
) -> Result<Vec<NetCheck>, Failed> {
    let applied = ops.apply(&plan.step).await.map_err(Failed::new)?;
    if let Some(uuid) = &applied.created {
        record.created.push(uuid.clone());
        // Best effort: the checkpoint deletes new profiles on a rollback too.
        let _ = files.write(RECORD, record.clone()).await;
    }

    let mut checks = Vec::new();
    if let Some(active) = &applied.active {
        match ops.activated(active, ACTIVATE).await {
            Ok(()) => checks.push(pass(
                "activated",
                applied.device.as_deref().unwrap_or("up").to_string(),
            )),
            Err(why) => {
                checks.push(fail("activated", &why));
                return Err(Failed {
                    reason: format!("the connection did not come up: {why}"),
                    checks,
                });
            }
        }
    }

    let brings_up = !matches!(
        plan.step,
        Step::Deactivate { .. } | Step::Delete { .. } | Step::Radio { on: false }
    );
    if let (true, Some(device)) = (brings_up, &applied.device) {
        if settle(ops, SETTLE, || ops.has_address(device)).await {
            checks.push(pass("address", format!("{device} has an address")));
        } else {
            checks.push(fail("address", format!("{device} has no address")));
            return Err(Failed {
                reason: format!("{device} got no address"),
                checks,
            });
        }
    }

    let mut route = ops.route().await;
    if let Some(before) = route_before {
        if route.is_none() && settle(ops, SETTLE, || async { ops.route().await.is_some() }).await {
            route = ops.route().await;
        }
        match &route {
            Some(now) => checks.push(pass(
                "route",
                format!("default route via {}", now.interface),
            )),
            None => {
                checks.push(fail(
                    "route",
                    format!(
                        "no default route any more (it was via {})",
                        before.interface
                    ),
                ));
                return Err(Failed {
                    reason: "the device lost its default route".to_string(),
                    checks,
                });
            }
        }
    }

    match (&plan.verify, &route) {
        (Verify::None, _) => {}
        (Verify::Gateway, None) => {
            checks.push(pass("reach", "no default route, so no gateway to ask"))
        }
        (verify, _) => {
            let mut last = String::new();
            let mut reached = None;
            for attempt in 0..REACH_TRIES {
                if attempt > 0 {
                    ops.pause(Duration::from_secs(1)).await;
                }
                match ops.reach(verify, route.as_ref()).await {
                    Ok(what) => {
                        reached = Some(what);
                        break;
                    }
                    Err(why) => last = why,
                }
            }
            match reached {
                Some(what) => checks.push(pass("reach", what)),
                None => {
                    checks.push(fail("reach", &last));
                    return Err(Failed {
                        reason: format!("{} did not hold: {last}", verify.describe()),
                        checks,
                    });
                }
            }
        }
    }
    Ok(checks)
}

/// Poll `ready` once a second until it holds or `limit` is up.
async fn settle<F, Fut>(ops: &dyn Ops, limit: Duration, ready: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let tries = limit.as_secs().max(1);
    for attempt in 0..=tries {
        if attempt > 0 {
            ops.pause(Duration::from_secs(1)).await;
        }
        // naked: every caller passes an ops. method
        if ready().await {
            return true;
        }
    }
    false
}

/// Write what held to disk, then let the checkpoint go. A `Down` or `Up`
/// changed no profile, and a forgotten one is gone already.
async fn commit(
    ops: &dyn Ops,
    plan: &Plan,
    record: &Record,
    checkpoint: &str,
) -> Result<(), String> {
    let saves: Vec<&String> = match &plan.step {
        Step::Update { uuid, .. } => vec![uuid],
        Step::Add { .. } => record.created.iter().collect(),
        _ => Vec::new(),
    };
    for uuid in saves {
        ops.save(uuid)
            .await
            .map_err(|err| format!("saving {uuid}: {err}"))?;
    }
    if let Err(err) = ops.destroy(checkpoint).await {
        // The change is on disk; a checkpoint left behind would roll the
        // runtime back at the backstop, so this one is worth a line.
        return Err(format!("dropping the checkpoint: {err}"));
    }
    Ok(())
}

/// Put everything back, as far as it goes. Each part is tried whatever the
/// others did: a half undone change is worse than a noisy journal.
async fn undo(ops: &dyn Ops, record: &Record, snapshots: &[Snapshot], log: &Log) {
    // The radio first: a checkpoint cannot bring a WiFi profile back up on
    // a radio that is off.
    if let Some(was) = record.radio_was {
        if let Err(err) = ops.set_radio(was).await {
            log.info(format!("network: restoring the WiFi radio: {err}"));
        }
    }
    if let Some(checkpoint) = &record.checkpoint {
        if let Err(err) = ops.rollback(checkpoint).await {
            log.debug(format!("network: checkpoint rollback: {err}"));
        }
    }
    for snapshot in snapshots {
        if let Err(err) = ops.restore(snapshot).await {
            log.info(format!("network: restoring {}: {err}", snapshot.uuid));
        }
    }
    for uuid in &record.created {
        if let Err(err) = ops.delete(uuid).await {
            log.debug(format!("network: removing {uuid}: {err}"));
        }
    }
}

/// A change the previous agent never finished: undo it now. After a reboot
/// the in-memory half is gone already and only what reached disk is put
/// back - a forgotten profile, the radio.
pub async fn recover(ops: &dyn Ops, files: &Files, log: &Log) -> Result<bool, String> {
    let Some(record) = files.unfinished().await? else {
        return Ok(false);
    };
    let snapshots: Vec<Snapshot> = files.read(SNAPSHOT).await?.unwrap_or_default();
    log.info(format!(
        "network: rolled back an unfinished network change ({} {})",
        record.action,
        record.profile.as_deref().unwrap_or("")
    ));
    // naked: undo() waits only through ops.
    undo(ops, &record, &snapshots, log).await;
    files.clear().await?;
    let change = NetChange {
        outcome: ChangeOutcome::RolledBack,
        action: record.action.clone(),
        profile: record.profile.clone(),
        uuid: record.touched.first().cloned(),
        reason: Some("the agent stopped before it finished".to_string()),
        checks: Vec::new(),
        note: None,
    };
    files.write(LAST, change).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every call; fails the one named in `fail_at`.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail_at: Option<&'static str>,
        /// `activated` answers this error.
        not_activated: Option<String>,
        /// No default route after the change.
        loses_route: bool,
        radio: Mutex<bool>,
    }

    impl Fake {
        fn failing(at: &'static str) -> Self {
            Self {
                fail_at: Some(at),
                ..Self::default()
            }
        }

        fn call(&self, name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(name.to_string());
            if self.fail_at == Some(name) {
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn applied(&self) -> bool {
            self.calls().iter().any(|call| call == "apply")
        }
    }

    #[async_trait::async_trait]
    impl Ops for Fake {
        async fn checkpoint(&self, _: &[String], _: Duration) -> Result<String, String> {
            self.call("checkpoint").map(|()| "/cp/1".to_string())
        }
        async fn rollback(&self, _: &str) -> Result<(), String> {
            self.call("rollback")
        }
        async fn destroy(&self, _: &str) -> Result<(), String> {
            self.call("destroy")
        }
        async fn snapshot(&self, uuid: &str) -> Result<Option<Snapshot>, String> {
            self.call("snapshot")?;
            Ok(Some(Snapshot {
                uuid: uuid.to_string(),
                saved: true,
                settings: vec![1, 2, 3],
            }))
        }
        async fn restore(&self, snapshot: &Snapshot) -> Result<(), String> {
            self.call(&format!("restore {}", snapshot.uuid))
        }
        async fn apply(&self, step: &Step) -> Result<Applied, String> {
            self.call("apply")?;
            Ok(match step {
                Step::Add { device, .. } => Applied {
                    active: Some("/ac/1".into()),
                    device: Some(device.clone()),
                    created: Some("new-uuid".into()),
                },
                Step::Update {
                    device: Some(device),
                    ..
                }
                | Step::Activate {
                    device: Some(device),
                    ..
                } => Applied {
                    active: Some("/ac/1".into()),
                    device: Some(device.clone()),
                    created: None,
                },
                _ => Applied::default(),
            })
        }
        async fn activated(&self, _: &str, _: Duration) -> Result<(), String> {
            self.call("activated")?;
            match &self.not_activated {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }
        async fn save(&self, uuid: &str) -> Result<(), String> {
            self.call(&format!("save {uuid}"))?;
            self.call("save")
        }
        async fn delete(&self, uuid: &str) -> Result<(), String> {
            self.call(&format!("delete {uuid}"))
        }
        async fn radio(&self) -> Result<bool, String> {
            Ok(*self.radio.lock().unwrap())
        }
        async fn set_radio(&self, on: bool) -> Result<(), String> {
            *self.radio.lock().unwrap() = on;
            self.call(&format!("radio {on}"))
        }
        async fn route(&self) -> Option<Route> {
            if self.loses_route && self.applied() {
                return None;
            }
            Some(Route {
                interface: "eth0".into(),
                gateway: Some("10.0.2.2".parse().unwrap()),
            })
        }
        async fn has_address(&self, _: &str) -> bool {
            true
        }
        async fn reach(&self, _: &Verify, _: Option<&Route>) -> Result<String, String> {
            self.call("reach")
                .map(|()| "10.0.2.2 answered".to_string())
                .map_err(|_| "10.0.2.2 did not answer".to_string())
        }
        async fn pause(&self, _: Duration) {}
    }

    fn update() -> Plan {
        Plan {
            action: "set".into(),
            profile: Some("Wired connection 1".into()),
            uuid: Some("wired".into()),
            devices: vec!["eth0".into()],
            touched: vec!["wired".into()],
            step: Step::Update {
                uuid: "wired".into(),
                settings: Dict::new(),
                device: Some("eth0".into()),
            },
            verify: Verify::Gateway,
            note: None,
        }
    }

    fn files() -> (tempfile::TempDir, Files) {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path().join("network"));
        (dir, files)
    }

    async fn run_with(fake: &Fake, plan: Plan) -> (NetChange, Files, tempfile::TempDir) {
        let (dir, files) = files();
        let log = Log::buffered(false);
        let change = run(fake, &files, plan, &log).await.unwrap();
        (change, files, dir)
    }

    #[tokio::test]
    async fn a_change_that_holds_is_saved_only_after_it_is_verified() {
        let fake = Fake::default();
        let (change, files, _dir) = run_with(&fake, update()).await;

        assert_eq!(change.outcome, ChangeOutcome::Committed);
        let calls = fake.calls();
        let at = |name: &str| calls.iter().position(|call| call == name).unwrap();
        assert!(at("snapshot") < at("checkpoint"));
        assert!(at("checkpoint") < at("apply"));
        assert!(at("activated") < at("save wired"));
        assert!(at("reach") < at("save wired"));
        assert!(at("save wired") < at("destroy"));
        assert!(!calls.contains(&"rollback".to_string()));
        assert_eq!(files.unfinished().await.unwrap(), None);
        assert_eq!(files.last().await.unwrap(), Some(change));
    }

    #[tokio::test]
    async fn every_failure_after_the_checkpoint_rolls_back_and_saves_nothing() {
        for at in ["apply", "activated", "reach", "save"] {
            let fake = Fake::failing(at);
            let (change, files, _dir) = run_with(&fake, update()).await;

            assert_eq!(change.outcome, ChangeOutcome::RolledBack, "failing {at}");
            let calls = fake.calls();
            assert!(calls.contains(&"rollback".to_string()), "failing {at}");
            assert!(calls.contains(&"restore wired".to_string()), "failing {at}");
            assert!(!calls.contains(&"destroy".to_string()), "failing {at}");
            if at != "save" {
                assert!(!calls.iter().any(|c| c.starts_with("save")), "failing {at}");
            }
            assert_eq!(files.unfinished().await.unwrap(), None);
            assert_eq!(
                files.last().await.unwrap().unwrap().outcome,
                ChangeOutcome::RolledBack
            );
        }
    }

    #[tokio::test]
    async fn a_refused_checkpoint_changes_nothing() {
        let fake = Fake::failing("checkpoint");
        let (dir, files) = files();
        let outcome = run(&fake, &files, update(), &Log::buffered(false)).await;
        assert!(outcome.is_err());
        assert!(!fake.applied());
        assert_eq!(files.unfinished().await.unwrap(), None);
        drop(dir);
    }

    #[tokio::test]
    async fn a_wrong_password_says_so_and_removes_the_new_profile() {
        let fake = Fake {
            not_activated: Some("no secrets (wrong password?)".into()),
            ..Fake::default()
        };
        let plan = Plan {
            action: "join".into(),
            touched: Vec::new(),
            step: Step::Add {
                settings: Dict::new(),
                device: "wlan0".into(),
            },
            ..update()
        };
        let (change, _files, _dir) = run_with(&fake, plan).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(change.reason.unwrap().contains("wrong password"));
        assert!(fake.calls().contains(&"delete new-uuid".to_string()));
    }

    #[tokio::test]
    async fn losing_the_default_route_is_a_failure() {
        let fake = Fake {
            loses_route: true,
            ..Fake::default()
        };
        let plan = Plan {
            action: "down".into(),
            step: Step::Deactivate {
                uuid: "wired".into(),
            },
            ..update()
        };
        let (change, _files, _dir) = run_with(&fake, plan).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(change.checks.iter().any(|c| c.name == "route" && !c.passed));
    }

    #[tokio::test]
    async fn the_radio_comes_back_on_a_rollback() {
        let fake = Fake {
            loses_route: true,
            radio: Mutex::new(true),
            ..Fake::default()
        };
        let plan = Plan {
            action: "wifi off".into(),
            touched: Vec::new(),
            step: Step::Radio { on: false },
            ..update()
        };
        let (change, _files, _dir) = run_with(&fake, plan).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(fake.calls().contains(&"radio true".to_string()));
    }

    #[tokio::test]
    async fn an_unfinished_change_is_rolled_back_at_startup() {
        let (_dir, files) = files();
        let record = Record {
            action: "set".into(),
            profile: Some("Office".into()),
            checkpoint: Some("/cp/1".into()),
            touched: vec!["office".into()],
            created: vec!["new".into()],
            radio_was: Some(true),
        };
        let snapshot = Snapshot {
            uuid: "office".into(),
            saved: true,
            settings: vec![9],
        };
        files.write(SNAPSHOT, vec![snapshot]).await.unwrap();
        files.write(RECORD, record).await.unwrap();

        let fake = Fake::default();
        let log = Log::buffered(false);
        assert!(recover(&fake, &files, &log).await.unwrap());
        let calls = fake.calls();
        for expected in ["radio true", "rollback", "restore office", "delete new"] {
            assert!(
                calls.contains(&expected.to_string()),
                "{expected}: {calls:?}"
            );
        }
        assert!(log
            .lines()
            .iter()
            .any(|line| line.contains("rolled back an unfinished network change")));
        assert_eq!(files.unfinished().await.unwrap(), None);
        assert_eq!(
            files.last().await.unwrap().unwrap().outcome,
            ChangeOutcome::RolledBack
        );

        // Nothing left to do the second time, as after a reboot with no
        // snapshot: the record alone still undoes what it names.
        assert!(!recover(&fake, &files, &log).await.unwrap());
    }

    #[tokio::test]
    async fn the_snapshot_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, files) = files();
        files.write(SNAPSHOT, Vec::<Snapshot>::new()).await.unwrap();
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(files.path(SNAPSHOT)), 0o600);
        assert_eq!(mode(files.dir.clone()), 0o700);
    }
}
