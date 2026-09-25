//! The main loop: probe the target URL, keep Chromium pointed at it, show the
//! local offline page while it is unreachable, and restart the browser when it
//! stops answering.
//!
//! The state machine is the shell agent's, moved over unchanged. With cog the
//! control surface was write-only, so every decision had to come from an
//! external probe plus bookkeeping. CDP can answer questions that version
//! could not - a real renderer liveness check, the actual URL on screen - but
//! the same conservative rules still apply. In particular `NavState::Unknown`
//! after a browser restart, because a fresh Chromium may be showing anything.

use std::panic::AssertUnwindSafe;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use tokio::sync::watch;

use crate::config::Config;
use crate::log::Log;
use crate::ports::{Cdp, DebugScreen, OfflinePage, ProbeResult, Prober, Units};
use crate::watchdog::{Heartbeat, GRACE, MAX_PLEDGE};

/// What we believe is on screen. "Unknown" is not ignorance for its own sake -
/// it is the only honest answer after the browser restarted under us, and it
/// is what makes the next cycle navigate instead of assuming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavState {
    Unknown,
    Live,
    Offline,
    /// The debug screen, which `browser.debug.enable` puts up instead of the site.
    Debug,
}

/// How often the debug screen is re-rendered, so an address that moves is on
/// screen within seconds. Only a changed page is navigated to.
const DEBUG_REFRESH: i64 = 5;

pub struct Agent<'a> {
    config: &'a Config,
    log: &'a Log,
    probe: &'a dyn Prober,
    cdp: &'a dyn Cdp,
    units: &'a dyn Units,
    offline: &'a dyn OfflinePage,
    debug_screen: &'a dyn DebugScreen,

    /// Level-triggered, so a signal that arrived while a cycle was running is
    /// still seen by the nap that follows it.
    shutdown: watch::Receiver<bool>,
    /// The watchdog pledge. Only the waiting between cycles is pledged here;
    /// every external call pledges itself through its adapter.
    heartbeat: Heartbeat,

    fails: i64,
    ping_fails: i64,
    restart_done: bool,
    nav_state: NavState,
    last_nav: i64,
    last_restart: i64,
    last_main_pid: u32,
    /// The CDP session generation seen last cycle; 0 before the first.
    last_generation: u64,
    /// Has the browser answered even once since this process started?
    seen_alive: bool,
    /// The origin the browser is allowed to be on. Starts unset, meaning
    /// "whatever `KIOSK_URL` says", and becomes wherever our own navigation
    /// actually lands.
    accepted_origin: Option<String>,
    /// We navigated last cycle, so this cycle's URL is the landing point.
    awaiting_landing: bool,
    /// Someone else is in DevTools, and the tab is theirs until they leave.
    held: bool,
}

