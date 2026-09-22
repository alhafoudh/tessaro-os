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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::log::Log;
use crate::ports::{Cdp, OfflinePage, ProbeResult, Prober, Units};

/// What we believe is on screen. "Unknown" is not ignorance for its own sake -
/// it is the only honest answer after the browser restarted under us, and it
/// is what makes the next cycle navigate instead of assuming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavState {
    Unknown,
    Live,
    Offline,
}

/// Sleep in slices this long, so a SIGTERM is noticed well inside systemd's
/// stop timeout instead of sitting through a ten-minute refresh interval and
/// then taking a SIGKILL.
const NAP_SLICE: i64 = 2;

pub struct Agent<'a> {
    config: &'a Config,
    log: &'a Log,
    probe: &'a dyn Prober,
    cdp: &'a dyn Cdp,
    units: &'a dyn Units,
    offline: &'a dyn OfflinePage,

    stop: Arc<AtomicBool>,

    fails: i64,
    ping_fails: i64,
    restart_done: bool,
    nav_state: NavState,
    last_nav: i64,
    last_restart: i64,
    last_main_pid: u32,
    /// Has the browser answered even once since this process started?
    seen_alive: bool,
    /// The origin the browser is allowed to be on. Starts unset, meaning
    /// "whatever `KIOSK_URL` says", and becomes wherever our own navigation
    /// actually lands.
    accepted_origin: Option<String>,
    /// We navigated last cycle, so this cycle's URL is the landing point.
    awaiting_landing: bool,
}

