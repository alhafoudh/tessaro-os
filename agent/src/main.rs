//! tessaro-agent: supervises the Tessaro kiosk browser.
//!
//! It runs as a plain systemd service on the device. Everything it needs
//! arrives as environment variables, parsed by systemd from
//! `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` and `/etc/default/tessaro-kiosk`
//! - there is no config file of its own and no command line to speak of.
//!
//! One thread, one tokio runtime, current-thread on purpose. The state machine
//! is sequential by design and needs no parallelism; what it needs is that a
//! blocking call anywhere is *noticed*. On a current-thread runtime a stray
//! blocking call stalls the watchdog keepalive along with everything else,
//! systemd restarts us, and the journal says why - so the watchdog doubles as
//! a standing test of the no-blocking-calls rule the whole design rests on.

mod agent;
mod cdp;
mod config;
mod deadline;
mod error;
mod http;
mod log;
mod notify;
mod offline;
mod ports;
mod probe;
mod systemd;
mod url;
mod watchdog;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;

use config::{Config, SystemEnv};
use log::Log;
use watchdog::Heartbeat;

/// What the agent pledges before its first cycle: parsing the environment,
/// connecting to the bus and staging the offline page. Each step pledges its
/// own budget as it goes; this only covers the gaps between them.
const STARTUP: Duration = Duration::from_secs(30);

/// Bytes of a probed page worth reading. The probe wants the status; a large
/// home page is truncated here rather than failed.
const PROBE_BODY: usize = 256 * 1024;

fn main() -> ExitCode {
    let config = Config::from_env();
    let log = Arc::new(Log::new(config.debug));

    if config.kiosk_url.is_empty() {
        log.info("KIOSK_URL is empty; nothing to watch");
        return ExitCode::FAILURE;
    }

    // getaddrinfo runs on the blocking pool. Its threads are bounded by
    // glibc's own resolver timeout, so a handful is plenty; a low ceiling
    // means a regression shows up as a loud stall rather than as hundreds of
    // quietly parked threads.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(8)
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            log.info(format!("could not start the runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(run(config, log));
    ExitCode::SUCCESS
}

async fn run(config: Config, log: Arc<Log>) {
    // First, before anything that can touch the network or the bus: systemd
    // armed the watchdog at exec, so the keepalive has to be running already.
    let heartbeat = Heartbeat::new(STARTUP);
    let stop = Arc::new(watch::channel(false).0);
    tokio::spawn(signals(Arc::clone(&stop), Arc::clone(&log)));

    let notifier = notify::Notifier::from_env(&SystemEnv, &log);
    watchdog::spawn(
        notifier,
        heartbeat.clone(),
        Arc::clone(&log),
        stop.subscribe(),
        config.watchdog,
    );

    for (key, seconds) in config.oversized_budgets() {
        log.info(format!(
            "{key}={seconds} is longer than the watchdog allows one call ({}s); \
             a call that really takes that long will get the agent restarted",
            watchdog::MAX_PLEDGE.as_secs()
        ));
    }

    let probe = probe::Probe::new(
        &log,
        http::HyperHttp::new(
            config.probe_connect_timeout,
            config.probe_timeout,
            PROBE_BODY,
            heartbeat.clone(),
        ),
        config.probe_connect_timeout,
        config.probe_timeout,
    );

    let session = cdp::session::spawn(
        cdp::session::SessionConfig {
            base_url: config.cdp_url.trim_end_matches('/').to_string(),
            timeout: seconds(config.cdp_timeout.max(1)),
            ping: seconds(config.cdp_ping.max(1)),
            reconnect_max: seconds(config.cdp_reconnect_max.max(1)),
            device_access: config.device_access,
        },
        Arc::clone(&log),
        stop.subscribe(),
    );
    let first_attempt = session.clone();
    let cdp = cdp::CdpClient::new(&log, session, heartbeat.clone(), config.cdp_timeout);

    let units = systemd::Systemd::connect(&config.unit, &log, heartbeat.clone()).await;
    let offline = offline::Offline::new(&log, &config);

    // Let the session try once before the first cycle asks it anything. At
    // boot Chromium's port is usually still closed and this returns at once;
    // after an agent restart it saves reporting a healthy browser as silent.
    let _ = heartbeat
        .within(
            "the first DevTools connection",
            seconds(config.cdp_timeout.max(1)),
            first_attempt.first_attempt(),
        )
        .await;

    agent::Agent::new(
        &config,
        &log,
        &probe,
        &cdp,
        &units,
        &offline,
        stop.subscribe(),
        heartbeat,
    )
    .run()
    .await;
}

/// SIGTERM, SIGINT and SIGHUP all mean "stop". The sender is shared with
/// `run`, so it outlives this task: a dropped sender would read as a stop.
async fn signals(stop: Arc<watch::Sender<bool>>, log: Arc<Log>) {
    let handlers = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    );

    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = handlers else {
        // Without handlers the default action applies, which terminates the
        // process - systemd's stop still works, just without the tidy exit.
        log.info("could not install signal handlers; stopping will not be graceful");
        return;
    };

    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
        _ = hup.recv() => {}
    }

    stop.send_replace(true);
}

fn seconds(value: i64) -> Duration {
    Duration::from_secs(value.max(0) as u64)
}
