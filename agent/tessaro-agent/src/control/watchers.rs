//! What the control plane keeps true on its own, with nobody asking: the
//! URL a read-only key moves, the public address, Weston's config against
//! the screens and keyboards plugged in, the WiFi client's fallback to the
//! hotspot after boot, the sound server against the audio.* settings, the
//! clock against the time.* settings, the welcome page's `welcome.json`, and
//! the CPU use `status` reports.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use protocol::keys;
use protocol::{AudioStatus, AudioTested, TimeStatus};
use tokio::time::Instant;

use super::{After, Control};
use crate::audio;
use crate::deadline::blocking;
use crate::hardware;
use crate::hotplug;
use crate::nm::profiles;
use crate::state;
use crate::sync::lock;
use crate::time;

/// Where `audio test --input` leaves its recording in the file store.
const AUDIO_RECORDING: &str = "audio-recording.wav";

impl Control {
    /// A browser.url that uses a read-only key - `{network.ip}` - can move with no
    /// `set` at all: DHCP renews, the link changes, and at boot the render
    /// ran before there was any address. So while the template uses one,
    /// this checks every 15s and, when the URL no longer matches the one the
    /// agent is driving, re-renders and hands the agent the new one, which
    /// navigates to it (and restarts the browser, if the origin - and with it
    /// the policy - moved).
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
    ///
    /// The same lookup is the online indicator: while the welcome page is on
    /// screen, or Quick Setup was read in the last 2 minutes, it is asked
    /// every minute, since that is someone watching the indicator and waiting
    /// on a cable or a WiFi join. A device showing its real page asks neither.
    pub fn watch_public_ip(self: &Arc<Self>) {
        const TICK: Duration = Duration::from_secs(15);
        const EVERY: Duration = Duration::from_secs(300);
        const WATCHED: Duration = Duration::from_secs(60);
        const RETRY: Duration = Duration::from_secs(30);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let http = crate::net::public_ip_client(control.proxy);
            // When the next request is due; `None` asks at once.
            let mut due: Option<Instant> = None;
            loop {
                // naked: a disk read under blocking()'s within()
                let watched = control.online_watched().await;
                // naked: a disk read under blocking()'s within()
                let wanted = watched || control.url_uses("network.public_ip").await;
                if !wanted {
                    // Asked afresh the moment the URL uses it again.
                    due = None;
                } else if due.is_none_or(|at| Instant::now() >= at) {
                    // naked: public_ip's every phase is under its own within()
                    let wait = match control.refresh_public_ip(&http).await {
                        Ok(()) if watched => WATCHED,
                        Ok(()) => EVERY,
                        Err(()) => RETRY,
                    };
                    due = Some(Instant::now() + wait);
                } else if let Some(at) = due {
                    // Someone started watching: a 5-minute wait shrinks to one.
                    if watched && at > Instant::now() + WATCHED {
                        due = Some(Instant::now() + WATCHED);
                    }
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

    /// Is anyone looking at the online indicator: the welcome page on screen,
    /// or Quick Setup read within the last 2 minutes?
    async fn online_watched(&self) -> bool {
        const PORTAL: Duration = Duration::from_secs(120);
        if lock(&self.portal_seen).is_some_and(|at| at.elapsed() < PORTAL) {
            return true;
        }
        // naked: a disk read under blocking()'s within()
        self.welcome_shown().await
    }

    /// Is the welcome page what the screen shows? It is browser.url's image
    /// default, so this is browser.url unset or set to it, with neither
    /// maintenance mode nor the debug screen in front of it.
    pub(super) async fn welcome_shown(&self) -> bool {
        let Ok(state) = self.read_state().await else {
            return false;
        };
        if state::debug_screen(&state.settings, &self.defaults) {
            return false;
        }
        let (_, template) = state::shown_template(&state.settings, &self.defaults);
        is_welcome(&template, &self.paths.selftest_origin)
    }

    /// Quick Setup read the device's state: keep the online check going.
    pub(crate) fn portal_seen(&self) {
        *lock(&self.portal_seen) = Some(Instant::now());
    }

    /// Whether the last lookup answered; `None` before the first one.
    pub(crate) fn online(&self) -> Option<bool> {
        *lock(&self.online)
    }

    /// Something the welcome page or the captive flag show changed: write
    /// them now rather than at the next tick.
    pub(crate) fn nudge_welcome(&self) {
        self.welcome.notify_one();
    }

    /// Keeps the captive flag nginx looks at: there while the device is
    /// unclaimed and network.wifi.captive is on, gone otherwise. Written
    /// with the welcome page, so a claim or a portal change reaches it at
    /// once. No portal directory, no captive portal: nothing to write.
    async fn write_captive_flag(&self) {
        let flag = self.paths.captive_flag();
        // naked: a disk read under blocking()'s within()
        let Ok(state) = self.read_state().await else {
            return;
        };
        let on = !self.claimed()
            && state::setting(&state.settings, &self.defaults, keys::WIFI_CAPTIVE).as_deref()
                == Some("1");
        let written = blocking("writing the captive flag", move || {
            if !flag.parent().is_some_and(|dir| dir.is_dir()) {
                return Ok(false);
            }
            let was = flag.exists();
            if on && !was {
                std::fs::write(&flag, b"").map_err(|err| format!("{}: {err}", flag.display()))?;
            } else if !on && was {
                std::fs::remove_file(&flag).map_err(|err| format!("{}: {err}", flag.display()))?;
            }
            Ok(on != was)
        })
        .await;
        match written {
            Ok(true) => self.log.info(if on {
                "quick setup: a phone joining the hotspot gets the sign-in sheet"
            } else {
                "quick setup: no sign-in sheet for phones on the hotspot"
            }),
            Ok(false) => {}
            Err(err) => self.log.info(format!("quick setup: {err}")),
        }
    }

    /// One lookup, saved on success. A failure is logged at debug and leaves
    /// the last address in place. Either way it is the online state.
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
                self.set_online(false);
                Err(())
            }
        }
    }