impl<'a> Agent<'a> {
    pub fn new(
        config: &'a Config,
        log: &'a Log,
        probe: &'a dyn Prober,
        cdp: &'a dyn Cdp,
        units: &'a dyn Units,
        offline: &'a dyn OfflinePage,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            log,
            probe,
            cdp,
            units,
            offline,
            stop,
            fails: 0,
            ping_fails: 0,
            restart_done: false,
            nav_state: NavState::Unknown,
            last_nav: 0,
            last_restart: 0,
            last_main_pid: 0,
            seen_alive: false,
            accepted_origin: None,
            awaiting_landing: false,
        }
    }

    pub fn run(&mut self) {
        if !self.config.agent_enable {
            // Parked rather than exiting: the unit still shows as running and
            // the reason is in the journal. Useful while debugging a page.
            self.log.info("KIOSK_AGENT_ENABLE is off; idling");
            while self.running() {
                self.nap(60);
            }
            return;
        }

        if self.config.probe_enabled() {
            self.offline.stage();
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
            self.cycle(now());

            let interval = if self.fails > 0 {
                self.config.probe_interval_fail
            } else {
                self.config.probe_interval
            };
            self.nap(interval);
        }

        self.log.info("stopping");
    }

    /// One pass of the loop. Public, and taking an explicit clock, so tests
    /// can drive the whole state machine without sleeping.
    pub fn cycle(&mut self, now: i64) {
        // A bug in one pass must not take the service down with it: systemd
        // would restart us, but the backoff and "one restart per outage"
        // bookkeeping lives in memory and would be lost, which is how a
        // crash-loop turns into the browser being restarted every ten seconds.
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| self.cycle_inner(now)));

        if let Err(payload) = outcome {
            self.log
                .info(format!("cycle failed: {}", panic_message(&payload)));
        }
    }

    fn cycle_inner(&mut self, now: i64) {
        let config = self.config;

        // Liveness first, so navigation failures below are attributed
        // correctly.
        if self.cdp.alive() {
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
        let main_pid = self.units.main_pid();
        if main_pid > 0 && self.last_main_pid > 0 && main_pid != self.last_main_pid {
            self.log.info(format!(
                "chromium restarted (pid {main_pid}); will re-navigate"
            ));
            self.nav_state = NavState::Unknown;
        }
        if main_pid > 0 {
            self.last_main_pid = main_pid;
        }

        let result = if config.probe_enabled() {
            self.probe.call(config.probe_target())
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
            let drifted = self.drifted_origin();
            if let Some(url) = &drifted {
                self.log.info(format!(
                    "chromium is showing {url}; returning to the kiosk URL"
                ));
            }

            if self.nav_state != NavState::Live
                || drifted.is_some()
                || (config.refresh_interval > 0 && now - self.last_nav >= config.refresh_interval)
            {
                self.go_live(now);
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
                self.go_offline(now);
            }

            // Escalation: down long enough that a wedged web process is worth
            // ruling out, and the screen already shows our page so the restart
            // costs nothing visible. Once per outage - restarting against a
            // dead network helps nobody.
            if !self.restart_done
                && config.restart_after > 0
                && self.fails >= config.restart_after
                && self.restart(
                    &format!("no successful probe in {} attempts", self.fails),
                    now,
                )
            {
                self.restart_done = true;
            }
        }

        // Escalation, the other trigger: chromium itself stopped answering.
        // Independent of the probe, because this one is about the browser and
        // not the network, and so deliberately not gated by restart_done.
        if self.ping_fails >= config.ping_fails {
            self.restart(
                &format!("no CDP reply after {} attempts", self.ping_fails),
                now,
            );
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
    fn drifted_origin(&mut self) -> Option<String> {
        if !self.config.enforce_origin || self.nav_state != NavState::Live {
            return None;
        }

        let want = match &self.accepted_origin {
            Some(origin) => origin.clone(),
            None => crate::url::origin(&self.config.kiosk_url)?.to_string(),
        };

        let current = self.cdp.current_url()?;
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
    /// info put two lines that read like faults into the journal of every
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

    fn go_live(&mut self, now: i64) {
        match self.cdp.navigate(&self.config.kiosk_url) {
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

    fn go_offline(&mut self, now: i64) {
        if self.config.offline_url == "none" {
            return;
        }

        // Staged on every use, so dropping a new file into /data/kiosk takes
        // effect without restarting anything.
        let uri = if self.config.offline_url.is_empty() {
            match self.offline.stage() {
                Some(uri) => uri,
                None => return,
            }
        } else {
            self.config.offline_url.clone()
        };

        match self.cdp.navigate(&uri) {
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

    fn restart(&mut self, reason: &str, now: i64) -> bool {
        let unit = &self.config.unit;

        if now - self.last_restart < self.config.restart_backoff {
            self.log
                .debug(format!("not restarting {unit} ({reason}): within backoff"));
            return false;
        }

        // If an operator stopped the unit by hand to look at something, do not
        // fight them. "failed" is still ours to fix - that is systemd giving
        // up, not a person deciding.
        let state = self.units.active_state();
        if !matches!(state.as_str(), "active" | "activating" | "failed") {
            self.log
                .info(format!("{unit} is '{state}'; leaving it alone"));
            return false;
        }

        // Set before the call, so a restart that raises still consumes the
        // backoff window rather than being retried every cycle.
        self.last_restart = now;
        self.log.info(format!("restarting {unit}: {reason}"));

        match self.units.restart() {
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
        !self.stop.load(Ordering::Relaxed)
    }

    fn nap(&self, seconds: i64) {
        let mut left = seconds;
        while left > 0 && self.running() {
            let slice = left.min(NAP_SLICE);
            std::thread::sleep(Duration::from_secs(slice as u64));
            left -= slice;
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
            }
        }
    }

    impl Cdp for FakeCdp {
        fn alive(&self) -> bool {
            assert!(!self.panics.get(), "the browser exploded");
            self.alive.get()
        }

        fn current_url(&self) -> Option<String> {
            self.current_url.borrow().clone()
        }

        fn navigate(&self, url: &str) -> Result<()> {
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

    impl Units for FakeUnits {
        fn active_state(&self) -> String {
            self.active_state.borrow().clone()
        }

        fn main_pid(&self) -> u32 {
            self.main_pid.get()
        }

        fn restart(&self) -> Result<()> {
            self.restarts.set(self.restarts.get() + 1);
            Ok(())
        }
    }

    struct FakeProbe {
        result: RefCell<ProbeResult>,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            Self {
                result: RefCell::new(ProbeResult::ok(200)),
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

    impl Prober for FakeProbe {
        fn call(&self, _url: &str) -> ProbeResult {
            self.result.borrow().clone()
        }
    }

    #[derive(Default)]
    struct FakeOffline {
        staged: Cell<usize>,
    }

    impl OfflinePage for FakeOffline {
        fn stage(&self) -> Option<String> {
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
                Arc::new(AtomicBool::new(false)),
            )
        }

        fn navigations(&self) -> Vec<String> {
            self.cdp.navigations.borrow().clone()
        }
    }

    #[test]
    fn the_first_cycle_navigates_to_the_kiosk_url() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);

        assert_eq!(world.navigations(), vec!["http://kiosk.test/"]);
    }

    #[test]
    fn a_healthy_kiosk_is_not_re_navigated_every_probe() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        agent.cycle(1030);

        assert_eq!(world.navigations().len(), 1);
    }

    #[test]
    fn the_refresh_interval_re_navigates() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        agent.cycle(1600);

        assert_eq!(world.navigations().len(), 2);
    }

    #[test]
    fn the_offline_page_waits_for_the_fail_threshold() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000);
        assert!(world.navigations().is_empty());

        agent.cycle(1010);
        assert_eq!(world.navigations(), vec![OFFLINE_URI]);
    }

    #[test]
    fn the_offline_page_is_not_re_navigated_before_its_own_interval() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000);
        agent.cycle(1010);
        agent.cycle(1020);

        assert_eq!(world.navigations().len(), 1);
    }

    #[test]
    fn recovery_navigates_back_to_the_kiosk_url() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000);
        agent.cycle(1010);
        world.probe.succeed();
        agent.cycle(1020);

        assert_eq!(world.navigations(), vec![OFFLINE_URI, "http://kiosk.test/"]);
    }

    #[test]
    fn a_silent_browser_is_restarted_and_then_re_navigated() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        agent.cycle(1000);
        agent.cycle(1010);
        agent.cycle(1020);

        assert_eq!(world.units.restarts.get(), 1);

        // The restart left nav_state unknown, so the next healthy cycle has to
        // navigate again rather than assume the page survived.
        agent.cycle(1030);
        assert_eq!(world.navigations().len(), 2);
    }

    #[test]
    fn restarts_respect_the_backoff() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        for now in [1000, 1010, 1020] {
            agent.cycle(now);
        }
        assert_eq!(world.units.restarts.get(), 1);

        for now in (1030..=1080).step_by(10) {
            agent.cycle(now);
        }
        assert_eq!(world.units.restarts.get(), 1);

        agent.cycle(1020 + 301);
        assert_eq!(world.units.restarts.get(), 2);
    }

    #[test]
    fn the_network_escalation_fires_once_per_outage() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        let mut clock = 1000;
        for _ in 0..40 {
            agent.cycle(clock);
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 1);

        for _ in 0..10 {
            agent.cycle(clock);
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 1);

        world.probe.succeed();
        agent.cycle(clock);
        clock += 10;
        world.probe.fail();
        for _ in 0..40 {
            agent.cycle(clock);
            clock += 10;
        }
        assert_eq!(world.units.restarts.get(), 2);
    }

    #[test]
    fn an_operator_stopped_unit_is_left_alone() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        *world.units.active_state.borrow_mut() = "inactive".to_string();

        for now in [1000, 1010, 1020, 1030] {
            agent.cycle(now);
        }

        assert_eq!(world.units.restarts.get(), 0);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("leaving it alone")));
    }

    #[test]
    fn a_new_main_pid_forces_a_re_navigation() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        world.units.main_pid.set(77);
        agent.cycle(1010);

        assert_eq!(world.navigations().len(), 2);
    }

    #[test]
    fn a_non_http_target_runs_refresh_only() {
        let world = World::new(&[
            ("KIOSK_PROBE_URL", ""),
            ("KIOSK_URL", "data:text/html,<h1>hi</h1>"),
        ]);
        let mut agent = world.agent();

        agent.cycle(1000);

        assert_eq!(world.navigations(), vec!["data:text/html,<h1>hi</h1>"]);
        assert_eq!(world.offline.staged.get(), 0);
    }

    #[test]
    fn a_failed_navigation_counts_towards_the_cdp_escalation() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        world.cdp.navigate_fails.set(true);

        agent.cycle(1000);
        agent.cycle(1010);

        // Two cycles, not the three a silent browser alone would need: each
        // cycle contributes both an unanswered probe and a refused
        // navigation, and a browser that will not take a navigation is as
        // broken as one that will not answer at all.
        assert!(world.navigations().is_empty());
        assert_eq!(world.units.restarts.get(), 1);
    }

    #[test]
    fn wandering_off_the_site_snaps_back_and_says_so() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        // Second cycle settles where that navigation landed. Only after that
        // is a change attributable to somebody else - see drifted_origin for
        // the one window this leaves.
        agent.cycle(1005);

        *world.cdp.current_url.borrow_mut() = Some("https://youtube.com/watch?v=x".to_string());
        agent.cycle(1010);

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

    #[test]
    fn a_cross_origin_redirect_does_not_loop() {
        // Regression, found on a device: https://freevision.sk redirects to
        // https://www.freevision.sk, a different origin, and comparing against
        // the configured URL made the agent reload the page every cycle
        // forever - a kiosk refreshing itself every 30 seconds.
        let world = World::new(&[("KIOSK_URL", "https://kiosk.test/")]);
        let mut agent = world.agent();

        // The site sends every visit to www.
        *world.cdp.redirect_to.borrow_mut() = Some("https://www.kiosk.test/".to_string());

        for now in [1000, 1005, 1010, 1015, 1020, 1025] {
            agent.cycle(now);
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

    #[test]
    fn drift_is_still_caught_after_a_redirect_was_accepted() {
        let world = World::new(&[("KIOSK_URL", "https://kiosk.test/")]);
        let mut agent = world.agent();
        *world.cdp.redirect_to.borrow_mut() = Some("https://www.kiosk.test/".to_string());

        agent.cycle(1000);
        agent.cycle(1005);
        assert_eq!(world.navigations().len(), 1);

        // Someone follows a link off the site.
        *world.cdp.current_url.borrow_mut() = Some("https://elsewhere.test/".to_string());
        agent.cycle(1010);

        assert_eq!(world.navigations().len(), 2);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is showing https://elsewhere.test/")));
    }

    #[test]
    fn the_sites_own_pages_are_left_alone() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        // Same origin: a sub-page, a query string, a fragment. All legitimate.
        for url in [
            "http://kiosk.test/about",
            "http://kiosk.test/?utm_source=x",
            "http://kiosk.test/a/b#c",
        ] {
            *world.cdp.current_url.borrow_mut() = Some(url.to_string());
            agent.cycle(1005);
        }

        assert_eq!(world.navigations().len(), 1, "should not have re-navigated");
    }

    #[test]
    fn a_browser_that_will_not_say_where_it_is_is_left_alone() {
        // "Cannot tell" must never be mistaken for "has drifted", or a browser
        // mid-navigation would be yanked back on every cycle.
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        *world.cdp.current_url.borrow_mut() = None;
        agent.cycle(1005);

        assert_eq!(world.navigations().len(), 1);
    }

    #[test]
    fn enforcement_can_be_turned_off() {
        let world = World::new(&[("KIOSK_ENFORCE_ORIGIN", "0")]);
        let mut agent = world.agent();

        agent.cycle(1000);
        *world.cdp.current_url.borrow_mut() = Some("https://elsewhere.test/".to_string());
        agent.cycle(1005);

        assert_eq!(world.navigations().len(), 1);
    }

    #[test]
    fn drift_is_not_chased_while_the_offline_page_is_up() {
        // The offline page is a file:// URL, which has no origin at all. It is
        // there on purpose and must not be treated as the browser wandering.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.probe.fail();

        agent.cycle(1000);
        agent.cycle(1010);
        assert_eq!(world.navigations(), vec![OFFLINE_URI]);

        agent.cycle(1020);
        assert_eq!(world.navigations().len(), 1);
    }

    #[test]
    fn a_recovered_browser_is_reported() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        agent.cycle(1000);
        world.cdp.alive.set(false);
        agent.cycle(1005);
        world.cdp.alive.set(true);
        agent.cycle(1010);

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is answering again after 1 failed checks")));
    }

    #[test]
    fn the_startup_race_is_not_reported_as_a_fault() {
        // Every boot starts here: systemd has exec'd Chromium, so the unit
        // counts as started, but the DevTools port is not open yet.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);
        world.cdp.navigate_fails.set(true);

        agent.cycle(1000);
        agent.cycle(1005);

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

    #[test]
    fn a_browser_that_never_comes_up_still_reports_the_restart() {
        // The quiet start must not swallow a browser that is genuinely dead:
        // the escalation is the line worth reading, and it stays at info.
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.alive.set(false);

        for now in [1000, 1005, 1010] {
            agent.cycle(now);
        }

        assert_eq!(world.units.restarts.get(), 1);
        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.starts_with("restarting tessaro-kiosk.service:")));
    }

    #[test]
    fn a_browser_that_dies_after_working_is_reported() {
        let world = World::new(&[]);
        let mut agent = world.agent();

        // One healthy cycle proves the browser can answer...
        agent.cycle(1000);
        assert!(!world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("not answering")));

        // ...so from here on, silence would be hiding a real fault.
        world.cdp.alive.set(false);
        agent.cycle(1005);

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.contains("chromium is not answering")));
    }

    #[test]
    fn a_cycle_survives_a_dependency_blowing_up() {
        let world = World::new(&[]);
        let mut agent = world.agent();
        world.cdp.panics.set(true);

        agent.cycle(1000);

        assert!(world
            .log
            .lines()
            .iter()
            .any(|line| line.starts_with("cycle failed:")));
    }
}
