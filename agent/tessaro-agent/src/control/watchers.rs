//! What the control plane keeps true on its own, with nobody asking: the
//! URL a read-only key moves, the public address, Weston's config against
//! the screens and keyboards plugged in, the WiFi client's fallback to the
//! hotspot after boot, the sound server against the audio.* settings, and
//! the welcome page's `welcome.json`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use protocol::keys;
use protocol::{AudioStatus, AudioTested};
use tokio::time::Instant;

use super::{After, Control};
use crate::audio;
use crate::deadline::blocking;
use crate::hotplug;
use crate::nm::profiles;
use crate::state;
use crate::sync::lock;

/// Where `audio test --input` leaves its recording in the file store.
const AUDIO_RECORDING: &str = "audio-recording.wav";

impl Control {
    /// A browser.url that uses a read-only key - `{network.ip}` - can move with no
    /// `set` at all: DHCP renews, the link changes, and at boot the render
    /// ran before there was any address. So while the template uses one,
    /// this checks every 15s and, when the URL no longer matches the one the
    /// agent is driving, re-renders and restarts the agent onto it (and the
    /// browser, if the origin - and with it the policy - moved).
    pub fn watch_url(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(15);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = shutdown.changed() => return,
                }
                // naked: check_url's every wait is blocking()/Bus, under within()
                control.check_url().await;
            }
        });
    }

    /// Keeps `network.public_ip` current while browser.url uses it - or the debug
    /// screen is on and its template does - and only then: a link may be
    /// metered, so a device that shows no `{network.public_ip}` never asks.
    /// While one does, Cloudflare's trace is
    /// asked every 5 minutes (every 30s until the first answer, and after a
    /// failure), and the answer goes to `/run/tessaro-kiosk/public-ip`, which
    /// is all the read-only key ever reads. A failure keeps the last address
    /// rather than emptying it, so one lost request never moves the URL;
    /// `watch_url` notices when it does change. The template is checked every
    /// 15s, so a `set` that starts using the key is answered within that.
    pub fn watch_public_ip(self: &Arc<Self>) {
        const TICK: Duration = Duration::from_secs(15);
        const EVERY: Duration = Duration::from_secs(300);
        const RETRY: Duration = Duration::from_secs(30);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let http = crate::net::public_ip_client();
            // When the next request is due; `None` asks at once.
            let mut due: Option<Instant> = None;
            loop {
                // naked: a disk read under blocking()'s within()
                let wanted = control.url_uses("network.public_ip").await;
                if !wanted {
                    // Asked afresh the moment the URL uses it again.
                    due = None;
                } else if due.is_none_or(|at| Instant::now() >= at) {
                    // naked: public_ip's every phase is under its own within()
                    let wait = match control.refresh_public_ip(&http).await {
                        Ok(()) => EVERY,
                        Err(()) => RETRY,
                    };
                    due = Some(Instant::now() + wait);
                }
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(TICK) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// Does the template on screen - browser.url, or browser.maintenance.url in
    /// maintenance mode, as set, else the image default - name this key as a
    /// placeholder? While the debug screen is up, its template counts too.
    async fn url_uses(&self, key: &str) -> bool {
        let Ok(state) = self.read_state().await else {
            return false;
        };
        let (_, template) = state::shown_template(&state.settings, &self.defaults);
        keys::placeholders(&template).contains(&key)
            || (state::debug_screen(&state.settings, &self.defaults)
                && keys::placeholders(&self.template(&state.settings, keys::DEBUG_TEMPLATE))
                    .contains(&key))
    }

    /// One lookup, saved on success. A failure is logged at debug and leaves
    /// the last address in place.
    async fn refresh_public_ip(&self, http: &crate::http::HyperHttp) -> Result<(), ()> {
        // naked: public_ip's every phase is under its own within()
        match crate::net::public_ip(http).await {
            Ok(ip) => {
                // naked: a file write under blocking()'s within()
                self.store_public_ip(ip.to_string()).await;
                Ok(())
            }
            Err(err) => {
                self.log
                    .debug(format!("public address: {} {err}", crate::net::TRACE_URL));
                Err(())
            }
        }
    }

    /// Someone asked for the public address outright - `net`, or
    /// `get network.public_ip` - so look it up now, whatever browser.url uses.
    /// One request per ask; at worst the command waits out the 5s budgets.
    pub(super) async fn refresh_public_ip_now(&self) {
        // naked: public_ip's every phase is under its own within()
        let _ = self
            .refresh_public_ip(&crate::net::public_ip_client())
            .await;
    }

    pub(super) async fn store_public_ip(&self, ip: String) {
        let paths = self.paths.clone();
        let body = format!("{ip}\n");
        let written = blocking("writing the public address", move || {
            let file = paths.public_ip_file();
            crate::store::replace_if_changed(&file, body.as_bytes(), 0o644)
                .map_err(|err| format!("{}: {err}", file.display()))
        })
        .await;
        match written {
            Ok(true) => self.log.info(format!("public address is {ip}")),
            Ok(false) => {}
            Err(err) => self.log.info(format!("public address: {err}")),
        }
    }

    // --- the welcome page --------------------------------------------------

    /// Keeps `/run/tessaro-kiosk/welcome.json` on what the welcome page at
    /// http://127.0.0.1/ shows: the node name, its IPv4 addresses, the
    /// hotspot's SSID while the hotspot is up, and whether the device is
    /// claimed. Looked at every 5s, and at once after a claim or an unclaim;
    /// written only when it changes. Nothing secret goes in: nginx serves the
    /// file to any page on the loopback.
    pub fn watch_welcome(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(5);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: every wait in it is blocking() or Network, each under within()
                control.write_welcome().await;
                // naked: a timer, the claim nudge and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = control.welcome.notified() => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    async fn write_welcome(&self) {
        let paths = self.paths.clone();
        let Ok(net) = blocking("reading the network", move || {
            Ok(crate::net::snapshot(&paths))
        })
        .await
        else {
            return;
        };
        let mut hotspot = None;
        // naked: a disk read under blocking()'s within()
        if let Ok(config) = self.net_config().await {
            let interface = config.wifi.interface.as_deref();
            if self.network.hotspot_up(interface).await {
                hotspot = Some(config.wifi.hotspot_ssid);
            }
        }
        let body = welcome_json(
            &self.identity.name,
            &net,
            hotspot.as_deref(),
            self.claimed(),
        );
        let paths = self.paths.clone();
        let written = blocking("writing the welcome page's values", move || {
            let file = paths.welcome_file();
            crate::store::replace_if_changed(&file, body.as_bytes(), 0o644)
                .map_err(|err| format!("{}: {err}", file.display()))
        })
        .await;
        if let Err(err) = written {
            self.log.debug(format!("welcome page: {err}"));
        }
    }

    async fn check_url(&self) {
        let Ok(state) = self.read_state().await else {
            return;
        };
        let (name, template) = state::shown_template(&state.settings, &self.defaults);
        if !state::Live::moves(&template) {
            return;
        }

        let url = self.expanded_url(&state.settings).await;
        if url == self.agent_url {
            return;
        }

        let _writes = self.writes.lock().await;
        self.log.info(format!(
            "{name} now expands to {url} (the agent is on {}); applying",
            self.agent_url
        ));
        let reply = self.converge(&[], &state, true, None).await;
        if let Err(err) = &reply.result {
            self.log.info(format!("applying the new {name}: {err}"));
        }
        if let Some(after) = reply.after {
            self.run_after(after).await;
        }
    }

    /// Screens and keyboards plugged in or out after Weston started - see
    /// `hotplug.rs`. Every 2s the connector and input device state is read;
    /// once it has held still for 5s the generator runs again, and Weston is
    /// restarted if its answer differs from the config Weston is running.
    /// The first check runs once the state has settled at startup, which
    /// catches a screen plugged in while Weston and this agent were starting.
    ///
    /// Paused while a change is on probation: that change restarted Weston
    /// itself, and a monitor re-syncing to the new mode must not be taken for
    /// a new one. The check runs once the probation is over.
    pub fn watch_display(self: &Arc<Self>) {
        const POLL: Duration = Duration::from_secs(2);
        const SETTLE: Duration = Duration::from_secs(5);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut seen: Option<String> = None;
            let mut since = Instant::now();
            let mut checked = false;
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(POLL) => {}
                    _ = shutdown.changed() => return,
                }
                let paths = control.paths.clone();
                let Ok(now) = blocking("reading the displays and input devices", move || {
                    Ok(hotplug::snapshot(&hotplug::Sources {
                        drm: &paths.drm,
                        input: &paths.input,
                    }))
                })
                .await
                else {
                    continue;
                };
                if seen.as_deref() != Some(now.as_str()) {
                    seen = Some(now);
                    since = Instant::now();
                    checked = false;
                    continue;
                }
                if checked || since.elapsed() < SETTLE || lock(&control.probation).is_some() {
                    continue;
                }
                checked = true;
                // naked: every wait in it is blocking()/Bus, under within()
                control.reconcile_display(&now).await;
            }
        });
    }

    async fn reconcile_display(&self, snapshot: &str) {
        let paths = self.paths.clone();
        let compared = blocking("regenerating the Weston config", move || {
            // A device whose Weston is not started through the drop-in, or a
            // host run: nothing to keep true.
            let Ok(running) = std::fs::read_to_string(&paths.weston_config) else {
                return Ok(None);
            };
            let candidate = hotplug::generate(
                &paths.weston_generator,
                &paths.weston_candidate(),
                &hotplug::Sources {
                    drm: &paths.drm,
                    input: &paths.input,
                },
                &paths.generated_env(),
            )?;
            let restarted_for = std::fs::read_to_string(paths.display_reconciled()).ok();
            Ok(Some((
                hotplug::verdict(&running, &candidate),
                restarted_for,
            )))
        })
        .await;

        let (reason, restarted_for) = match compared {
            Ok(Some((Some(reason), restarted_for))) => (reason, restarted_for),
            Ok(Some((None, _))) => {
                self.log
                    .debug("display: the hardware changed, Weston's config still fits");
                return;
            }
            Ok(None) => {
                self.log.debug(format!(
                    "display: no {}, not watching hotplug",
                    self.paths.weston_config.display()
                ));
                return;
            }
            Err(err) => {
                self.log.info(format!("display: {err}"));
                return;
            }
        };

        // Weston was restarted for exactly this hardware and still came up
        // with a config the generator would not write. Another restart would
        // end the same way, on a public screen, every few seconds.
        if restarted_for.as_deref() == Some(snapshot) {
            self.log.info(format!(
                "display: {reason}, but Weston was already restarted for this hardware; leaving it"
            ));
            return;
        }

        // An operator who stopped the compositor meant it.
        let state = self.bus.active_state(&self.paths.weston_unit).await;
        if state != "active" {
            self.log.debug(format!(
                "display: {reason}, but {} is {state}",
                self.paths.weston_unit
            ));
            return;
        }

        // Not in the middle of a `set`: its restart would race this one.
        let _writes = self.writes.lock().await;
        let paths = self.paths.clone();
        let body = snapshot.to_string();
        if let Err(err) = blocking("recording the display restart", move || {
            let file = paths.display_reconciled();
            crate::store::replace_if_changed(&file, body.as_bytes(), 0o644)
                .map(|_| ())
                .map_err(|err| format!("{}: {err}", file.display()))
        })
        .await
        {
            // Without the record there is no loop guard; better not restart.
            self.log
                .info(format!("display: {reason}, not restarting: {err}"));
            return;
        }

        self.log.info(format!(
            "display: {reason}; restarting {}",
            self.paths.weston_unit
        ));
        self.run_after(After::Restart(self.paths.weston_unit.clone()))
            .await;
    }

    // --- WiFi fallback -----------------------------------------------------

    /// Gives the WiFi device to the hotspot when the client has not connected
    /// within `network.wifi.fallback_after` of this agent first seeing it
    /// armed, so a device moved away from its network can still be reached.
    /// Boot only: once the client has been up, or has fallen back, this is
    /// over until the next boot, which renders the client again (the markers
    /// are in `/run`). A network that drops later is NetworkManager's to
    /// retry. The clock starts again while a network change runs, and while
    /// WiFi is not a client with a network to join.
    pub fn watch_wifi(self: &Arc<Self>) {
        const POLL: Duration = Duration::from_secs(5);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut since: Option<Instant> = None;
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(POLL) => {}
                    _ = shutdown.changed() => return,
                }
                // naked: marker reads under blocking()'s within()
                if control.wifi_client_seen().await || control.wifi_fallback().await.is_some() {
                    return;
                }
                // naked: a disk read under blocking(), NetworkManager under nm_call()
                let Some(watch) = control.wifi_watched().await else {
                    since = None;
                    continue;
                };
                // naked: Network bounds every NetworkManager call with within()
                if control.network.client_up(watch.interface.as_deref()).await {
                    // naked: a marker write under blocking()'s within()
                    control.wifi_client_up(&watch).await;
                    return;
                }
                let started = *since.get_or_insert_with(Instant::now);
                if started.elapsed() >= watch.after {
                    // naked: blocking() and Network, every wait under within()
                    control.fall_back_wifi(&watch).await;
                    return;
                }
            }
        });
    }

    /// The client the fallback waits on right now, if it is armed.
    async fn wifi_watched(&self) -> Option<profiles::FallbackWatch> {
        let state = self.read_state().await.ok()?;
        let value = profiles::value_of(&state.settings, &self.defaults);
        let watch = profiles::fallback_watch(&value)?;
        let wanted = watch.interface.as_deref();
        if self.network.busy() || self.network.wifi_device(wanted).await.is_none() {
            return None;
        }
        Some(watch)
    }

    async fn wifi_client_up(&self, watch: &profiles::FallbackWatch) {
        let marker = self.paths.wifi_client_seen_marker();
        if let Err(err) = self.write_marker(marker, String::new()).await {
            self.log.info(format!("network: {err}"));
        }
        self.log.info(format!(
            "network: WiFi client {} is up; no fallback until the next boot",
            watch.ssid
        ));
    }

    /// The marker goes first, so a re-render meanwhile - a claim, an agent
    /// restart - already renders the hotspot.
    async fn fall_back_wifi(&self, watch: &profiles::FallbackWatch) {
        let marker = self.paths.wifi_fallback_marker();
        if let Err(err) = self.write_marker(marker, watch.ssid.clone()).await {
            self.log
                .info(format!("network: not falling back to the hotspot: {err}"));
            return;
        }
        let config = match self.net_config().await {
            Ok(config) => config,
            Err(err) => {
                self.log.info(format!("network: {err}"));
                return;
            }
        };
        self.log.info(format!(
            "network: WiFi client {} did not connect within {}s; falling back to the \
             hotspot {} until the next boot",
            watch.ssid,
            watch.after.as_secs(),
            config.wifi.hotspot_ssid
        ));
        if let Err(err) = self.network.fall_back(&config).await {
            self.log
                .info(format!("network: bringing up the hotspot: {err}"));
        }
    }

    // --- audio -------------------------------------------------------------

    /// Keeps the sound server on the audio.* settings - see `audio.rs`. They
    /// are applied once PipeWire is up, again whenever the sound hardware
    /// changes (a sound card comes or goes, a screen is plugged in, PipeWire
    /// restarts) and has held still for a moment, and once a minute anyway,
    /// which puts back anything else that moved it. The hardware is read from
    /// the kernel every 2s, which is cheap; PipeWire is only asked when there
    /// is something to apply. Applying is idempotent and logs real changes
    /// only.
    pub fn watch_audio(self: &Arc<Self>) {
        const POLL: Duration = Duration::from_secs(2);
        const SETTLE: Duration = Duration::from_secs(2);
        /// PipeWire not up yet, or nothing to play on yet.
        const RETRY: Duration = Duration::from_secs(5);
        const EVERY: Duration = Duration::from_secs(60);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut seen: Option<String> = None;
            let mut due = Instant::now();
            // The last problem logged, so a retry every few seconds is one
            // journal line, not one per retry.
            let mut reported: Option<String> = None;
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(POLL) => {}
                    _ = shutdown.changed() => return,
                }
                let paths = control.paths.clone();
                let Ok(now) = blocking("reading the sound hardware", move || {
                    Ok(audio::hardware(&paths))
                })
                .await
                else {
                    continue;
                };
                if seen.as_deref() != Some(now.as_str()) {
                    seen = Some(now);
                    due = Instant::now() + SETTLE;
                    continue;
                }
                if Instant::now() < due {
                    continue;
                }
                // naked: apply_audio waits only on blocking() and Audio, both bounded
                let outcome = control.apply_audio().await;
                due = Instant::now()
                    + match &outcome {
                        Ok(done) if !done.incomplete => EVERY,
                        _ => RETRY,
                    };
                let problem = outcome.err();
                if problem != reported {
                    match &problem {
                        // Normal for a few seconds at boot, and on a host.
                        Some(err) if err.starts_with("PipeWire is not running") => {
                            control.log.debug(format!("audio: {err}"))
                        }
                        Some(err) => control.log.info(format!("audio: {err}")),
                        None => {}
                    }
                    reported = problem;
                }
            }
        });
    }

    async fn apply_audio(&self) -> Result<audio::Outcome, String> {
        let wanted = self.audio_wanted().await?;
        self.audio.apply(&wanted).await
    }

    /// What the audio.* settings ask for now, set or image default.
    async fn audio_wanted(&self) -> Result<audio::Wanted, String> {
        let state = self.read_state().await?;
        Ok(self.audio_wanted_from(&state.settings))
    }

    pub(super) fn audio_wanted_from(&self, settings: &BTreeMap<String, String>) -> audio::Wanted {
        audio::Wanted::from_env(&state::Effective::new(&self.defaults, settings, &self.log))
    }

    pub(super) async fn audio_status(&self) -> Result<AudioStatus, String> {
        let wanted = self.audio_wanted().await?;
        Ok(self.audio.status(&wanted).await)
    }

    /// A recording is kept in the file store, over the one before, so it can
    /// be downloaded and listened to. Not being able to keep it still leaves
    /// the level worth reporting.
    pub(super) async fn audio_test(
        &self,
        caller: &str,
        input: bool,
    ) -> Result<AudioTested, String> {
        let wanted = self.audio_wanted().await?;
        if !input {
            return self.audio.test_output(&wanted).await;
        }
        let (mut tested, recording) = self.audio.test_input(&wanted).await?;
        match self.files.store(caller, AUDIO_RECORDING, recording).await {
            Ok(()) => {
                tested
                    .message
                    .push_str(&format!("; saved as /{AUDIO_RECORDING}"));
                tested.saved = Some(AUDIO_RECORDING.to_string());
            }
            Err(err) => tested.message.push_str(&format!("; not saved: {err}")),
        }
        Ok(tested)
    }
}