    pub(super) fn set_online(&self, online: bool) {
        let was = lock(&self.online).replace(online);
        if was != Some(online) {
            self.log.info(if online {
                "online: the public address lookup answers"
            } else {
                "offline: the public address lookup does not answer"
            });
            self.welcome.notify_one();
        }
    }

    /// Someone asked for the public address outright - `net`, or
    /// `get network.public_ip` - so look it up now, whatever browser.url uses.
    /// One request per ask; at worst the command waits out the 5s budgets.
    pub(super) async fn refresh_public_ip_now(&self) {
        // naked: public_ip's every phase is under its own within()
        let _ = self
            .refresh_public_ip(&crate::net::public_ip_client(self.proxy))
            .await;
    }

    /// A lookup answered, from any caller: save the address, and the device
    /// is online.
    pub(super) async fn store_public_ip(&self, ip: String) {
        self.set_online(true);
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
    /// hotspot's SSID while the hotspot is up, whether the device is
    /// claimed and online, and the setup QR code while it is unclaimed with
    /// the hotspot up. Looked at every 5s, and at once after a claim, an
    /// unclaim or a change of online; written only when it changes. Nothing
    /// secret goes in: nginx serves the file to any page on the loopback, and
    /// the QR is only ever shown for the open hotspot.
    pub fn watch_welcome(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(5);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: every wait in it is blocking() or Network, each under within()
                control.write_welcome().await;
                // naked: disk reads and writes under blocking()'s within()
                control.write_captive_flag().await;
                // naked: a timer, the claim nudge and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = control.welcome.notified() => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// What the welcome page shows, which Quick Setup shows too.
    pub(crate) async fn welcome(&self) -> Result<serde_json::Value, String> {
        let paths = self.paths.clone();
        let net = blocking("reading the network", move || {
            Ok(crate::net::snapshot(&paths))
        })
        .await?;
        let mut hotspot = None;
        // naked: a disk read under blocking()'s within()
        if let Ok(config) = self.net_config().await {
            let interface = config.wifi.interface.as_deref();
            if self.network.hotspot_up(interface).await {
                hotspot = Some(config.wifi.hotspot_ssid);
            }
        }
        Ok(welcome_value(&Welcome {
            node: &self.identity.name,
            net: &net,
            hotspot: hotspot.as_deref(),
            claimed: self.claimed(),
            online: self.online(),
        }))
    }

    async fn write_welcome(&self) {
        // naked: every wait in it is blocking() or Network, each under within()
        let Ok(value) = self.welcome().await else {
            return;
        };
        let body = format!("{value}\n");
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
        let shown = self.agent_url();
        if url == shown {
            return;
        }

        let _writes = self.writes.lock().await;
        self.log.info(format!(
            "{name} now expands to {url} (the agent is on {shown}); applying"
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

    // --- time --------------------------------------------------------------

    /// Keeps the clock on the time.* settings - see `time.rs`. Applied as
    /// soon as the agent starts, which is what puts the timesyncd drop-in
    /// back in `/run` after a boot, then once a minute: that follows a DHCP
    /// lease that brought other NTP servers, and a timesyncd restarted by
    /// anyone, which forgets its runtime servers. Applying is idempotent
    /// and logs real changes only.
    pub fn watch_time(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(60);
        /// timedated or the bus not answering yet.
        const RETRY: Duration = Duration::from_secs(10);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut reported: Option<String> = None;
            loop {
                // naked: apply_time waits only on blocking() and Time, each bounded
                let outcome = control.apply_time().await;
                let wait = if outcome.is_ok() { EVERY } else { RETRY };
                let problem = outcome.err();
                if problem != reported {
                    if let Some(err) = &problem {
                        control.log.info(format!("time: {err}"));
                    }
                    reported = problem;
                }
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// Keeps `status`'s CPU use current: `/proc/stat` every 2s, the busy
    /// share of the ticks since the sample before. A share needs two
    /// samples, so it is taken here for every caller rather than between one
    /// client's calls: a lone `device status` gets the last interval too, and
    /// two clients polling do not shorten each other's. The second sample
    /// comes after `FIRST`, so a `status` right after the agent restarts -
    /// which a `config set` often does - already has a share.
    pub fn watch_cpu(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(2);
        const FIRST: Duration = Duration::from_millis(500);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut before = None;
            let mut wait = FIRST;
            loop {
                let stat = control.paths.proc_stat.clone();
                // naked: a /proc read under blocking()'s within()
                let now = blocking("reading /proc/stat", move || Ok(hardware::cpu_times(&stat)))
                    .await
                    .ok()
                    .flatten();
                *lock(&control.cpu) = before
                    .zip(now)
                    .and_then(|(before, now)| hardware::cpu_percent(before, now));
                before = now;
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = shutdown.changed() => return,
                }
                wait = EVERY;
            }
        });
    }

    async fn apply_time(&self) -> Result<time::Outcome, String> {
        let wanted = self.time_wanted().await?;
        self.time.apply(&self.bus, &wanted).await
    }

    async fn time_wanted(&self) -> Result<time::Wanted, String> {
        let state = self.read_state().await?;
        Ok(self.time_wanted_from(&state.settings))
    }

    pub(super) fn time_wanted_from(&self, settings: &BTreeMap<String, String>) -> time::Wanted {
        time::Wanted::from_env(&state::Effective::new(&self.defaults, settings, &self.log))
    }

    pub(super) async fn time_status(&self) -> Result<TimeStatus, String> {
        let wanted = self.time_wanted().await?;
        Ok(self.time.status(&self.bus, &wanted).await)
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

/// Is this browser.url template the welcome page? The page is `index.html`
/// at the self-test origin, however the URL names it.
fn is_welcome(template: &str, origin: &str) -> bool {
    let origin = origin.trim_end_matches('/');
    template
        .strip_prefix(origin)
        .is_some_and(|rest| matches!(rest, "" | "/" | "/index.html"))
}

/// What goes into `welcome.json`.
struct Welcome<'a> {
    node: &'a str,
    net: &'a protocol::Net,
    /// The hotspot's SSID, while it is up.
    hotspot: Option<&'a str>,
    claimed: bool,
    online: Option<bool>,
}

/// The body of `welcome.json`. IPv4 global addresses only, with the
/// interface each is on: those are what someone reads off the screen to reach
/// the device. The hotspot is `null` unless it is up; `online` is `null`
/// until the first lookup. `setup` - the QR code that joins a phone to the
/// hotspot, and Quick Setup's address - is there only while the hotspot is up
/// and the device is unclaimed, which is when the hotspot is open.
fn welcome_value(welcome: &Welcome) -> serde_json::Value {
    let addresses: Vec<_> = welcome
        .net
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
    // An SSID too long for a QR code cannot happen (32 bytes at most); if
    // the encoder refused anyway, the page falls back to the written line.
    let setup = welcome
        .hotspot
        .filter(|_| !welcome.claimed)
        .and_then(|ssid| crate::qr::svg(&crate::qr::wifi_payload(ssid)).ok())
        .map(|qr| {
            serde_json::json!({
                "qr": qr,
                // The API's port, where the agent serves Webconfig;
                // nginx on port 80 sends a typed http://10.42.0.1/ there too.
                "url": format!(
                    "https://{}:{}/",
                    profiles::HOTSPOT_ADDRESS,
                    protocol::DEFAULT_PORT
                ),
            })
        });
    serde_json::json!({
        "node": welcome.node,
        "addresses": addresses,
        "hotspot": welcome.hotspot.map(|ssid| serde_json::json!({ "ssid": ssid })),
        "claimed": welcome.claimed,
        "online": welcome.online,
        "setup": setup,
    })
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
            proxy: None,
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
        let body = welcome_value(&welcome("lobby", &net, None, false, None));
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
                "online": null,
                "setup": null,
            })
        );
    }

    fn welcome<'a>(
        node: &'a str,
        net: &'a Net,
        hotspot: Option<&'a str>,
        claimed: bool,
        online: Option<bool>,
    ) -> Welcome<'a> {
        Welcome {
            node,
            net,
            hotspot,
            claimed,
            online,
        }
    }

    #[test]
    fn the_welcome_page_names_a_running_hotspot() {
        let net = net(vec![]);
        let body = welcome_value(&welcome(
            "lobby",
            &net,
            Some("tessaro-lobby"),
            true,
            Some(true),
        ));
        assert_eq!(
            body["hotspot"],
            serde_json::json!({ "ssid": "tessaro-lobby" })
        );
        assert_eq!(body["claimed"], true);
        assert_eq!(body["online"], true);
        assert_eq!(body["addresses"], serde_json::json!([]));
        assert_eq!(body["setup"], serde_json::Value::Null, "claimed: no QR");
    }

    #[test]
    fn the_setup_qr_is_there_only_for_the_open_hotspot() {
        let net = net(vec![]);
        let body = welcome_value(&welcome(
            "lobby",
            &net,
            Some("tessaro-lobby"),
            false,
            Some(false),
        ));
        assert_eq!(body["online"], false);
        assert_eq!(body["setup"]["url"], "https://10.42.0.1:7400/");
        let qr = body["setup"]["qr"].as_str().unwrap();
        assert!(qr.starts_with("<svg "), "{qr}");

        let body = welcome_value(&welcome("lobby", &net, None, false, None));
        assert_eq!(body["setup"], serde_json::Value::Null, "no hotspot: no QR");
    }

    /// The API documents the body as `WelcomeInfo`; every field of it
    /// survives the round trip, so the document is what is sent.
    #[test]
    fn the_welcome_body_is_what_the_api_documents() {
        let net = net(vec![interface(
            "eth0",
            "ethernet",
            &[("192.168.1.42", "ipv4", "global")],
        )]);
        for body in [
            welcome_value(&welcome("lobby", &net, None, true, None)),
            welcome_value(&welcome("lobby", &net, Some("t"), false, Some(true))),
        ] {
            let typed: protocol::WelcomeInfo = serde_json::from_value(body.clone()).unwrap();
            assert_eq!(serde_json::to_value(typed).unwrap(), body);
        }
    }

    #[test]
    fn the_welcome_page_is_the_self_test_origin_s_index() {
        let origin = "http://127.0.0.1";
        assert!(is_welcome("http://127.0.0.1/", origin));
        assert!(is_welcome("http://127.0.0.1", origin));
        assert!(is_welcome("http://127.0.0.1/index.html", origin));
        assert!(!is_welcome("http://127.0.0.1/maintenance.html", origin));
        assert!(!is_welcome("http://127.0.0.1:8080/", origin));
        assert!(!is_welcome("https://example.com/", origin));
    }
}
