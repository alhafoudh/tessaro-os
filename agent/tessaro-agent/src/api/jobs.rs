//! Jobs: the commands that run in steps (a speed test, a ping, growing
//! `/data`), kept on the device and polled.
//!
//! Starting one answers at once with its id. A task drains the steps into
//! the job as they come, within the command's own total, and a client asks
//! `GET /api/v1/jobs/{job}?after=N` for what came since. A job that has
//! ended is kept for `KEEP`, so a client that polls late still reads how it
//! ended; cancelling one drops its steps, which stops whatever produces
//! them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use protocol::JobPage;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

use crate::control::Stream;
use crate::deadline::within;
use crate::sync::lock;
use crate::{speedtest, storage};

/// How long an ended job stays readable.
const KEEP: Duration = Duration::from_secs(5 * 60);
/// Jobs at once, running or kept. Each is bounded in time, and the speed
/// test and the grow allow one run each, so only pings could pile up.
const MAX: usize = 32;

#[derive(Default)]
pub struct Jobs(Mutex<HashMap<String, Job>>);

#[derive(Default)]
struct Job {
    events: Vec<Value>,
    done: bool,
    error: Option<String>,
    ended: Option<Instant>,
    task: Option<tokio::task::AbortHandle>,
}

impl Jobs {
    /// Keep what `stream` produces as a new job; its id.
    pub fn start(self: &std::sync::Arc<Self>, stream: Stream) -> Result<String, String> {
        let mut random = [0u8; 8];
        openssl::rand::rand_bytes(&mut random).map_err(|err| err.to_string())?;
        let id = protocol::hex(&random);
        {
            let mut jobs = lock(&self.0);
            prune(&mut jobs);
            if jobs.len() >= MAX {
                return Err("too many jobs are running on this device; try again shortly".into());
            }
            jobs.insert(id.clone(), Job::default());
        }
        let jobs = std::sync::Arc::clone(self);
        let job = id.clone();
        let task = tokio::spawn(async move {
            let outcome = match stream {
                Stream::Speedtest(steps) => {
                    // naked: drain bounds the whole run with speedtest::TOTAL
                    drain(&jobs, &job, steps, "the speed test", speedtest::TOTAL).await
                }
                Stream::Grow(steps) => {
                    // naked: drain bounds the whole run with storage::TOTAL
                    drain(&jobs, &job, steps, "growing /data", storage::TOTAL).await
                }
                // naked: drain bounds the whole run with the plan's total
                Stream::Ping { steps, total } => drain(&jobs, &job, steps, "the ping", total).await,
            };
            jobs.end(&job, outcome.err());
        });
        if let Some(job) = lock(&self.0).get_mut(&id) {
            job.task = Some(task.abort_handle());
        }
        Ok(id)
    }

    /// What `id` did from step `after` on; `None` for no such job.
    pub fn page(&self, id: &str, after: u64) -> Option<JobPage> {
        let mut jobs = lock(&self.0);
        prune(&mut jobs);
        let job = jobs.get(id)?;
        let from = (after as usize).min(job.events.len());
        Some(JobPage {
            events: job.events[from..].to_vec(),
            next: job.events.len() as u64,
            done: job.done,
            error: job.error.clone(),
        })
    }

    /// Stop `id` and forget it. False for no such job.
    pub fn cancel(&self, id: &str) -> bool {
        match lock(&self.0).remove(id) {
            Some(job) => {
                if let Some(task) = job.task {
                    task.abort();
                }
                true
            }
            None => false,
        }
    }

    fn push(&self, id: &str, event: Value) {
        if let Some(job) = lock(&self.0).get_mut(id) {
            job.events.push(event);
        }
    }

    fn end(&self, id: &str, error: Option<String>) {
        if let Some(job) = lock(&self.0).get_mut(id) {
            job.done = true;
            job.error = error;
            job.ended = Some(Instant::now());
            job.task = None;
        }
    }
}

fn prune(jobs: &mut HashMap<String, Job>) {
    jobs.retain(|_, job| job.ended.is_none_or(|ended| ended.elapsed() < KEEP));
}

/// Every step into the job, within `total`. The error, if the steps end
/// with one or run out of time.
async fn drain<T: Serialize>(
    jobs: &Jobs,
    id: &str,
    mut steps: Receiver<Result<T, String>>,
    what: &'static str,
    total: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let left = total.saturating_sub(started.elapsed());
        match within(what, left, steps.recv()).await {
            Ok(Some(Ok(step))) => {
                let event = serde_json::to_value(&step).map_err(|err| err.to_string())?;
                jobs.push(id, event);
            }
            Ok(Some(Err(error))) => return Err(error),
            Ok(None) => return Ok(()),
            Err(expired) => return Err(expired.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn latency(ms: f64) -> protocol::SpeedtestEvent {
        protocol::SpeedtestEvent::Latency {
            samples: 1,
            avg_ms: Some(ms),
            min_ms: Some(ms),
            max_ms: Some(ms),
        }
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_job_is_its_steps_then_done() {
        let jobs = Arc::new(Jobs::default());
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let id = jobs.start(Stream::Speedtest(rx)).unwrap();

        tx.send(Ok(latency(9.0))).await.unwrap();
        settle().await;
        let page = jobs.page(&id, 0).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.next, 1);
        assert!(!page.done);

        tx.send(Ok(latency(8.0))).await.unwrap();
        drop(tx);
        settle().await;
        let page = jobs.page(&id, 1).unwrap();
        assert_eq!(
            page.events,
            vec![serde_json::to_value(latency(8.0)).unwrap()]
        );
        assert_eq!(page.next, 2);
        assert!(page.done);
        assert_eq!(page.error, None);
        assert!(jobs.page("b", 0).is_none());
    }

    #[tokio::test]
    async fn a_failed_step_ends_the_job_with_its_error() {
        let jobs = Arc::new(Jobs::default());
        let (tx, rx) = tokio::sync::mpsc::channel::<speedtest::Step>(4);
        let id = jobs.start(Stream::Speedtest(rx)).unwrap();
        tx.send(Err("offline".into())).await.unwrap();
        settle().await;
        let page = jobs.page(&id, 0).unwrap();
        assert!(page.done);
        assert_eq!(page.error.as_deref(), Some("offline"));
    }

    /// A producer that never sends must not keep the job forever.
    #[tokio::test(start_paused = true)]
    async fn a_job_that_goes_silent_is_cut_off() {
        let jobs = Arc::new(Jobs::default());
        let (_tx, rx) = tokio::sync::mpsc::channel::<speedtest::Step>(4);
        let id = jobs.start(Stream::Speedtest(rx)).unwrap();
        tokio::time::sleep(speedtest::TOTAL + Duration::from_secs(1)).await;
        settle().await;
        let page = jobs.page(&id, 0).unwrap();
        assert!(page.done);
        assert!(page.error.unwrap().contains("300s"));

        tokio::time::sleep(KEEP).await;
        assert!(jobs.page(&id, 0).is_none(), "an ended job is forgotten");
    }

    #[tokio::test]
    async fn cancelling_drops_the_steps() {
        let jobs = Arc::new(Jobs::default());
        let (tx, rx) = tokio::sync::mpsc::channel::<speedtest::Step>(4);
        let id = jobs.start(Stream::Speedtest(rx)).unwrap();
        settle().await;
        assert!(jobs.cancel(&id));
        settle().await;
        assert!(tx.is_closed(), "the producer sees the job is gone");
        assert!(!jobs.cancel(&id));
    }
}
