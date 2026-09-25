//! `tessaro-ctl network speedtest`: the device's own internet connection, measured
//! against speed.cloudflare.com by the cfspeedtest crate.
//!
//! It runs here, not in the client, because the question is what the kiosk
//! gets where it hangs - a laptop next to it on another network answers a
//! different one.
//!
//! cfspeedtest is blocking reqwest, so the whole test is one
//! `spawn_blocking` thread that sends a `SpeedtestEvent` per step down a
//! channel, and the server forwards each as it arrives. These rules follow:
//!
//! 1. **The reqwest client lives and dies on that thread.** A blocking
//!    client owns a runtime of its own, and dropping one on the agent's
//!    runtime thread panics.
//! 2. **The thread always ends on its own.** Every request has a 30s
//!    timeout, and the thread stops at the next step once nobody is
//!    listening, so a client that leaves or a deadline that fires does not
//!    leave a download running behind it. The server bounds the whole test
//!    with `TOTAL` and answers an error when it runs out, so the agent is
//!    never the one waiting on Cloudflare.
//! 3. **One at a time.** The lock is held by the thread, not the request, so
//!    a second test cannot start while an abandoned one is still finishing.
//!
//! Payload sizes are run one per call, smallest first, so each is its own
//! event, and a size that took longer than `GROW_LIMIT` is the last -
//! cfspeedtest's own rule, which keeps a slow link from spending minutes on
//! 25 MB samples.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cfspeedtest::speedtest::{fetch_metadata, run_latency_test, run_tests_with_retries, TestType};
use cfspeedtest::OutputFormat;
use protocol::{
    speedtest_size_label as size_label, Direction, SpeedtestEvent, SPEEDTEST_DEFAULT_SIZE,
    SPEEDTEST_DEFAULT_TESTS, SPEEDTEST_SIZES, SPEEDTEST_UPLOAD_MAX,
};
use tokio::sync::{mpsc, OwnedMutexGuard};

use crate::http::USER_AGENT;
use crate::log::Log;

/// The whole test, from the server's side. A slow link that has not finished
/// by then gets what it has so far.
pub const TOTAL: Duration = Duration::from_secs(5 * 60);

/// One request: cfspeedtest's own client timeout.
const REQUEST: Duration = Duration::from_secs(30);

/// A payload size that took longer than this is the last one tried.
const GROW_LIMIT: Duration = Duration::from_secs(5);

const LATENCY_SAMPLES: u32 = 25;
const MAX_TESTS: u32 = 100;

/// What to run, validated before anything starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    sizes: Vec<u64>,
    tests: u32,
    /// The device's local proxy, when the test goes through it; `None` goes
    /// straight out, and never through an `HTTP_PROXY` in the environment.
    pub proxy: Option<std::net::SocketAddr>,
}

impl Plan {
    pub fn new(max_size: Option<u64>, tests: Option<u32>) -> Result<Self, String> {
        let max_size = max_size.unwrap_or(SPEEDTEST_DEFAULT_SIZE);
        if !SPEEDTEST_SIZES.contains(&max_size) {
            return Err(format!(
                "the largest payload must be one of {}",
                SPEEDTEST_SIZES.map(size_label).join(", ")
            ));
        }
        let tests = tests.unwrap_or(SPEEDTEST_DEFAULT_TESTS);
        if !(1..=MAX_TESTS).contains(&tests) {
            return Err(format!("tests must be between 1 and {MAX_TESTS}"));
        }
        Ok(Self {
            sizes: SPEEDTEST_SIZES
                .into_iter()
                .filter(|size| *size <= max_size)
                .collect(),
            tests,
            proxy: None,
        })
    }
}

/// One message from the thread: a step, or the reason it cannot go on.
pub type Step = Result<SpeedtestEvent, String>;

/// Start the test on its own thread. `lock` is held until that thread is done.
pub fn start(plan: Plan, lock: OwnedMutexGuard<()>, log: Arc<Log>) -> mpsc::Receiver<Step> {
    crate::sync::spawn_steps(lock, move |send| run(&plan, send, &log))
}

