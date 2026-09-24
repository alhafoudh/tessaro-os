//! The systemd watchdog: proving the state machine is still making progress.
//!
//! The two obvious designs are both wrong. A timer that pings every N seconds
//! keeps patting the dog while the state machine sits wedged in an await that
//! never returns - the one failure this exists to catch. Pinging at the end of
//! each cycle does catch it, but then `WatchdogSec=` must exceed the probe
//! interval plus the slowest legitimate cycle, and both are settings
//! (`tessaro-ctl config set`): someone raising `agent.probe_timeout` for a
//! slow link would silently turn a healthy agent into a restart loop against
//! a number baked into the image.
//!
//! So the state machine publishes a *pledge* instead: before every external
//! call, "whatever I am doing now will be over by T". The keepalive task pings
//! only while T is still in the future. The pledge is made by the same
//! `within()` that enforces the call's deadline, so the two cannot drift
//! apart, and a pledge is computed from the configured timeout at runtime -
//! `WatchdogSec=` is decoupled from every tunable. Waiting between cycles is a
//! pledge too, so idle is alive.
//!
//! `tokio::time::Instant`, never `SystemTime`: a kiosk with no RTC takes a
//! large NTP step minutes after boot, and a wall-clock staleness check would
//! then either fire spuriously or never fire again.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use crate::deadline::{self, Expired};
use crate::log::Log;
use crate::notify::Notifier;

/// The ceiling on any single pledge, so that an enormous configured timeout
/// cannot disarm the watchdog for an hour. `config::Config::oversized_budgets`
/// warns at startup about any budget this clamps.
pub const MAX_PLEDGE: Duration = Duration::from_secs(120);

/// Slack on every pledge, for scheduling and for the bookkeeping either side
/// of the call.
pub const GRACE: Duration = Duration::from_secs(5);

/// Between calls the state machine only compares strings and parses JSON.
const STEP: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct Pledge {
    due: Instant,
    /// What the notifier names before it stops patting. "watchdog: the kiosk
    /// probe is 12s overdue" tells a technician where to look; systemd's own
    /// "Watchdog timeout!" does not.
    what: &'static str,
}

/// Shared between the state machine, which writes it, and the keepalive task,
/// which reads it. `std::sync::Mutex`, never held across an await.
///
/// A *detached* heartbeat publishes nothing. It is what background tasks and
/// tests hold: the CDP session driver, for one, reconnects in a loop of its
/// own, and if it could renew the pledge it would keep the watchdog fed while
/// the state machine was stuck.
#[derive(Clone)]
pub struct Heartbeat(Option<Arc<Mutex<Pledge>>>);

impl Heartbeat {
    /// Starts out pledging `startup`: the first cycle has not begun yet, but
    /// systemd armed the watchdog at exec.
    pub fn new(startup: Duration) -> Self {
        Self(Some(Arc::new(Mutex::new(Pledge {
            due: Instant::now() + startup.min(MAX_PLEDGE),
            what: "starting up",
        }))))
    }

    pub fn detached() -> Self {
        Self(None)
    }

    pub fn pledge(&self, what: &'static str, budget: Duration) {
        if let Some(pledge) = &self.0 {
            let mut pledge = pledge
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *pledge = Pledge {
                due: Instant::now() + budget.min(MAX_PLEDGE),
                what,
            };
        }
    }

    /// `Some((what, by how much))` once the current pledge has passed.
    pub fn overdue(&self) -> Option<(&'static str, Duration)> {
        let pledge = *self
            .0
            .as_ref()?
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        (now >= pledge.due).then(|| (pledge.what, now - pledge.due))
    }

    /// An external call from the state machine: pledge, enforce the deadline,
    /// then pledge the short step to whatever comes next.
    pub async fn within<T>(
        &self,
        what: &'static str,
        budget: Duration,
        work: impl Future<Output = T>,
    ) -> Result<T, Expired> {
        self.pledge(what, budget + GRACE);
        let outcome = deadline::within(what, budget, work).await;
        self.pledge("between steps", STEP);
        outcome
    }
}

