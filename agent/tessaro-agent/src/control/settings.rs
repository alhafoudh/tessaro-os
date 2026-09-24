//! Changing the settings: validate, commit to `state.json`, render, restart
//! what reads the keys that changed - and the probation a guarded change
//! (`screen.resolution`) waits out before it is kept.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Consumer, Key};
use protocol::{Applied, Done, Secret};
use tokio::time::Instant;

use super::{unknown, After, Caller, Control, Reply};
use crate::audio;
use crate::deadline::blocking;
use crate::display;
use crate::nm::profiles;
use crate::render;
use crate::secrets::Secrets;
use crate::state::{self, PendingChange, State};
use crate::sync::lock;

impl Control {
    pub(super) async fn change(
        self: &Arc<Self>,
        caller: &Caller,
        changes: BTreeMap<String, Option<String>>,
        if_revision: Option<u64>,
        apply: bool,
        verify: protocol::Verify,
        wifi_psk: Option<Secret>,
    ) -> Reply {
        if changes.is_empty() {
            return Reply::err("nothing to change");
        }

        // Validate everything before touching anything. Keyed by the name as
        // given, not the registry entry's: every data.* shares one entry.
        let mut normalized: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut guarded: Vec<&'static Key> = Vec::new();
        for (name, value) in changes {
            let Some(key) = keys::find(&name) else {
                return Reply::err(unknown(&name));
            };
            let value = match value {
                Some(value) => match keys::validate(key, &value) {
                    Ok(value) => Some(value),
                    Err(err) => return Reply::err(err),
                },
                None => None,
            };
            if let (keys::Kind::Resolution, Some(mode)) = (key.kind, &value) {
                if let Err(err) = self.check_mode(mode).await {
                    return Reply::err(err);
                }
            }
            // A kind of output (`usb`) may be set before it is plugged in;
            // one output by name must be one the device has.
            if let Some(name) = &value {
                if let Some(direction) = audio::named_device(key, name) {
                    if let Err(err) = self.audio.check_name(direction, name).await {
                        return Reply::err(err);
                    }
                }
            }
            if key.guarded {
                if !apply {
                    return Reply::err(format!(
                        "{} is applied on probation and cannot be set without applying",
                        key.name
                    ));
                }
                guarded.push(key);
            }
            normalized.insert(name, value);
        }

        let network = wifi_psk.is_some()
            || normalized.keys().any(|name| {
                keys::find(name).is_some_and(|key| key.consumers.contains(&Consumer::Network))
            });
        if network && !apply {
            return Reply::err(
                "network settings are applied and checked at once; drop --no-apply".to_string(),
            );
        }

        let _writes = self.writes.lock().await;

        let edit = Edit {
            normalized,
            if_revision,
            guarded: guarded.iter().map(|key| key.name).collect(),
            default_templates: keys::TEMPLATES
                .iter()
                .map(|(name, expansion)| (*name, *expansion, self.template(&BTreeMap::new(), name)))
                .collect(),
            defaults: self.defaults.clone(),
        };

        let (committed, network_change) = if network {
            match self.change_network(caller, &edit, verify, wifi_psk).await {
                Ok(outcome) => outcome,
                Err(err) => return Reply::err(err),
            }
        } else {
            let edit = edit.clone();
            let committed = self
                .update_state("updating state.json", move |state| edit.apply(state))
                .await;
            match committed {
                Ok(outcome) => (outcome, None),
                Err(err) => return Reply::err(err),
            }
        };
        let (before, after) = committed;

        let changed = changed_keys(&before, &after.settings);
        if changed.is_empty() {
            return Reply::ok(Applied {
                revision: after.revision,
                changed: Vec::new(),
                restarted: Vec::new(),
                pending: self.pending(&after),
                network: network_change,
                audio: None,
            });
        }

        self.log.info(format!(
            "settings revision {}: {} changed by {}",
            after.revision,
            changed
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            caller.describe()
        ));

        if after.pending.is_some() && !guarded.is_empty() {
            self.arm_probation();
        }

        self.converge(&changed, &after, apply, network_change).await
    }

