//! Changing the settings: validate, commit to `tessaro.db`, render, restart
//! what reads the keys that changed - and the probation a guarded change
//! (`screen.resolution`) waits out before it is kept.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Consumer, Key};
use protocol::{Applied, Done, Secret};
use tokio::time::Instant;

use super::{not_offered, unknown, After, Caller, Control, Reply};
use crate::audio;
use crate::config::{Config, Current};
use crate::db;
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
            // Unset stays allowed, so a value copied over from a device that
            // has the hardware can still be taken out.
            if value.is_some() && !self.paths.offers(key) {
                return Reply::err(not_offered(key));
            }
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
            // A timezone must be one timedated can switch to.
            if let (keys::Kind::Timezone, Some(zone)) = (key.kind, &value) {
                if let Err(err) = self.time.check_zone(&self.bus, zone).await {
                    return Reply::err(err);
                }
            }
            // A playlist must be one the device has.
            if let (keys::Kind::Playlist, Some(name)) = (key.kind, &value) {
                if !name.is_empty() {
                    if let Err(err) = self.check_playlist(name).await {
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
                .update_state("updating the settings", move |state| edit.apply(state))
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
                time: None,
                reboot: false,
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

        if !after.pending.is_empty() && !guarded.is_empty() {
            self.arm_probation();
        }

        self.converge(&changed, &after, apply, network_change).await
    }

    /// A change that touches the network: tried as one transaction the
    /// device verifies, and saved - the settings, and a staged WiFi password,
    /// in one transaction - only once it has held. A change that did not hold
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
            let db = self.db.clone();
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
                    // The password and the settings together, or neither.
                    db.transaction(|tx| {
                        if let Some(psk) = wifi_psk {
                            let mut secrets = db::load::<Secrets>(tx)?;
                            secrets.wifi_psk = Some(psk);
                            db::save(tx, &secrets)?;
                        }
                        let mut state = db::load::<State>(tx)?;
                        let committed = edit.apply(&mut state)?;
                        db::save(tx, &state)?;
                        Ok(committed)
                    })
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

    /// The agent's configuration as these settings, and the device as it is
    /// now, make it: what `main` builds at start, from the same defaults.
    async fn config_from(&self, settings: &BTreeMap<String, String>) -> Config {
        let live = self.live().await;
        let effective = state::Effective::new(&self.defaults, settings, &self.log).with_live(live);
        Config::load(&effective)
    }

    /// browser.url as these settings, and the device as it is now, expand it.
    pub(super) async fn expanded_url(&self, settings: &BTreeMap<String, String>) -> String {
        self.config_from(settings).await.kiosk_url
    }

    /// Hand the running agent the configuration these settings make: the
    /// state machine, the debug screen, the watchdog and the page bridge
    /// follow it at once, and the log's verbosity is switched here. Nothing
    /// is published when nothing the agent reads moved.
    fn publish(&self, config: Config, settings: &BTreeMap<String, String>) {
        self.log.set_debug(config.debug);
        let next = Current {
            config,
            settings: settings.clone(),
        };
        self.current.send_if_modified(|current| {
            if **current == next {
                return false;
            }
            *current = Arc::new(next);
            true
        });
    }

    /// Render, apply what the agent reads, then restart what reads the
    /// other changed keys. The reply is built here so every path that
    /// changes settings reports it the same way.
    pub(super) async fn converge(
        &self,
        changed: &[Changed],
        state: &State,
        apply: bool,
        network: Option<protocol::NetChange>,
    ) -> Reply {
        let rendered = match self.render(state).await {
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
        let weston = reads(Consumer::Weston);
        let browser = !weston && (reads(Consumer::Browser) || rendered.policy_changed);

        // What the agent reads applies at once, whichever key moved it:
        // browser.url can be built from any setting. Only what it sets up
        // once per process restarts it, and the proxy being switched on or
        // off, which its clients are built for.
        let config = self.config_from(&state.settings).await;
        let agent = restarts_agent(changed, &config, self.proxy);
        if apply {
            // Before any browser restart below, so the browser comes back to
            // the new page with the new scripts.
            self.publish(config, &state.settings);
        }
        // The bridge, and the page's copy of the settings, follow.
        self.poke_bridge();

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

        // The clock restarts nothing but timesyncd, and only when its servers
        // changed. A timedated that does not answer gets the settings from
        // the watcher later, so the change is saved either way.
        let time = if apply && reads(Consumer::Time) {
            let wanted = self.time_wanted_from(&state.settings);
            Some(match self.time.apply(&self.bus, &wanted).await {
                Ok(outcome) => outcome.summary,
                Err(err) => format!("saved, not applied yet: {err}"),
            })
        } else {
            None
        };

        let mut restarted = Vec::new();
        let mut after = None;
        if apply {
            // The local proxy first, so a browser restarted for it below
            // comes up with the proxy already there. Its config in /run is
            // what decides: rendered means run it, removed means stop it.
            if reads(Consumer::Proxy) && rendered.proxy_changed {
                let unit = &self.paths.proxy_unit;
                let run = self.paths.proxy_config.exists();
                let outcome = if run {
                    self.bus.restart(unit).await
                } else {
                    self.bus.stop(unit).await
                };
                if run && outcome.is_ok() {
                    restarted.push(unit.clone());
                }
                if let Err(err) = outcome {
                    return Reply::err(format!(
                        "saved as revision {}, but the local proxy did not follow: {err}",
                        state.revision
                    ));
                }
            }
            // Every running camera mirror, to capture in the new format. One
            // stopped stays stopped (TryRestart): udev starts it when its
            // camera is plugged in, and it reads camera.env then.
            if reads(Consumer::Camera) && rendered.camera_changed {
                let outcome = match self.bus.list_units(&[&self.paths.camera_units]).await {
                    Ok(units) => {
                        let mut outcome = Ok(());
                        let running = units
                            .into_iter()
                            .filter(|(_, active)| active == "active" || active == "activating");
                        for (unit, _) in running {
                            match self.bus.try_restart(&unit).await {
                                Ok(()) => restarted.push(unit),
                                Err(err) => outcome = Err(err),
                            }
                        }
                        outcome
                    }
                    Err(err) => Err(err),
                };
                if let Err(err) = outcome {
                    return Reply::err(format!(
                        "saved as revision {}, but a camera did not follow: {err}",
                        state.revision
                    ));
                }
            }
            if browser {
                if let Err(err) = self.bus.restart(&self.paths.kiosk_unit).await {
                    return Reply::err(format!(
                        "saved as revision {}, but restarting the browser failed: {err}",
                        state.revision
                    ));
                }
                restarted.push(self.paths.kiosk_unit.clone());
            }
            // The agent last: it is this process.
            let mut units = Vec::new();
            if weston {
                units.push(self.paths.weston_unit.clone());
            }
            if agent {
                units.push(self.paths.agent_unit.clone());
            }
            restarted.extend(units.iter().cloned());
            after = match units.len() {
                0 => None,
                1 => units.pop().map(After::Restart),
                _ => Some(After::Restarts(units)),
            };
        }

        Reply::ok(Applied {
            revision: state.revision,
            changed: changed.iter().map(|(name, _)| name.clone()).collect(),
            restarted,
            pending: self.pending(state),
            network,
            audio,
            time,
            // Never rebooted for: a reboot blanks a public screen, so when is
            // the operator's call. Written whether or not the change was
            // applied, since the firmware reads it only at power-on anyway.
            reboot: rendered.firmware_changed,
        })
        .then(after)
    }

    pub(super) async fn render(&self, state: &State) -> Result<render::Rendered, String> {
        let paths = self.paths.clone();
        let defaults = self.defaults.clone();
        let state = state.clone();
        let log = Arc::clone(&self.log);
        blocking("rendering", move || {
            render::state(&paths, &defaults, &state, &log)
        })
        .await
    }

    /// Render what follows a change only once it is kept, after a confirm.
    /// Everything else already follows it, and restarts nothing again.
    async fn render_confirmed(&self, state: &State) {
        let paths = self.paths.clone();
        let defaults = self.defaults.clone();
        let state = state.clone();
        let rendered = blocking("rendering the splash", move || {
            render::render_splash(&paths, &defaults, &state)
        })
        .await;
        if let Err(err) = rendered {
            self.log
                .info(format!("confirmed, but the splash did not follow: {err}"));
        }
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
    /// startup when one is pending, after a crash or a reboot.
    pub fn arm_probation(self: &Arc<Self>) {
        let deadline = Instant::now() + Duration::from_secs(protocol::CONFIRM_SECONDS);
        *lock(&self.probation) = Some(deadline);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut deadline = deadline;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {}
                    _ = shutdown.changed() => return,
                }
                // Held open for longer since (`extend_probation`): wait on.
                match *lock(&control.probation) {
                    Some(later) if later > deadline => deadline = later,
                    _ => break,
                }
            }
            control.expire_probation(deadline).await;
        });
    }

    /// Give the change on probation, if there is one, its whole confirm
    /// window from now: Weston was just restarted for it, and the screen it
    /// is judged on is only coming back.
    pub(super) fn extend_probation(&self) {
        if let Some(deadline) = lock(&self.probation).as_mut() {
            *deadline = Instant::now() + Duration::from_secs(protocol::CONFIRM_SECONDS);
        }
    }

    pub async fn arm_if_pending(self: &Arc<Self>) {
        if let Ok(state) = self.read_state().await {
            if !state.pending.is_empty() {
                self.log.info(format!(
                    "{} on probation: `tessaro-ctl screen confirm` within {}s or it reverts",
                    state::listed(&state.pending),
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
            Ok((pending, _)) if pending.is_empty() => return,
            Ok(reverted) => reverted,
            Err(err) => {
                self.log
                    .info(format!("could not revert an unconfirmed change: {err}"));
                return;
            }
        };
        *lock(&self.probation) = None;

        self.log.info(format!(
            "{} not confirmed within {}s; {}",
            state::listed(&pending),
            protocol::CONFIRM_SECONDS,
            state::listed_back(&pending)
        ));

        let changed: Vec<Changed> = pending
            .iter()
            .filter_map(|change| keys::find(&change.key).map(|key| (change.key.clone(), key)))
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
        let (kept, state) = self
            .update_state("confirming", |state| {
                if state.pending.is_empty() {
                    return Err("nothing is waiting to be confirmed".to_string());
                }
                Ok((std::mem::take(&mut state.pending), state.clone()))
            })
            .await?;

        *lock(&self.probation) = None;
        let kept = state::listed(&kept);
        self.log.info(format!("{kept} confirmed"));
        // What follows only a kept change (the splash) follows it now.
        self.render_confirmed(&state).await;
        Ok(Done::new(format!("kept {kept}")))
    }
}

/// A setting that changed: its name as stored, and the registry entry that
/// says what reads it.
pub(super) type Changed = (String, &'static Key);

/// Whether these changes need the agent to restart: a key it sets up once
/// per process (`Consumer::AgentRestart`), or the proxy switched on or off
/// against how the process started (`running`), which its clients are built
/// for. A new upstream or bypass keeps the same local proxy address.
pub(super) fn restarts_agent(
    changed: &[Changed],
    config: &Config,
    running: Option<std::net::SocketAddr>,
) -> bool {
    changed
        .iter()
        .any(|(_, key)| key.consumers.contains(&Consumer::AgentRestart))
        || config.proxy != running
}

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

/// One `set`/`unset`, validated, as it is applied to the settings: once as a
/// dry run to know what a network change would become, then for real - by
/// the store, or by the network transaction once the change has held.
#[derive(Debug, Clone)]
struct Edit {
    normalized: BTreeMap<String, Option<String>>,
    if_revision: Option<u64>,
    /// The guarded keys among them (`screen.resolution`, `screen.rotation`),
    /// in name order.
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
        if !self.guarded.is_empty() && !state.pending.is_empty() {
            let (is, it) = if state.pending.len() == 1 {
                ("is", "it")
            } else {
                ("are", "them")
            };
            return Err(format!(
                "{} {is} waiting for `tessaro-ctl screen confirm`; confirm {it} or let {it} revert first",
                state::listed(&state.pending)
            ));
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
        // Every guarded key this changed, together: one confirm keeps them
        // all, and a revert takes them all back. An unset one is on
        // probation at the image default it falls back to.
        for name in &self.guarded {
            if before.get(*name) != state.settings.get(*name) {
                state.pending.push(PendingChange {
                    key: name.to_string(),
                    value: state::setting(&state.settings, &self.defaults, name)
                        .unwrap_or_default(),
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
        // The likeliest slip: {table} for {data.table}.
        let hint = if keys::is_param(typo) {
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