/// Keeps systemd's watchdog fed for as long as the heartbeat is honest.
///
/// Ticks at a quarter of `WatchdogSec`, not systemd's conventional half: at
/// half one late tick is already a kill, at a quarter two can go missing.
/// The first tick is immediate, because systemd armed the watchdog at exec
/// and the first cycle may well outlast `WatchdogSec`.
///
/// `judge` off (`KIOSK_WATCHDOG=0`) keeps pinging regardless of the pledge.
/// Not pinging is not an option: systemd has already armed the watchdog, so
/// a keepalive that goes quiet guarantees the very kill it meant to avoid.
/// This way the runtime-wedge half of the protection survives.
pub fn spawn(
    notifier: Option<Notifier>,
    heartbeat: Heartbeat,
    log: Arc<Log>,
    mut shutdown: watch::Receiver<bool>,
    judge: bool,
) {
    let Some(notifier) = notifier else {
        log.debug("no NOTIFY_SOCKET; not started by systemd");
        return;
    };
    let Some(window) = notifier.watchdog() else {
        log.debug("no WATCHDOG_USEC; systemd did not arm a watchdog");
        return;
    };

    let tick = (window / 4).max(Duration::from_millis(250));
    if judge {
        log.info(format!(
            "watchdog armed: systemd expects a ping every {}s; pinging every {}s while the loop keeps its promises",
            window.as_secs(),
            tick.as_secs()
        ));
    } else {
        log.info(format!(
            "watchdog armed but KIOSK_WATCHDOG=0: pinging every {}s without judging the loop",
            tick.as_secs()
        ));
    }

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tick);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut withheld = false;

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticker.tick() => {}
            }

            match heartbeat.overdue() {
                Some((what, by)) if judge => {
                    if !withheld {
                        log.info(format!(
                            "watchdog: {what} is {}s overdue; letting systemd restart the agent",
                            by.as_secs()
                        ));
                        withheld = true;
                    }
                }
                _ => {
                    if withheld {
                        log.info("watchdog: the loop caught up; pinging again");
                        withheld = false;
                    }
                    notifier.send("WATCHDOG=1", &log);
                }
            }
        }

        notifier.send("STOPPING=1", &log);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::test_support::FakeSystemd;
    use std::collections::HashMap;

    fn notifier_for(systemd: &FakeSystemd, watchdog_sec: u64) -> Notifier {
        let env: HashMap<String, String> = [
            ("NOTIFY_SOCKET".to_string(), systemd.notify_socket.clone()),
            (
                "WATCHDOG_USEC".to_string(),
                (watchdog_sec * 1_000_000).to_string(),
            ),
        ]
        .into_iter()
        .collect();
        Notifier::from_env(&env, &Log::buffered(true)).expect("notifier")
    }

    /// Advance virtual time and let the keepalive task run.
    async fn advance(seconds: u64) {
        tokio::time::advance(Duration::from_secs(seconds)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    fn pings(systemd: &FakeSystemd) -> usize {
        systemd
            .received()
            .iter()
            .filter(|message| *message == "WATCHDOG=1")
            .count()
    }

    #[tokio::test(start_paused = true)]
    async fn a_kept_promise_keeps_the_dog_fed() {
        let systemd = FakeSystemd::abstract_socket();
        let heartbeat = Heartbeat::new(Duration::from_secs(30));
        let log = Arc::new(Log::buffered(false));
        let (_stop, shutdown) = watch::channel(false);

        spawn(
            Some(notifier_for(&systemd, 60)),
            heartbeat.clone(),
            log,
            shutdown,
            true,
        );

        // The first tick is immediate: systemd armed the watchdog at exec.
        advance(0).await;
        assert_eq!(pings(&systemd), 1);

        // Re-pledging keeps it alive indefinitely.
        for _ in 0..8 {
            heartbeat.pledge("the kiosk probe", Duration::from_secs(20));
            advance(15).await;
        }
        assert_eq!(pings(&systemd), 8);
    }

    #[tokio::test(start_paused = true)]
    async fn a_broken_promise_stops_the_pings_and_says_why() {
        let systemd = FakeSystemd::abstract_socket();
        let heartbeat = Heartbeat::new(Duration::from_secs(30));
        let log = Arc::new(Log::buffered(false));
        let (_stop, shutdown) = watch::channel(false);

        spawn(
            Some(notifier_for(&systemd, 60)),
            heartbeat.clone(),
            log.clone(),
            shutdown,
            true,
        );
        advance(0).await;

        // The state machine goes into a call and never comes back out.
        heartbeat.pledge("the kiosk probe", Duration::from_secs(20));
        advance(15).await;
        assert_eq!(pings(&systemd), 2, "still inside the pledge");

        advance(15).await;
        advance(15).await;
        advance(15).await;
        assert_eq!(pings(&systemd), 0, "no ping once the pledge has passed");

        let lines = log.lines();
        let overdue: Vec<_> = lines
            .iter()
            .filter(|line| line.contains("overdue"))
            .collect();
        assert_eq!(overdue.len(), 1, "said once, not every tick: {lines:?}");
        assert!(overdue[0].contains("the kiosk probe"));
    }

    #[tokio::test(start_paused = true)]
    async fn within_pledges_the_call_and_then_the_step() {
        let heartbeat = Heartbeat::new(Duration::from_secs(1));

        let outcome = heartbeat
            .within("the system bus", Duration::from_secs(5), async {
                tokio::time::sleep(Duration::from_secs(3)).await;
            })
            .await;
        assert!(outcome.is_ok());
        assert_eq!(heartbeat.overdue(), None);

        advance(6).await;
        assert_eq!(
            heartbeat.overdue().map(|(what, _)| what),
            Some("between steps")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_enormous_budget_is_clamped() {
        let heartbeat = Heartbeat::new(Duration::from_secs(1));

        heartbeat.pledge("a huge timeout", Duration::from_secs(3600));
        advance(MAX_PLEDGE.as_secs()).await;

        assert_eq!(
            heartbeat.overdue().map(|(what, _)| what),
            Some("a huge timeout")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn not_judging_still_pings() {
        let systemd = FakeSystemd::abstract_socket();
        let heartbeat = Heartbeat::new(Duration::from_secs(1));
        let (_stop, shutdown) = watch::channel(false);

        spawn(
            Some(notifier_for(&systemd, 60)),
            heartbeat,
            Arc::new(Log::buffered(false)),
            shutdown,
            false,
        );
        advance(0).await;
        advance(15).await;
        advance(15).await;

        assert_eq!(
            pings(&systemd),
            3,
            "the pledge expired, but KIOSK_WATCHDOG=0"
        );
    }

    #[test]
    fn a_detached_heartbeat_is_never_overdue() {
        let heartbeat = Heartbeat::detached();
        heartbeat.pledge("anything", Duration::ZERO);

        assert_eq!(heartbeat.overdue(), None);
    }
}