/// The body of `welcome.json`. IPv4 global addresses only, with the
/// interface each is on: those are what someone reads off the screen to reach
/// the device. The hotspot is `null` unless it is up.
fn welcome_json(node: &str, net: &protocol::Net, hotspot: Option<&str>, claimed: bool) -> String {
    let addresses: Vec<_> = net
        .interfaces
        .iter()
        .filter(|interface| interface.kind != "loopback")
        .flat_map(|interface| {
            interface
                .addresses
                .iter()
                .filter(|address| address.family == "ipv4" && address.scope == "global")
                .map(|address| {
                    serde_json::json!({
                        "address": address.address,
                        "interface": interface.name,
                    })
                })
        })
        .collect();
    let body = serde_json::json!({
        "node": node,
        "addresses": addresses,
        "hotspot": hotspot.map(|ssid| serde_json::json!({ "ssid": ssid })),
        "claimed": claimed,
    });
    format!("{body}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Net, NetAddress, NetInterface};

    fn interface(name: &str, kind: &str, addresses: &[(&str, &str, &str)]) -> NetInterface {
        NetInterface {
            name: name.into(),
            kind: kind.into(),
            mac: None,
            state: "up".into(),
            carrier: Some(true),
            mtu: None,
            speed_mbps: None,
            default_route: false,
            addresses: addresses
                .iter()
                .map(|(address, family, scope)| NetAddress {
                    address: (*address).into(),
                    prefix: 24,
                    family: (*family).into(),
                    scope: (*scope).into(),
                })
                .collect(),
        }
    }

    fn net(interfaces: Vec<NetInterface>) -> Net {
        Net {
            hostname: "tessaro".into(),
            interface: None,
            gateway: None,
            dns: vec![],
            interfaces,
            public_ip: None,
        }
    }

    #[test]
    fn the_welcome_page_gets_global_ipv4_addresses_only() {
        let net = net(vec![
            interface("lo", "loopback", &[("127.0.0.1", "ipv4", "loopback")]),
            interface(
                "eth0",
                "ethernet",
                &[
                    ("192.168.1.42", "ipv4", "global"),
                    ("fe80::1", "ipv6", "link-local"),
                    ("2001:db8::1", "ipv6", "global"),
                ],
            ),
            interface("wlan0", "wireless", &[("10.42.0.1", "ipv4", "global")]),
        ]);
        let body: serde_json::Value =
            serde_json::from_str(&welcome_json("lobby", &net, None, false)).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "node": "lobby",
                "addresses": [
                    { "address": "192.168.1.42", "interface": "eth0" },
                    { "address": "10.42.0.1", "interface": "wlan0" },
                ],
                "hotspot": null,
                "claimed": false,
            })
        );
    }

    #[test]
    fn the_welcome_page_names_a_running_hotspot() {
        let body: serde_json::Value = serde_json::from_str(&welcome_json(
            "lobby",
            &net(vec![]),
            Some("tessaro-lobby"),
            true,
        ))
        .unwrap();
        assert_eq!(
            body["hotspot"],
            serde_json::json!({ "ssid": "tessaro-lobby" })
        );
        assert_eq!(body["claimed"], true);
        assert_eq!(body["addresses"], serde_json::json!([]));
    }
}
