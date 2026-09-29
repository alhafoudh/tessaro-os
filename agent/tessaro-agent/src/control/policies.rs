//! `tessaro-ctl browser policies`: the browser policies in `tessaro.db`
//! (`crate::policies`).
//!
//! A change re-renders the Chromium policy and restarts the browser when the
//! merged policy moved, since most policies are read only at start. A save
//! that changes nothing restarts nothing. Like the certificates, the store
//! stays through an unclaim and goes with a factory reset.

use protocol::policy::{EffectiveEntry, PolicyDoc, PolicyInfo, PolicyRemoved, PolicySaved};

use super::{Caller, Control, Reply};
use crate::deadline::blocking;
use crate::policies;
use crate::render;

impl Control {
    pub(super) async fn policy_list(&self) -> Result<Vec<PolicyInfo>, String> {
        let db = self.db.clone();
        blocking("reading the browser policies", move || policies::list(&db)).await
    }

    pub(super) async fn policy_get(&self, name: String) -> Result<PolicyDoc, String> {
        let db = self.db.clone();
        blocking("reading a browser policy", move || {
            let text = policies::read(&db, &name)?;
            Ok(PolicyDoc {
                revision: protocol::policy::revision(&text),
                name,
                text,
            })
        })
        .await
    }

    pub(super) async fn policy_set(
        &self,
        caller: &Caller,
        name: String,
        text: String,
        if_revision: Option<String>,
    ) -> Reply {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let paths = self.paths.clone();
        let stored = {
            let name = name.clone();
            let text = text.clone();
            blocking("storing a browser policy", move || {
                let changed = policies::store(&db, &name, &text, if_revision.as_deref())?;
                let docs = policies::load_checked(&db)?.checked;
                let base = render::base_keys(&paths)?;
                Ok((changed, docs, base))
            })
            .await
        };
        let (changed, docs, base) = match stored {
            Ok(stored) => stored,
            Err(err) => return Reply::err(err),
        };

        let keys: Vec<String> = protocol::policy::check(&text)
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default();
        let (shadows, shadowed_by) = policies::overlaps(&docs, &name);
        let restarted = if changed {
            self.log.info(format!(
                "browser policy {name} ({}) stored by {}",
                keys.join(", "),
                caller.describe()
            ));
            match self.apply_policies().await {
                Ok(restarted) => restarted,
                Err(err) => return Reply::err(err),
            }
        } else {
            false
        };

        Reply::ok(PolicySaved {
            revision: protocol::policy::revision(&text),
            overrides_image: keys
                .iter()
                .filter(|key| base.contains(key))
                .cloned()
                .collect(),
            name,
            keys,
            unchanged: !changed,
            shadows,
            shadowed_by,
            restarted,
        })
    }

    pub(super) async fn policy_remove(&self, caller: &Caller, name: String) -> Reply {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let removed = {
            let name = name.clone();
            blocking("removing a browser policy", move || {
                policies::remove(&db, &name)
            })
            .await
        };
        if let Err(err) = removed {
            return Reply::err(err);
        }
        self.log.info(format!(
            "browser policy {name} removed by {}",
            caller.describe()
        ));
        match self.apply_policies().await {
            Ok(restarted) => Reply::ok(PolicyRemoved { name, restarted }),
            Err(err) => Reply::err(err),
        }
    }

    pub(super) async fn policy_effective(&self) -> Result<Vec<EffectiveEntry>, String> {
        let paths = self.paths.clone();
        let db = self.db.clone();
        blocking("reading the policy", move || render::effective(&paths, &db)).await
    }

    /// Render the policy again from the saved settings and restart the
    /// browser when it changed. Returns whether it was restarted.
    async fn apply_policies(&self) -> Result<bool, String> {
        let state = self.read_state().await?;
        let rendered = self
            .render(&state.settings)
            .await
            .map_err(|err| format!("the browser policy is stored, but rendering failed: {err}"))?;
        if !rendered.policy_changed {
            return Ok(false);
        }
        self.bus
            .restart(&self.paths.kiosk_unit)
            .await
            .map_err(|err| {
                format!("the browser policy is stored, but restarting the browser failed: {err}")
            })?;
        Ok(true)
    }
}
