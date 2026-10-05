//! `tessaro-ctl network certs`: the extra certificate authorities in
//! `/data/tessaro/ca-certs` (`crate::certs`).
//!
//! A change re-renders the Chromium policy, which Chromium applies without a
//! restart, and restarts the agent once the answer is out, so its own TLS
//! client is built again with the new roots (`http::trust`). The kiosk keeps
//! running through both. Like the settings, the store stays through an
//! unclaim and goes with a factory reset.

use protocol::{CertInfo, CertsAdded};

use super::{After, Caller, Control, Reply};
use crate::certs;
use crate::deadline::blocking;

impl Control {
    pub(super) async fn cert_add(&self, caller: &Caller, pem: String) -> Reply {
        let _writes = self.writes.lock().await;
        let dir = self.paths.ca_certs_dir();
        let added = match blocking("adding a certificate", move || certs::add(&dir, &pem)).await {
            Ok(added) => added,
            Err(err) => return Reply::err(err),
        };
        for cert in &added.added {
            self.log.info(format!(
                "certificate authority {} ({}) trusted by {}",
                cert.fingerprint,
                cert.subject,
                caller.describe()
            ));
        }
        let changed = !added.added.is_empty();
        if let Err(err) = self.apply_certs(changed).await {
            return Reply::err(err);
        }
        Reply::ok(CertsAdded {
            added: added.added,
            present: added.present,
        })
        .then(changed.then(|| After::Restart(self.paths.agent_unit.clone())))
    }

    pub(super) async fn cert_list(&self) -> Result<Vec<CertInfo>, String> {
        let dir = self.paths.ca_certs_dir();
        blocking("reading the certificates", move || {
            certs::list(&dir).map_err(|err| format!("{}: {err}", dir.display()))
        })
        .await
    }

    pub(super) async fn cert_revoke(&self, caller: &Caller, query: String) -> Reply {
        let _writes = self.writes.lock().await;
        let dir = self.paths.ca_certs_dir();
        let cert = match blocking("revoking a certificate", move || {
            certs::remove(&dir, &query)
        })
        .await
        {
            Ok(cert) => cert,
            Err(err) => return Reply::err(err),
        };
        self.log.info(format!(
            "certificate authority {} ({}) revoked by {}",
            cert.fingerprint,
            cert.subject,
            caller.describe()
        ));
        if let Err(err) = self.apply_certs(true).await {
            return Reply::err(err);
        }
        Reply::ok(cert).then(Some(After::Restart(self.paths.agent_unit.clone())))
    }

    /// Render the policy again from the saved settings. Nothing else in the
    /// render depends on the certificates, so nothing else changes.
    async fn apply_certs(&self, changed: bool) -> Result<(), String> {
        if !changed {
            return Ok(());
        }
        let state = self.read_state().await?;
        self.render(&state)
            .await
            .map(|_| ())
            .map_err(|err| format!("the certificates are stored, but rendering failed: {err}"))
    }
}