    /// A change that touches the network: tried as one transaction the
    /// device verifies, and saved - `state.json`, and a staged WiFi password
    /// to `secrets.json` - only once it has held. A change that did not hold
    /// is an error, and nothing is saved. The saving is done by the
    /// transaction itself, so it happens even if this caller is gone.
    async fn change_network(
        &self,
        caller: &Caller,
        edit: &Edit,
        verify: protocol::Verify,
        wifi_psk: Option<Secret>,
    ) -> Result<(Committed, Option<protocol::NetChange>), String> {
        if self.updates.is_pending() {
            return Err(
                "an update is committed and waiting for its reboot; change the network after it"
                    .to_string(),
            );
        }
        let current = self.read_state().await?;
        let mut next = current.clone();
        let (before, _) = edit.apply(&mut next)?;
        if next.settings == before && wifi_psk.is_none() {
            return Ok(((before, next), None));
        }

        let secrets = self.read_secrets().await;
        let staged = Secrets {
            wifi_psk: wifi_psk.clone().or_else(|| secrets.wifi_psk.clone()),
            ..secrets.clone()
        };
        let value = profiles::value_of(&next.settings, &self.defaults);
        keys::check_network(
            |name| profiles::effective(&value, name),
            staged.wifi_psk.is_some(),
        )?;
        let (old, new, ends_fallback) = self
            .with_fallback(
                self.net_config_for(&current.settings, &secrets),
                self.net_config_for(&next.settings, &staged),
                wifi_psk.is_some(),
            )
            .await;

        let action = match &wifi_psk {
            Some(_) => format!(
                "join {}",
                new.wifi
                    .client
                    .as_ref()
                    .map(|c| c.ssid.as_str())
                    .unwrap_or("")
            ),
            None => format!(
                "set {}",
                changed_keys(&before, &next.settings)
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        };

        // What the transaction runs once the change has held.
        let slot: Arc<Mutex<Option<Committed>>> = Arc::default();
        let commit: crate::nm::txn::Commit = {
            let store = self.state.clone();
            let secrets_store = self.secrets.clone();
            let log = Arc::clone(&self.log);
            let edit = edit.clone();
            let slot = Arc::clone(&slot);
            let fallback_marker = self.paths.wifi_fallback_marker();
            Box::pin(async move {
                let committed = blocking("saving the network settings", move || {
                    if ends_fallback {
                        match std::fs::remove_file(&fallback_marker) {
                            Ok(()) => log.info("network: the WiFi fallback ends"),
                            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                            Err(err) => {
                                log.info(format!("network: {}: {err}", fallback_marker.display()))
                            }
                        }
                    }
                    if let Some(psk) = wifi_psk {
                        secrets_store.update(&log, |secrets: &mut Secrets| {
                            secrets.wifi_psk = Some(psk);
                            Ok(())
                        })?;
                    }
                    store.update(&log, |state: &mut State| edit.apply(state))
                })
                .await?;
                *lock(&slot) = Some(committed);
                Ok(())
            })
        };

        // naked: Network bounds every NetworkManager call with within()
        let change = self
            .network
            .apply(caller.describe(), action, old, new, verify, commit)
            .await?;
        if change.outcome == protocol::ChangeOutcome::RolledBack {
            return Err(format!(
                "the network change was rolled back: {}",
                change.reason.as_deref().unwrap_or("it did not hold")
            ));
        }
        let committed = lock(&slot).take().ok_or_else(|| {
            "the network change held, but its settings were not saved".to_string()
        })?;
        Ok((committed, Some(change)))
    }

    /// browser.url as these settings, and the device as it is now, expand it.
    pub(super) async fn expanded_url(&self, settings: &BTreeMap<String, String>) -> String {
        let live = self.live().await;
        let effective = state::Effective::new(&self.defaults, settings, &self.log).with_live(live);
        crate::config::Env::get(&effective, "KIOSK_URL").unwrap_or_default()
    }

    /// Render, then restart what reads the changed keys. The reply is built
    /// here so every path that changes settings reports it the same way.
    pub(super) async fn converge(
        &self,
        changed: &[Changed],
        state: &State,
        apply: bool,
        network: Option<protocol::NetChange>,
    ) -> Reply {
        let rendered = match self.render(&state.settings).await {
            Ok(rendered) => rendered,
            Err(err) => {
                return Reply::err(format!(
                    "saved as revision {}, but rendering failed: {err}",
                    state.revision
                ))
            }
        };

        let reads = |consumer: Consumer| {
            changed
                .iter()
                .any(|(_, key)| key.consumers.contains(&consumer))
        };
        // browser.url can be built from any setting, so a change to one of
        // them can move the URL without touching a key the agent reads. The
        // test is whether the URL this agent started with is still the one.
        let url_moved = self.expanded_url(&state.settings).await != self.agent_url;

        let weston = reads(Consumer::Weston);
        let browser = !weston && (reads(Consumer::Browser) || rendered.policy_changed);
        let agent = !weston && (reads(Consumer::Agent) || url_moved);

        // Sound restarts nothing: the running server is switched at once. A
        // server that is not up yet gets the settings from the watcher when
        // it is, so the change is saved either way.
        let audio = if apply && reads(Consumer::Audio) {
            let wanted = self.audio_wanted_from(&state.settings);
            Some(match self.audio.apply(&wanted).await {
                Ok(outcome) => outcome.summary,
                Err(err) => format!("saved, not applied yet: {err}"),
            })
        } else {
            None
        };

        let mut restarted = Vec::new();
        let mut after = None;
        if apply {
            if browser {
                if let Err(err) = self.bus.restart(&self.paths.kiosk_unit).await {
                    return Reply::err(format!(
                        "saved as revision {}, but restarting the browser failed: {err}",
                        state.revision
                    ));
                }
                restarted.push(self.paths.kiosk_unit.clone());
            }
            if weston {
                restarted.push(self.paths.weston_unit.clone());
                after = Some(After::Restart(self.paths.weston_unit.clone()));
            } else if agent {
                restarted.push(self.paths.agent_unit.clone());
                after = Some(After::Restart(self.paths.agent_unit.clone()));
            }
        }

        Reply::ok(Applied {
            revision: state.revision,
            changed: changed.iter().map(|(name, _)| name.clone()).collect(),
            restarted,
            pending: self.pending(state),
            network,
            audio,
        })
        .then(after)
    }

    pub(super) async fn render(
        &self,
        settings: &BTreeMap<String, String>,
    ) -> Result<render::Rendered, String> {
        let paths = self.paths.clone();
        let defaults = self.defaults.clone();
        let settings = settings.clone();
        let log = Arc::clone(&self.log);
        blocking("rendering", move || {
            render::all(&paths, &defaults, &settings, &log)
        })
        .await
    }

    async fn check_mode(&self, mode: &str) -> Result<(), String> {
        if mode == "preferred" {
            return Ok(());
        }
        let connectors = self.modes().await?;
        if connectors.is_empty() {
            return Err(format!(
                "no connected display reports its modes, so {mode} cannot be checked; refusing"
            ));
        }
        if display::offered(&connectors, mode) {
            return Ok(());
        }
        let offered: Vec<String> = connectors
            .iter()
            .map(|c| format!("{}: {}", c.name, c.modes.join(" ")))
            .collect();
        Err(format!(
            "no connected display offers {mode}; see `tessaro-ctl screen modes` ({})",
            offered.join("; ")
        ))
    }

    // --- probation ---------------------------------------------------------

    /// Start the confirm window. Called when a guarded change is made, and at
    /// startup when one is pending - which is the usual case, because the
    /// change restarted Weston and this agent with it.
    pub fn arm_probation(self: &Arc<Self>) {
        let deadline = Instant::now() + Duration::from_secs(protocol::CONFIRM_SECONDS);
        *lock(&self.probation) = Some(deadline);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {}
                _ = shutdown.changed() => return,
            }
            control.expire_probation(deadline).await;
        });
    }

    pub async fn arm_if_pending(self: &Arc<Self>) {
        if let Ok(state) = self.read_state().await {
            if let Some(pending) = &state.pending {
                self.log.info(format!(
                    "{}={} is on probation: `tessaro-ctl screen confirm` within {}s or it reverts",
                    pending.key,
                    pending.value,
                    protocol::CONFIRM_SECONDS
                ));
                self.arm_probation();
            }
        }
    }

    async fn expire_probation(&self, deadline: Instant) {
        if *lock(&self.probation) != Some(deadline) {
            return; // confirmed, or re-armed since
        }

        let _writes = self.writes.lock().await;
        let reverted = self
            .update_state("reverting a change", |state| {
                Ok((state.revert_pending(), state.clone()))
            })
            .await;

        let (pending, state) = match reverted {
            Ok((Some(pending), state)) => (pending, state),
            Ok((None, _)) => return,
            Err(err) => {
                self.log
                    .info(format!("could not revert an unconfirmed change: {err}"));
                return;
            }
        };
        *lock(&self.probation) = None;

        self.log.info(format!(
            "{}={} was not confirmed within {}s; back to {}",
            pending.key,
            pending.value,
            protocol::CONFIRM_SECONDS,
            pending.previous_or_default()
        ));

        let changed: Vec<Changed> = keys::find(&pending.key)
            .map(|key| (pending.key.clone(), key))
            .into_iter()
            .collect();
        let reply = self.converge(&changed, &state, true, None).await;
        if let Err(err) = &reply.result {
            self.log.info(format!("reverting: {err}"));
        }
        if let Some(after) = reply.after {
            self.run_after(after).await;
        }
    }

    pub(super) async fn confirm(&self) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let kept = self
            .update_state("confirming", |state| {
                state
                    .pending
                    .take()
                    .ok_or_else(|| "nothing is waiting to be confirmed".to_string())
            })
            .await?;

        *lock(&self.probation) = None;
        self.log
            .info(format!("{}={} confirmed", kept.key, kept.value));
        Ok(Done::new(format!("kept {}={}", kept.key, kept.value)))
    }
}