impl<'a> Agent<'a> {
    // Every argument is one seam of the state machine; bundling them into a
    // struct would only move the list somewhere else.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &'a Config,
        log: &'a Log,
        probe: &'a dyn Prober,
        cdp: &'a dyn Cdp,
        units: &'a dyn Units,
        offline: &'a dyn OfflinePage,
        debug_screen: &'a dyn DebugScreen,
        shutdown: watch::Receiver<bool>,
        heartbeat: Heartbeat,
    ) -> Self {
        Self {
            config,
            log,
            probe,
            cdp,
            units,
            offline,
            debug_screen,
            shutdown,
            heartbeat,
            fails: 0,
            ping_fails: 0,
            restart_done: false,
            nav_state: NavState::Unknown,
            last_nav: 0,
            last_restart: 0,
            last_main_pid: 0,
            last_generation: 0,
            seen_alive: false,
            accepted_origin: None,
            awaiting_landing: false,
            held: false,
        }
    }

    pub async fn run(&mut self) {
        if !self.config.agent_enable {
            // Parked rather than exiting: the unit still shows as running and
            // the reason is in the journal. Useful while debugging a page.
            // The nap pledges, so a parked agent is not a stalled one.
            self.log.info("KIOSK_AGENT_ENABLE is off; idling");
            while self.running() {
                self.nap(60).await;
            }
            return;
        }

        if self.config.debug_screen {
            self.log.info(format!(
                "debug screen on: showing browser.debug.template instead of {}",
                self.config.kiosk_url
            ));
        } else if self.config.probe_enabled() {
            self.offline.stage().await;
            self.log.info(format!(
                "watching {} (probe every {}s, refresh every {}s)",
                self.config.kiosk_url, self.config.probe_interval, self.config.refresh_interval
            ));
        } else {
            self.log.info(format!(
                "watching {} (probe disabled, refresh-only)",
                self.config.kiosk_url
            ));
        }

        while self.running() {
            self.cycle(now()).await;

            let interval = if self.config.debug_screen {
                self.config.probe_interval.min(DEBUG_REFRESH)
            } else if self.fails > 0 {
                self.config.probe_interval_fail
            } else {
                self.config.probe_interval
            };
            self.nap(interval).await;
        }

        self.log.info("stopping");
    }

    /// One pass of the loop. Public, and taking an explicit clock, so tests
    /// can drive the whole state machine without sleeping.
    pub async fn cycle(&mut self, now: i64) {
        // A bug in one pass must not take the service down with it: systemd
        // would restart us, but the backoff and "one restart per outage"
        // bookkeeping lives in memory and would be lost, which is how a
        // crash-loop turns into the browser being restarted every ten seconds.
        let outcome = AssertUnwindSafe(self.cycle_inner(now)).catch_unwind().await;

        if let Err(payload) = outcome {
            self.log
                .info(format!("cycle failed: {}", panic_message(&payload)));
        }
    }

    async fn cycle_inner(&mut self, now: i64) {
        let config = self.config;

        // A technician in DevTools owns the tab. A breakpoint stops the
        // renderer answering `Runtime.evaluate`, which would get the browser
        // restarted under them, and a page they open is not drift. So no
        // liveness check, probe, navigation or restart until they disconnect.
        if self.cdp.inspected().await {
            if !self.held {
                self.held = true;
                self.log.info(
                    "a DevTools client is connected; leaving the browser alone until it disconnects",
                );
            }
            return;
        }
        if self.held {
            self.held = false;
            // What they left on screen is not ours to vouch for, and a
            // debugger pause is not a silent browser.
            self.nav_state = NavState::Unknown;
            self.ping_fails = 0;
            self.log
                .info("the DevTools client disconnected; watching the browser again");
        }

        // Liveness first, so navigation failures below are attributed
        // correctly.
        if self.cdp.alive().await {
            // Worth a line: "not answering" is logged below, so without this
            // the journal shows a browser going silent and never coming back.
            if self.ping_fails > 0 && self.seen_alive {
                self.log.info(format!(
                    "chromium is answering again after {} failed checks",
                    self.ping_fails
                ));
            }
            self.ping_fails = 0;
            self.seen_alive = true;
        } else {
            self.ping_fails += 1;
            if self.ping_fails == 1 {
                self.report_cdp_failure(format!("chromium is not answering on {}", config.cdp_url));
            }
        }

        // A browser that restarted under us is showing whatever its ExecStart
        // URL produced, which we cannot be sure of, so stop claiming to know.
        //
        // Independent witnesses. A new page generation from the DevTools
        // session means a new browser, a new page target, or a crashed tab -
        // never merely a reconnect to the same live page. The
        // MainPID check catches a restart over the bus - but only when
        // there IS a bus: in `mise run agent:integration`, or on a device
        // whose systemd1 is unreachable, main_pid() is 0 and a browser that
        // died and came back used to be invisible. One `if`, so both firing
        // in the same cycle log once.
        let generation = self.cdp.generation();
        let session_is_new =
            generation > 0 && self.last_generation > 0 && generation != self.last_generation;
        if generation > 0 {
            self.last_generation = generation;
        }

        let main_pid = self.units.main_pid().await;
        let pid_is_new = main_pid > 0 && self.last_main_pid > 0 && main_pid != self.last_main_pid;
        if main_pid > 0 {
            self.last_main_pid = main_pid;
        }

        if pid_is_new {
            self.log.info(format!(
                "chromium restarted (pid {main_pid}); will re-navigate"
            ));
            self.nav_state = NavState::Unknown;
        } else if session_is_new {
            self.log
                .info("chromium is showing a new page (restarted or crashed); will re-navigate");
            self.nav_state = NavState::Unknown;
        }

        if config.debug_screen {
            // The screen is the debug text whatever the site is doing: no
            // probe, no offline page, no origin to enforce. What stays is the
            // browser escalation below - a wedged browser shows no text either.
            self.show_debug(now).await;
        } else {
            self.follow_site(now).await;
        }

        // Escalation, the other trigger: chromium itself stopped answering.
        // Independent of the probe, because this one is about the browser and
        // not the network, and so deliberately not gated by restart_done.
        if self.ping_fails >= config.ping_fails {
            self.restart(
                &format!("no CDP reply after {} attempts", self.ping_fails),
                now,
            )
            .await;
        }
    }

    /// Probe the site and put it, or the offline page, on screen.
    async fn follow_site(&mut self, now: i64) {
        let config = self.config;
        let result = if config.probe_enabled() {
            self.probe.call(config.probe_target()).await
        } else {
            // Refresh-only: there is nothing meaningful to probe, so the site
            // counts as up and only the refresh timer drives navigation.
            ProbeResult::ok(200)
        };

        if result.ok {
            if self.fails > 0 {
                self.log.info(format!(
                    "{} reachable again after {} failed probes",
                    config.probe_target(),
                    self.fails
                ));
                self.fails = 0;
                self.restart_done = false;
            }

            // Navigate on recovery, when the browser has wandered off our
            // site, or when the refresh timer expires - never on every probe.
            // Unknown counts as a reason to navigate: after a browser restart
            // it may be sitting on its own error page.
            let drifted = self.drifted_origin().await;
            if let Some(url) = &drifted {
                self.log.info(format!(
                    "chromium is showing {url}; returning to the kiosk URL"
                ));
            }

            if self.nav_state != NavState::Live
                || drifted.is_some()
                || (config.refresh_interval > 0 && now - self.last_nav >= config.refresh_interval)
            {
                self.go_live(now).await;
            }
        } else {
            self.fails += 1;
            if self.fails == config.fail_threshold {
                self.log.info(format!(
                    "{} unreachable: {}",
                    config.probe_target(),
                    result.reason
                ));
            }

            if self.fails >= config.fail_threshold
                && (self.nav_state != NavState::Offline
                    || (config.offline_refresh > 0
                        && now - self.last_nav >= config.offline_refresh))
            {
                self.go_offline(now).await;
            }

            // Escalation: down long enough that a wedged web process is worth
            // ruling out, and the screen already shows our page so the restart
            // costs nothing visible. Once per outage - restarting against a
            // dead network helps nobody.
            if !self.restart_done
                && config.restart_after > 0
                && self.fails >= config.restart_after
                && self
                    .restart(
                        &format!("no successful probe in {} attempts", self.fails),
                        now,
                    )
                    .await
            {
                self.restart_done = true;
            }
        }
    }

    /// Put the debug screen up, and bring it up to date when its text moved.
    async fn show_debug(&mut self, now: i64) {
        let Some(staged) = self.debug_screen.stage().await else {
            return;
        };
        if self.nav_state == NavState::Debug && !staged.changed {
            return;
        }

        match self.cdp.navigate(&staged.uri).await {
            Ok(()) => {
                // Info once, when it goes up; its updates are only news to
                // someone debugging the agent itself.
                let message = format!("navigated to the debug screen ({})", staged.uri);
                if self.nav_state == NavState::Debug {
                    self.log.debug(message);
                } else {
                    self.log.info(message);
                }
                self.nav_state = NavState::Debug;
                self.last_nav = now;
            }
            Err(err) => {
                self.ping_fails += 1;
                self.report_cdp_failure(format!(
                    "could not tell chromium to open the debug screen ({err})"
                ));
            }
        }
    }

    /// The URL on screen when it is on a different site from the kiosk.
    ///
    /// Compared by origin, not by whole URL: the site's own sub-pages, query
    /// strings and in-page routing are legitimate, and snapping back on those
    /// would fight the site. Only leaving the site counts.
    ///
    /// The origin we compare against is not `KIOSK_URL`'s but **wherever our
    /// own navigation last landed**. That is what stops the obvious disaster
    /// here: `https://example.com` redirecting to `https://www.example.com` is
    /// a *different origin*, so comparing against the configured URL made the
    /// agent reload the page on every single cycle, forever. Accepting the
    /// landing point costs us nothing - a redirect the site itself performs is
    /// the site - and it cannot loop, because whatever we navigate to is what
    /// we then accept.
    ///
    /// A page someone else navigated to is still caught: it was not reached by
    /// our navigation, so it is measured against the accepted origin and
    /// fails. The one window is a foreign navigation landing in the same cycle
    /// as our periodic refresh, which would be adopted - and then corrected at
    /// the next refresh, which is exactly the behaviour before any of this
    /// existed.
    ///
    /// `None` whenever we cannot be sure: enforcement off, a kiosk URL with no
    /// origin to compare against (`data:`, `file:`), a browser that will not
    /// say, or a page we are not currently claiming to own. "Cannot tell" must
    /// never become a navigation.
    async fn drifted_origin(&mut self) -> Option<String> {
        if !self.config.enforce_origin || self.nav_state != NavState::Live {
            return None;
        }

        let want = match &self.accepted_origin {
            Some(origin) => origin.clone(),
            None => crate::url::origin(&self.config.kiosk_url)?.to_string(),
        };

        let current = self.cdp.current_url().await?;
        let current_origin = crate::url::origin(&current).map(str::to_string);

        // The cycle after we navigated: whatever is on screen is where that
        // navigation ended up, redirects included. Adopt it and never call it
        // drift.
        if self.awaiting_landing {
            self.awaiting_landing = false;

            if let Some(origin) = current_origin {
                if origin != want {
                    self.log.info(format!(
                        "{} redirected to {origin}; treating that as the kiosk origin",
                        self.config.kiosk_url
                    ));
                }
                self.accepted_origin = Some(origin);
            }

            return None;
        }

        match current_origin {
            Some(origin) if origin == want => None,
            // Includes about:blank and anything else without an origin, which
            // is drift too - the browser is not showing our site.
            _ => Some(current),
        }
    }

    /// A CDP failure is only news once the browser has proved it can answer.
    ///
    /// `tessaro-kiosk.service` is `Type=exec`, so systemd calls it started the
    /// moment `/usr/bin/chromium` is exec'd - seconds before Chromium opens
    /// its DevTools port. Our `After=` on it therefore guarantees nothing, and
    /// the first cycle after every boot finds the port closed. Logging that at
    /// info put lines that read like faults into the journal of every
    /// device on every boot, which is how a technician learns to skim past the
    /// journal. Nothing is hidden: if the browser never comes up at all, the
    /// escalation still logs the restart at info, which is the line that
    /// actually needs reading.
    fn report_cdp_failure(&self, message: String) {
        if self.seen_alive {
            self.log.info(message);
        } else {
            self.log.debug(message);
        }
    }

    async fn go_live(&mut self, now: i64) {
        match self.cdp.navigate(&self.config.kiosk_url).await {
            Ok(()) => {
                self.nav_state = NavState::Live;
                self.last_nav = now;
                // Where this actually lands is next cycle's business - the
                // site may redirect us somewhere else entirely.
                self.awaiting_landing = true;
                // Info, not debug: this is the one line that says what is on
                // screen, and at the default refresh it costs one entry per
                // ten minutes.
                self.log
                    .info(format!("navigated to {}", self.config.kiosk_url));
            }
            Err(err) => {
                // Counts towards the CDP escalation: a browser that will not
                // take a navigation is as broken as one that will not answer.
                self.ping_fails += 1;
                self.report_cdp_failure(format!(
                    "could not tell chromium to open the kiosk URL ({err})"
                ));
            }
        }
    }

    async fn go_offline(&mut self, now: i64) {
        if self.config.offline_url == "none" {
            return;
        }

        // Staged on every use, so dropping a new file into /data/kiosk takes
        // effect without restarting anything.
        let uri = if self.config.offline_url.is_empty() {
            match self.offline.stage().await {
                Some(uri) => uri,
                None => return,
            }
        } else {
            self.config.offline_url.clone()
        };

        match self.cdp.navigate(&uri).await {
            Ok(()) => {
                self.nav_state = NavState::Offline;
                self.last_nav = now;
                self.log
                    .info(format!("navigated to the offline page ({uri})"));
            }
            Err(err) => {
                self.ping_fails += 1;
                self.report_cdp_failure(format!(
                    "could not tell chromium to open the offline page ({err})"
                ));
            }
        }
    }

    async fn restart(&mut self, reason: &str, now: i64) -> bool {
        let unit = &self.config.unit;

        if now - self.last_restart < self.config.restart_backoff {
            self.log
                .debug(format!("not restarting {unit} ({reason}): within backoff"));
            return false;
        }

        // If an operator stopped the unit by hand to look at something, do not
        // fight them. "failed" is still ours to fix - that is systemd giving
        // up, not a person deciding.
        let state = self.units.active_state().await;
        if !matches!(state.as_str(), "active" | "activating" | "failed") {
            self.log
                .info(format!("{unit} is '{state}'; leaving it alone"));
            return false;
        }

        // Set before the call, so a restart that raises still consumes the
        // backoff window rather than being retried every cycle.
        self.last_restart = now;
        self.log.info(format!("restarting {unit}: {reason}"));

        match self.units.restart().await {
            Ok(()) => {
                self.ping_fails = 0;
                // Whatever is on screen now, we no longer know what it is.
                self.nav_state = NavState::Unknown;
                true
            }
            Err(err) => {
                self.log.info(format!("restart of {unit} failed: {err}"));
                false
            }
        }
    }

    fn running(&self) -> bool {
        !*self.shutdown.borrow()
    }

    /// Sleep until the interval is up or a signal arrives, whichever is first.
    /// The wake-up is exact, so a SIGTERM during a ten-minute refresh wait is
    /// honoured immediately rather than after a nap slice.
    ///
    /// The wait is pledged to the watchdog in chunks no longer than the pledge
    /// ceiling, which is what makes idle count as alive: neither
    /// `KIOSK_PROBE_INTERVAL` nor the `KIOSK_AGENT_ENABLE=0` park needs any
    /// headroom in `WatchdogSec=`.
    async fn nap(&mut self, seconds: i64) {
        let mut left = Duration::from_secs(seconds.max(0) as u64);

        while !left.is_zero() && self.running() {
            let slice = left.min(MAX_PLEDGE);
            self.heartbeat
                .pledge("waiting for the next cycle", slice + GRACE);

            tokio::select! {
                _ = tokio::time::sleep(slice) => left -= slice,
                changed = self.shutdown.changed() => {
                    // A dropped sender resolves changed() immediately, every
                    // time. Sleep the slice out instead of spinning on it.
                    if changed.is_err() {
                        tokio::time::sleep(slice).await;
                        left -= slice;
                    }
                }
            }
        }
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::config_with;
    use crate::error::{Error, Result};
    use async_trait::async_trait;
    use std::cell::{Cell, RefCell};

    const OFFLINE_URI: &str = "file:///run/tessaro-kiosk/index.html";

    struct FakeCdp {
        alive: Cell<bool>,
        panics: Cell<bool>,
        navigations: RefCell<Vec<String>>,
        navigate_fails: Cell<bool>,
        /// What the browser claims to be showing. `None` means "will not say",
        /// which is the case a real browser hits mid-navigation.
        current_url: RefCell<Option<String>>,
        /// Where the site sends every navigation, as a real one does when it
        /// redirects apex to www.
        redirect_to: RefCell<Option<String>>,
        /// The DevTools session generation; bumped by `reconnected`.
        generation: Cell<u64>,
        /// A technician is connected to DevTools.
        inspected: Cell<bool>,
    }

    impl Default for FakeCdp {
        fn default() -> Self {
            Self {
                alive: Cell::new(true),
                panics: Cell::new(false),
                navigations: RefCell::new(Vec::new()),
                navigate_fails: Cell::new(false),
                current_url: RefCell::new(Some("http://kiosk.test/".to_string())),
                redirect_to: RefCell::new(None),
                generation: Cell::new(1),
                inspected: Cell::new(false),
            }
        }
    }

    impl FakeCdp {
        /// The browser went away and came back on a new page.
        fn reconnected(&self) {
            self.generation.set(self.generation.get() + 1);
        }
    }

    #[async_trait(?Send)]
    impl Cdp for FakeCdp {
        async fn alive(&self) -> bool {
            assert!(!self.panics.get(), "the browser exploded");
            self.alive.get()
        }

        async fn current_url(&self) -> Option<String> {
            self.current_url.borrow().clone()
        }

        fn generation(&self) -> u64 {
            self.generation.get()
        }

        async fn inspected(&self) -> bool {
            self.inspected.get()
        }

        async fn navigate(&self, url: &str) -> Result<()> {
            if self.navigate_fails.get() {
                return Err(Error::Cdp("navigate refused".to_string()));
            }
            self.navigations.borrow_mut().push(url.to_string());
            // A real browser ends up wherever the site sent it, and the next
            // cycle's drift check reads that back.
            let landed = self
                .redirect_to
                .borrow()
                .clone()
                .unwrap_or_else(|| url.to_string());
            *self.current_url.borrow_mut() = Some(landed);
            Ok(())
        }
    }

    struct FakeUnits {
        active_state: RefCell<String>,
        main_pid: Cell<u32>,
        restarts: Cell<usize>,
    }

    impl Default for FakeUnits {
        fn default() -> Self {
            Self {
                active_state: RefCell::new("active".to_string()),
                main_pid: Cell::new(42),
                restarts: Cell::new(0),
            }
        }
    }

    #[async_trait(?Send)]
    impl Units for FakeUnits {
        async fn active_state(&self) -> String {
            self.active_state.borrow().clone()
        }

        async fn main_pid(&self) -> u32 {
            self.main_pid.get()
        }

        async fn restart(&self) -> Result<()> {
            self.restarts.set(self.restarts.get() + 1);
            Ok(())
        }
    }

    struct FakeProbe {
        result: RefCell<ProbeResult>,
        calls: Cell<usize>,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            Self {
                result: RefCell::new(ProbeResult::ok(200)),
                calls: Cell::new(0),
            }
        }
    }

    impl FakeProbe {
        fn fail(&self) {
            *self.result.borrow_mut() = ProbeResult::failed("connection refused");
        }

        fn succeed(&self) {
            *self.result.borrow_mut() = ProbeResult::ok(200);
        }
    }

    #[async_trait(?Send)]
    impl Prober for FakeProbe {
        async fn call(&self, _url: &str) -> ProbeResult {
            self.calls.set(self.calls.get() + 1);
            self.result.borrow().clone()
        }
    }

    const DEBUG_URI: &str = "file:///run/tessaro-kiosk/debug.html";

    /// A debug screen whose text moves when the test says so.
    #[derive(Default)]
    struct FakeDebug {
        changed: Cell<bool>,
    }

    #[async_trait(?Send)]
    impl DebugScreen for FakeDebug {
        async fn stage(&self) -> Option<crate::ports::Staged> {
            Some(crate::ports::Staged {
                uri: DEBUG_URI.to_string(),
                changed: self.changed.replace(false),
            })
        }
    }

    #[derive(Default)]
    struct FakeOffline {
        staged: Cell<usize>,
    }

    #[async_trait(?Send)]
    impl OfflinePage for FakeOffline {
        async fn stage(&self) -> Option<String> {
            self.staged.set(self.staged.get() + 1);
            Some(OFFLINE_URI.to_string())
        }
    }

    /// Everything a test needs, owned in one place so the borrows the Agent
    /// takes all outlive it.
    struct World {
        config: Config,
        log: Log,
        cdp: FakeCdp,
        units: FakeUnits,
        probe: FakeProbe,
        offline: FakeOffline,
        debug: FakeDebug,
        /// Kept alive deliberately: a dropped sender makes every
        /// `Receiver::changed()` resolve at once. Nothing cycle-driven naps
        /// today, but the day a test does, this is why it still works.
        stop: watch::Sender<bool>,
    }

    impl World {
        fn new(overrides: &[(&str, &str)]) -> Self {
            Self {
                config: config_with(overrides),
                // Not a debug log: these cases assert on what actually
                // reaches the journal on a device, where KIOSK_DEBUG is 0.
                log: Log::buffered(false),
                cdp: FakeCdp::default(),
                units: FakeUnits::default(),
                probe: FakeProbe::default(),
                offline: FakeOffline::default(),
                debug: FakeDebug::default(),
                stop: watch::channel(false).0,
            }
        }

        fn agent(&self) -> Agent<'_> {
            Agent::new(
                &self.config,
                &self.log,
                &self.probe,
                &self.cdp,
                &self.units,
                &self.offline,
                &self.debug,
                self.stop.subscribe(),
                Heartbeat::detached(),
            )
        }

        fn navigations(&self) -> Vec<String> {
            self.cdp.navigations.borrow().clone()
        }
    }

    #[tokio::test]
    async fn the_debug_screen_replaces_the_site_and_is_never_probed() {
        let world = World::new(&[("KIOSK_DEBUG_SCREEN", "1")]);
        world.probe.fail();
        let mut agent = world.agent();

        for now in [1000, 1005, 1010, 1100] {
            agent.cycle(now).await;
        }

        assert_eq!(world.navigations(), vec![DEBUG_URI]);
        assert_eq!(world.probe.calls.get(), 0);
        assert_eq!(world.offline.staged.get(), 0);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("navigated to the debug screen")));
    }

    #[tokio::test]
    async fn the_debug_screen_is_reloaded_only_when_its_text_moved() {
        let world = World::new(&[("KIOSK_DEBUG_SCREEN", "1")]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        agent.cycle(1005).await;
        world.debug.changed.set(true);
        agent.cycle(1010).await;
        agent.cycle(1015).await;

        assert_eq!(world.navigations(), vec![DEBUG_URI, DEBUG_URI]);
    }

    #[tokio::test]
    async fn a_silent_browser_is_still_restarted_under_the_debug_screen() {
        let world = World::new(&[("KIOSK_DEBUG_SCREEN", "1")]);
        world.cdp.alive.set(false);
        let mut agent = world.agent();

        for now in [1000, 1005, 1010, 1015] {
            agent.cycle(now).await;
        }

        assert_eq!(world.units.restarts.get(), 1);
    }

    #[tokio::test]
    async fn a_browser_in_devtools_is_left_alone() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        agent.cycle(1000).await;

        // Paused at a breakpoint, on a page of its own, with the site down.
        world.cdp.inspected.set(true);
        world.cdp.alive.set(false);
        *world.cdp.current_url.borrow_mut() = Some("http://elsewhere.test/".to_string());
        world.probe.fail();
        let probes = world.probe.calls.get();
        for now in [1010, 1020, 1030, 1040, 1700] {
            agent.cycle(now).await;
        }

        assert_eq!(world.units.restarts.get(), 0);
        assert_eq!(world.navigations(), vec!["http://kiosk.test/"]);
        assert_eq!(world.probe.calls.get(), probes);
        let held = world
            .log
            .lines()
            .iter()
            .filter(|line| line.contains("a DevTools client is connected"))
            .count();
        assert_eq!(held, 1);
    }

    #[tokio::test]
    async fn the_kiosk_page_comes_back_when_devtools_disconnects() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        agent.cycle(1000).await;
        world.cdp.inspected.set(true);
        world.cdp.alive.set(false);
        agent.cycle(1010).await;
        agent.cycle(1020).await;

        world.cdp.inspected.set(false);
        world.cdp.alive.set(true);
        agent.cycle(1030).await;

        assert_eq!(
            world.navigations(),
            vec!["http://kiosk.test/", "http://kiosk.test/"]
        );
        assert_eq!(world.units.restarts.get(), 0);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("the DevTools client disconnected")));
    }

    #[tokio::test]
    async fn the_first_cycle_navigates_to_the_kiosk_url() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;

        assert_eq!(world.navigations(), vec!["http://kiosk.test/"]);
    }

    #[tokio::test]
    async fn a_healthy_kiosk_is_not_re_navigated_every_probe() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        agent.cycle(1030).await;

        assert_eq!(world.navigations().len(), 1);
    }

    #[tokio::test]
    async fn the_refresh_interval_re_navigates() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        agent.cycle(1600).await;

        assert_eq!(world.navigations().len(), 2);
    }

    #[tokio::test]
    async fn the_offline_page_waits_for_the_fail_threshold() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000).await;
        assert!(world.navigations().is_empty());

        agent.cycle(1010).await;
        assert_eq!(world.navigations(), vec![OFFLINE_URI]);
    }

    #[tokio::test]
    async fn the_offline_page_is_not_re_navigated_before_its_own_interval() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000).await;
        agent.cycle(1010).await;
        agent.cycle(1020).await;

        assert_eq!(world.navigations().len(), 1);
    }

    #[tokio::test]
    async fn recovery_navigates_back_to_the_kiosk_url() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000).await;
        agent.cycle(1010).await;
        world.probe.succeed();
        agent.cycle(1020).await;

        assert_eq!(world.navigations(), vec![OFFLINE_URI, "http://kiosk.test/"]);
    }

    #[tokio::test]
    async fn a_silent_browser_is_restarted_and_then_re_navigated() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        agent.cycle(1000).await;
        agent.cycle(1010).await;
        agent.cycle(1020).await;

        assert_eq!(world.units.restarts.get(), 1);

        // The restart left nav_state unknown, so the next healthy cycle has to
        // navigate again rather than assume the page survived.
        agent.cycle(1030).await;
        assert_eq!(world.navigations().len(), 2);
    }

    #[tokio::test]
    async fn restarts_respect_the_backoff() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        for now in [1000, 1010, 1020] {
            agent.cycle(now).await;
        }
        assert_eq!(world.units.restarts.get(), 1);

        for now in (1030..=1080).step_by(10) {
            agent.cycle(now).await;
        }
        assert_eq!(world.units.restarts.get(), 1);

        agent.cycle(1020 + 301).await;
        assert_eq!(world.units.restarts.get(), 2);
    }

    #[tokio::test]
    async fn the_network_escalation_fires_once_per_outage() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        let mut clock = 1000;
        for _ in 0..40 {
            agent.cycle(clock).await;
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 1);

        for _ in 0..10 {
            agent.cycle(clock).await;
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 1);

        world.probe.succeed();
        agent.cycle(clock).await;
        clock += 10;
        world.probe.fail();
        for _ in 0..40 {
            agent.cycle(clock).await;
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 2);
    }

    #[tokio::test]
    async fn an_operator_stopped_unit_is_left_alone() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        *world.units.active_state.borrow_mut() = "inactive".to_string();

        for now in [1000, 1010, 1020, 1030] {
            agent.cycle(now).await;
        }

        assert_eq!(world.units.restarts.get(), 0);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("leaving it alone")));
    }

    #[tokio::test]
    async fn a_new_main_pid_forces_a_re_navigation() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        world.units.main_pid.set(77);
        agent.cycle(1010).await;

        assert_eq!(world.navigations().len(), 2);
    }

    #[tokio::test]
    async fn a_new_cdp_session_forces_a_re_navigation_even_without_a_bus() {
        // No bus, as in agent:integration: main_pid() is 0 throughout, so the
        // session generation is the only thing that can see this restart.
        let world = World::new(&[]);
        world.units.main_pid.set(0);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        world.cdp.reconnected();
        agent.cycle(1010).await;

        assert_eq!(world.navigations().len(), 2);
        assert!(world.log.lines().contains(
            &"chromium is showing a new page (restarted or crashed); will re-navigate".to_string()
        ));
    }

    #[tokio::test]
    async fn a_new_session_and_a_new_pid_in_one_cycle_log_once() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        world.units.main_pid.set(77);
        world.cdp.reconnected();
        agent.cycle(1010).await;

        let restarted: Vec<_> = world
            .log
            .lines()
            .into_iter()
            .filter(|line| line.ends_with("will re-navigate"))
            .collect();
        assert_eq!(
            restarted,
            vec!["chromium restarted (pid 77); will re-navigate"]
        );
        assert_eq!(world.navigations().len(), 2);
    }

    #[tokio::test]
    async fn a_non_http_target_runs_refresh_only() {
        let world = World::new(&[
            ("KIOSK_PROBE_URL", ""),
            ("KIOSK_URL", "data:text/html,<h1>hi</h1>"),
        ]);
        let mut agent = world.agent();

        agent.cycle(1000).await;

        assert_eq!(world.navigations(), vec!["data:text/html,<h1>hi</h1>"]);
        assert_eq!(world.offline.staged.get(), 0);
    }

    #[tokio::test]
    async fn a_failed_navigation_counts_towards_the_cdp_escalation() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        world.cdp.navigate_fails.set(true);

        agent.cycle(1000).await;
        agent.cycle(1010).await;

        // Two cycles, not the three a silent browser alone would need: each
        // cycle contributes both an unanswered probe and a refused
        // navigation, and a browser that will not take a navigation is as
        // broken as one that will not answer at all.
        assert!(world.navigations().is_empty());
        assert_eq!(world.units.restarts.get(), 1);
    }

    #[tokio::test]
    async fn wandering_off_the_site_snaps_back_and_says_so() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        // Second cycle settles where that navigation landed. Only after that
        // is a change attributable to somebody else - see drifted_origin for
        // the one window this leaves.
        agent.cycle(1005).await;

        *world.cdp.current_url.borrow_mut() = Some("https://youtube.com/watch?v=x".to_string());
        agent.cycle(1010).await;

        assert_eq!(
            world.navigations(),
            vec!["http://kiosk.test/", "http://kiosk.test/"]
        );
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is showing https://youtube.com/watch?v=x")));
    }

    #[tokio::test]
    async fn a_cross_origin_redirect_does_not_loop() {
        // Regression, found on a device: https://freevision.sk redirects to
        // https://www.freevision.sk, a different origin, and comparing against
        // the configured URL made the agent reload the page every cycle
        // forever - a kiosk refreshing itself every 30 seconds.
        let world = World::new(&[("KIOSK_URL", "https://kiosk.test/")]);
        let mut agent = world.agent();

        // The site sends every visit to www.
        *world.cdp.redirect_to.borrow_mut() = Some("https://www.kiosk.test/".to_string());

        for now in [1000, 1005, 1010, 1015, 1020, 1025] {
            agent.cycle(now).await;
        }

        assert_eq!(
            world.navigations().len(),
            1,
            "should have navigated once and then accepted where it landed, got {:?}",
            world.navigations()
        );
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("redirected to https://www.kiosk.test")));
    }

    #[tokio::test]
    async fn drift_is_still_caught_after_a_redirect_was_accepted() {
        let world = World::new(&[("KIOSK_URL", "https://kiosk.test/")]);
        let mut agent = world.agent();
        *world.cdp.redirect_to.borrow_mut() = Some("https://www.kiosk.test/".to_string());

        agent.cycle(1000).await;
        agent.cycle(1005).await;
        assert_eq!(world.navigations().len(), 1);

        // Someone follows a link off the site.
        *world.cdp.current_url.borrow_mut() = Some("https://elsewhere.test/".to_string());
        agent.cycle(1010).await;

        assert_eq!(world.navigations().len(), 2);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is showing https://elsewhere.test/")));
    }

    #[tokio::test]
    async fn the_sites_own_pages_are_left_alone() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        // Same origin: a sub-page, a query string, a fragment. All legitimate.
        for url in [
            "http://kiosk.test/about",
            "http://kiosk.test/?utm_source=x",
            "http://kiosk.test/a/b#c",
        ] {
            *world.cdp.current_url.borrow_mut() = Some(url.to_string());
            agent.cycle(1005).await;
        }

        assert_eq!(world.navigations().len(), 1, "should not have re-navigated");
    }

    #[tokio::test]
    async fn a_browser_that_will_not_say_where_it_is_is_left_alone() {
        // "Cannot tell" must never be mistaken for "has drifted", or a browser
        // mid-navigation would be yanked back on every cycle.
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        *world.cdp.current_url.borrow_mut() = None;
        agent.cycle(1005).await;

        assert_eq!(world.navigations().len(), 1);
    }

    #[tokio::test]
    async fn enforcement_can_be_turned_off() {
        let world = World::new(&[("KIOSK_ENFORCE_ORIGIN", "0")]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        *world.cdp.current_url.borrow_mut() = Some("https://elsewhere.test/".to_string());
        agent.cycle(1005).await;

        assert_eq!(world.navigations().len(), 1);
    }

    #[tokio::test]
    async fn drift_is_not_chased_while_the_offline_page_is_up() {
        // The offline page is a file:// URL, which has no origin at all. It is
        // there on purpose and must not be treated as the browser wandering.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000).await;
        agent.cycle(1010).await;
        assert_eq!(world.navigations(), vec![OFFLINE_URI]);

        agent.cycle(1020).await;
        assert_eq!(world.navigations().len(), 1);
    }

    #[tokio::test]
    async fn a_recovered_browser_is_reported() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000).await;
        world.cdp.alive.set(false);
        agent.cycle(1005).await;
        world.cdp.alive.set(true);
        agent.cycle(1010).await;

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is answering again after 1 failed checks")));
    }

    #[tokio::test]
    async fn the_startup_race_is_not_reported_as_a_fault() {
        // Every boot starts here: systemd has exec'd Chromium, so the unit
        // counts as started, but the DevTools port is not open yet.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        world.cdp.navigate_fails.set(true);

        agent.cycle(1000).await;
        agent.cycle(1005).await;

        // The escalation may well fire - that part is deliberate and tested
        // below. What must not appear is the pair of lines that read like
        // faults but only describe a browser that has not finished starting.
        let noise: Vec<_> = world
            .log
            .lines()
            .into_iter()
            .filter(|line| {
                line.contains("chromium is not answering")
                    || line.contains("could not tell chromium")
            })
            .collect();

        assert!(
            noise.is_empty(),
            "startup race should not reach the journal, got {noise:?}"
        );
    }

    #[tokio::test]
    async fn a_browser_that_never_comes_up_still_reports_the_restart() {
        // The quiet start must not swallow a browser that is genuinely dead:
        // the escalation is the line worth reading, and it stays at info.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        for now in [1000, 1005, 1010] {
            agent.cycle(now).await;
        }

        assert_eq!(world.units.restarts.get(), 1);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.starts_with("restarting tessaro-kiosk.service:")));
    }

    #[tokio::test]
    async fn a_browser_that_dies_after_working_is_reported() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        // One healthy cycle proves the browser can answer...
        agent.cycle(1000).await;
        assert!(!world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("not answering")));

        // ...so from here on, silence would be hiding a real fault.
        world.cdp.alive.set(false);
        agent.cycle(1005).await;

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is not answering")));
    }

    #[tokio::test]
    async fn a_cycle_survives_a_dependency_blowing_up() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.panics.set(true);

        agent.cycle(1000).await;

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.starts_with("cycle failed:")));
    }
}