/// Everything on the blocking thread. Returns early once `send` says
/// nobody is listening: whoever asked has gone.
fn run(plan: &Plan, send: &dyn Fn(Step) -> bool, log: &Log) {
    // Plain HTTP to the local proxy, which does the upstream's scheme and
    // credentials; so no reqwest socks feature, whatever network.proxy.url
    // speaks.
    let builder = reqwest::blocking::Client::builder()
        .timeout(REQUEST)
        .user_agent(USER_AGENT);
    let builder = match plan.proxy {
        Some(proxy) => match reqwest::Proxy::all(format!("http://{proxy}")) {
            Ok(proxy) => builder.proxy(proxy),
            Err(err) => {
                send(Err(format!("speed test: the proxy: {err}")));
                return;
            }
        },
        None => builder.no_proxy(),
    };
    let client = match builder.build() {
        Ok(client) => client,
        Err(err) => {
            send(Err(format!("speed test: {err}")));
            return;
        }
    };

    // The trace doubles as the reachability check: if it does not answer,
    // nothing after it would either.
    match fetch_metadata(&client) {
        Ok(meta) => {
            if !send(Ok(SpeedtestEvent::Server {
                ip: meta.ip,
                colo: meta.colo,
                country: meta.country,
            })) {
                return;
            }
        }
        Err(err) => {
            let error = format!("speed.cloudflare.com did not answer: {err}");
            log.info(format!("speed test: {error}"));
            send(Err(error));
            return;
        }
    }

    let (latencies, _) = run_latency_test(&client, LATENCY_SAMPLES, OutputFormat::None);
    let latency = Stats::of(&latencies);
    if !send(Ok(SpeedtestEvent::Latency {
        samples: latencies.len() as u32,
        avg_ms: latency.map(|s| s.avg),
        min_ms: latency.map(|s| s.min),
        max_ms: latency.map(|s| s.max),
    })) {
        return;
    }

    let mut best = [None, None];
    for (at, direction) in [Direction::Download, Direction::Upload]
        .into_iter()
        .enumerate()
    {
        let (test_type, cap) = match direction {
            Direction::Download => (TestType::Download, u64::MAX),
            Direction::Upload => (TestType::Upload, SPEEDTEST_UPLOAD_MAX),
        };
        for &size in plan.sizes.iter().filter(|size| **size <= cap) {
            let started = Instant::now();
            let (measurements, attempts) = run_tests_with_retries(
                &client,
                test_type,
                vec![size as usize],
                plan.tests,
                OutputFormat::None,
                true,
            );
            let mbits: Vec<f64> = measurements.iter().map(|m| m.mbit).collect();
            let stats = Stats::of(&mbits);
            if let Some(stats) = stats {
                best[at] = Some(stats.median);
            }
            if !send(Ok(SpeedtestEvent::Transfer {
                direction,
                size,
                samples: mbits.len() as u32,
                attempts: attempts.iter().map(|a| a.attempts).sum(),
                median_mbit: stats.map(|s| s.median),
                min_mbit: stats.map(|s| s.min),
                max_mbit: stats.map(|s| s.max),
            })) {
                return;
            }
            // Nothing came back at this size, so nothing will at a larger one.
            if stats.is_none() || started.elapsed() > GROW_LIMIT {
                break;
            }
        }
    }

    let [download_mbit, upload_mbit] = best;
    let latency_ms = latency.map(|s| s.avg);
    log.info(format!(
        "speed test: download {}, upload {}, latency {}",
        mbit_label(download_mbit),
        mbit_label(upload_mbit),
        latency_ms.map_or("n/a".to_string(), |ms| format!("{ms:.1} ms")),
    ));
    send(Ok(SpeedtestEvent::Result {
        download_mbit,
        upload_mbit,
        latency_ms,
    }));
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Stats {
    median: f64,
    min: f64,
    max: f64,
    avg: f64,
}

impl Stats {
    /// `None` for no samples. Non-finite samples are dropped first.
    fn of(samples: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n.is_multiple_of(2) {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        } else {
            sorted[n / 2]
        };
        Some(Self {
            median,
            min: sorted[0],
            max: sorted[n - 1],
            avg: sorted.iter().sum::<f64>() / n as f64,
        })
    }
}

fn mbit_label(mbit: Option<f64>) -> String {
    mbit.map_or("n/a".to_string(), |v| format!("{v:.1} Mbit/s"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_steps_up_to_the_largest_size() {
        let plan = Plan::new(Some(10_000_000), Some(3)).unwrap();
        assert_eq!(plan.sizes, vec![100_000, 1_000_000, 10_000_000]);
        assert_eq!(plan.tests, 3);

        let default = Plan::new(None, None).unwrap();
        assert_eq!(default.sizes.last(), Some(&SPEEDTEST_DEFAULT_SIZE));
        assert_eq!(default.tests, SPEEDTEST_DEFAULT_TESTS);
    }

    #[test]
    fn a_plan_refuses_what_cfspeedtest_does_not_offer() {
        let err = Plan::new(Some(5_000_000), None).unwrap_err();
        assert!(err.contains("100k, 1m, 10m, 25m, 100m"), "{err}");
        assert!(Plan::new(None, Some(0)).is_err());
        assert!(Plan::new(None, Some(MAX_TESTS + 1)).is_err());
    }

    #[test]
    fn stats_take_the_middle_and_ignore_nonsense() {
        assert_eq!(Stats::of(&[]), None);
        assert_eq!(Stats::of(&[f64::NAN]), None);

        let odd = Stats::of(&[30.0, 10.0, f64::INFINITY, 20.0]).unwrap();
        assert_eq!(
            (odd.median, odd.min, odd.max, odd.avg),
            (20.0, 10.0, 30.0, 20.0)
        );

        let even = Stats::of(&[4.0, 1.0, 3.0, 2.0]).unwrap();
        assert_eq!(even.median, 2.5);
    }

    #[test]
    fn sizes_are_labelled_as_the_client_spells_them() {
        assert_eq!(
            SPEEDTEST_SIZES.map(size_label),
            ["100k", "1m", "10m", "25m", "100m"]
        );
    }

    /// Against the real speed.cloudflare.com, so ignored by default:
    /// `cargo test -p tessaro-agent -- --ignored speedtest`.
    #[tokio::test]
    #[ignore]
    async fn speedtest_against_cloudflare() {
        let lock = Arc::new(tokio::sync::Mutex::new(()))
            .try_lock_owned()
            .unwrap();
        let plan = Plan::new(Some(100_000), Some(2)).unwrap();
        let mut rx = start(plan, lock, Arc::new(Log::buffered(false)));

        let mut steps = Vec::new();
        // naked: a test against the real network, bounded by REQUEST per call
        while let Some(step) = rx.recv().await {
            steps.push(step.expect("step"));
        }
        assert!(matches!(steps.first(), Some(SpeedtestEvent::Server { .. })));
        assert!(matches!(
            steps.last(),
            Some(SpeedtestEvent::Result {
                download_mbit: Some(_),
                ..
            })
        ));
    }
}