/// A setting that changed: its name as stored, and the registry entry that
/// says what reads it.
pub(super) type Changed = (String, &'static Key);

/// What a committed edit leaves: the settings before, and the state after.
type Committed = (BTreeMap<String, String>, State);

/// Every stored name whose value differs, in name order.
fn changed_keys(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<Changed> {
    let names: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    names
        .into_iter()
        .filter(|name| before.get(*name) != after.get(*name))
        .filter_map(|name| keys::find(name).map(|key| (name.clone(), key)))
        .collect()
}

/// One `set`/`unset`, validated, as it is applied to `state.json`: once as a
/// dry run to know what a network change would become, then for real - by
/// the store, or by the network transaction once the change has held.
#[derive(Debug, Clone)]
struct Edit {
    normalized: BTreeMap<String, Option<String>>,
    if_revision: Option<u64>,
    /// The guarded keys among them (`screen.resolution`).
    guarded: Vec<&'static str>,
    /// Every one of `keys::TEMPLATES`, with its image default.
    default_templates: Vec<(&'static str, keys::Expansion, String)>,
    defaults: HashMap<String, String>,
}

impl Edit {
    /// Apply to `state`, returning the settings before and the state after.
    /// An edit that changes nothing leaves the revision where it was.
    fn apply(&self, state: &mut State) -> Result<Committed, String> {
        if let Some(expected) = self.if_revision {
            if state.revision != expected {
                return Err(format!(
                    "the settings are at revision {}, not {expected}; someone else changed them",
                    state.revision
                ));
            }
        }
        if !self.guarded.is_empty() {
            if let Some(pending) = &state.pending {
                return Err(format!(
                    "{}={} is waiting for `tessaro-ctl screen confirm`; confirm it or let it revert first",
                    pending.key, pending.value
                ));
            }
        }

        let before = state.settings.clone();
        for (name, value) in &self.normalized {
            match value {
                Some(value) => state.settings.insert(name.clone(), value.clone()),
                None => state.settings.remove(name),
            };
        }
        if state.settings == before {
            return Ok((before, state.clone()));
        }

        // Every {placeholder} a template uses must have a value - checked on
        // the result, so setting a template and its values in one command
        // works, and unsetting a value still in use does not. Every template,
        // whichever mode the device is in: a maintenance page or a debug
        // screen that cannot expand is found at `set`, not when someone
        // needs it. Read-only keys always have a value, so no live values
        // are needed to know what is missing.
        for (key, expansion, default) in &self.default_templates {
            let template = state.settings.get(*key).unwrap_or(default);
            let (_, missing) = state::expand(
                *expansion,
                template,
                &state.settings,
                &self.defaults,
                &state::Live::default(),
            );
            check_template(key, template, &missing)?;
        }

        state.revision += 1;
        for name in &self.guarded {
            if before.get(*name) != state.settings.get(*name) {
                state.pending = Some(PendingChange {
                    key: name.to_string(),
                    value: state
                        .settings
                        .get(*name)
                        .cloned()
                        .unwrap_or_else(|| "preferred".to_string()),
                    previous: before.get(*name).cloned(),
                });
            }
        }
        Ok((before, state.clone()))
    }
}

/// Why one of `keys::TEMPLATES` cannot be saved with these placeholders
/// missing, if it cannot.
fn check_template(key: &str, template: &str, missing: &[String]) -> Result<(), String> {
    // A custom data.* nobody set yet just needs a value; anything else is not
    // a setting at all.
    let (unset, typos): (Vec<&String>, Vec<&String>) = missing
        .iter()
        .partition(|name| keys::param_name(name).is_some());
    if let Some(typo) = typos.first() {
        // The likeliest slips: {table} for {data.table}, and an old name.
        let hint = if let Some(new) = keys::renamed(typo) {
            format!("; {typo} is {{{new}}} now")
        } else if keys::is_param(typo) {
            format!(
                "; a custom value is written in full: {{{}{typo}}}",
                keys::DATA_PREFIX
            )
        } else {
            String::new()
        };
        return Err(format!(
            "{key} {template} uses {{{typo}}}, which is not a setting \
             (and no template can contain a template){hint}; `tessaro-ctl config keys` lists them"
        ));
    }
    if !unset.is_empty() {
        return Err(format!(
            "{key} {template} uses {}; set {} (it can go in the same command)",
            unset
                .iter()
                .map(|name| format!("{{{name}}}"))
                .collect::<Vec<_>>()
                .join(", "),
            unset
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}
