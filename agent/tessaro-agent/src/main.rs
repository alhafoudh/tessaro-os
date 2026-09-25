//! tessaro-agent: supervises the Tessaro kiosk browser, and is the device's
//! one management surface.
//!
//! It runs as a plain systemd service on the device. The image defaults
//! arrive as environment variables, parsed by systemd from
//! `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`; what was set on this device
//! comes from `/data/tessaro/state.json` on top. Nothing else configures it,
//! and the only way to change `state.json` is `tessaro-ctl`, over the local
//! socket or TLS (`control/`, `server.rs`).
//!
//! `tessaro-agent boot` is the other mode: the oneshot that renders the
//! configuration before anything reads it (`boot.rs`).
//!
//! One thread, one tokio runtime, current-thread on purpose. The state machine
//! is sequential by design and needs no parallelism; what it needs is that a
//! blocking call anywhere is *noticed*. On a current-thread runtime a stray
//! blocking call stalls the watchdog keepalive along with everything else,
//! systemd restarts us, and the journal says why - so the watchdog doubles as
//! a standing test of the no-blocking-calls rule the whole design rests on.

mod agent;
mod audio;
mod auth;
mod boot;
mod cdp;
mod config;
mod control;
mod deadline;
mod debug;
mod display;
mod error;
mod files;
mod hotplug;
mod http;
mod identity;
mod log;
mod mdns;
mod net;
mod nm;
mod notify;
mod offline;
mod paths;
mod ping;
mod ports;
mod probe;
mod proc;
mod render;
mod secrets;
mod server;
mod shadow;
mod speedtest;
mod ssh;
mod state;
mod storage;
mod store;
mod sync;
mod systemd;
mod updates;
mod url;
mod watchdog;
mod zoom;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;

use config::{Config, SystemEnv};
use log::Log;
use watchdog::Heartbeat;

/// Everything read from disk before the runtime starts: small, local, and
/// needed by both the state machine and the control plane.
struct Device {
    paths: paths::Paths,
    defaults: HashMap<String, String>,
    auth: auth::Auth,
    identity: control::Identity,
    tls: Option<identity::Tls>,
    listen: Option<SocketAddr>,
    mdns: bool,
}

/// What the agent pledges before its first cycle: parsing the environment,
/// connecting to the bus and staging the offline page. Each step pledges its
/// own budget as it goes; this only covers the gaps between them.
const STARTUP: Duration = Duration::from_secs(30);

/// Bytes of a probed page worth reading. The probe wants the status; a large
/// home page is truncated here rather than failed.
const PROBE_BODY: usize = 256 * 1024;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("boot") => {
            boot::run(&SystemEnv, &Log::new(false));
            return ExitCode::SUCCESS;
        }
        Some("zoom") => {
            zoom::run(&SystemEnv, &Log::new(false));
            return ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("tessaro-agent {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("usage: tessaro-agent [boot | zoom | --version] (got {other:?})");
            return ExitCode::FAILURE;
        }
    }

    // The image defaults are the process environment; the device's settings
    // go on top. Read before the runtime exists, so it cannot block it.
    let bootstrap = Log::new(false);
    let paths = paths::Paths::load(&SystemEnv);
    let defaults = state::defaults(&SystemEnv);
    let settings: state::State = store::Store::new(&paths.state_dir, state::FILE).read(&bootstrap);
    // What the device reports too - derived name, node id, addresses - so a
    // placeholder in browser.url means here exactly what the renderer made of it.
    let effective = state::Effective::new(&SystemEnv, &settings.settings, &bootstrap)
        .with_live(render::live(&paths));

    let config = Config::load(&effective);
    let log = Arc::new(Log::new(config.debug));
    let device = device(paths, defaults, &effective, &log);

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

    runtime.block_on(run(config, log, device, settings.settings));
    ExitCode::SUCCESS
}

fn device(
    paths: paths::Paths,
    defaults: HashMap<String, String>,
    effective: &dyn config::Env,
    log: &Log,
) -> Device {
    let auth: auth::Auth = store::Store::new(&paths.state_dir, auth::FILE).read(log);

    let id = match identity::read_node_id(&paths.machine_id) {
        Ok(id) => id,
        Err(err) => {
            log.info(format!("no node id ({err}); using a placeholder"));
            "00000000000000000000000000000000".to_string()
        }
    };
    let name = match effective
        .get("KIOSK_NODE_NAME")
        .filter(|name| !name.is_empty())
    {
        Some(name) => name,
        None => identity::friendly_name(&id),
    };

    // Normally made by the boot oneshot; made here too so a host run works.
    let tls = match identity::tls(&paths.tls_dir()) {
        Ok((tls, _)) => Some(tls),
        Err(err) => {
            log.info(format!("no TLS identity ({err}); the TCP API stays off"));
            None
        }
    };

    let listen_value = effective
        .get("KIOSK_API_LISTEN")
        .unwrap_or_else(|| format!("0.0.0.0:{}", protocol::DEFAULT_PORT));
    let listen = match listen_value.as_str() {
        "off" => None,
        value => match value.parse() {
            Ok(addr) => Some(addr),
            Err(_) => {
                log.info(format!(
                    "KIOSK_API_LISTEN={value} is not address:port; the TCP API stays off"
                ));
                None
            }
        },
    };

    log.info(format!(
        "node {id}, name {name}, {}",
        if auth.claimed() {
            "claimed"
        } else {
            "unclaimed"
        }
    ));

    Device {
        identity: control::Identity {
            id,
            name,
            machine: paths.machine.clone(),
            fingerprint: tls
                .as_ref()
                .map(|tls| tls.fingerprint.clone())
                .unwrap_or_default(),
        },
        paths,
        defaults,
        auth,
        tls,
        listen,
        mdns: effective.get("KIOSK_MDNS").as_deref() != Some("off"),
    }
}

/// The control plane: the store, the socket, TLS, mDNS, and a probation
/// timer if a guarded change is waiting. None of it is on the state
/// machine's path, and none of it can stop the kiosk: every failure here is
/// a journal line and a missing feature.
async fn start_control(
    device: Device,
    kiosk_url: &str,
    session: cdp::session::SessionHandle,
    log: &Arc<Log>,
    stop: &Arc<watch::Sender<bool>>,
) {
    let bus = systemd::Bus::connect(log).await;
    let socket = device.paths.socket.clone();
    let control = control::Control::new(
        Arc::clone(log),
        device.paths.clone(),
        device.defaults,
        device.auth,
        session,
        bus,
        device.identity,
        stop.subscribe(),
        kiosk_url.to_string(),
    );

    if let Err(err) = server::spawn_unix(
        Arc::clone(&control),
        socket.clone(),
        Arc::clone(log),
        stop.subscribe(),
    ) {
        log.info(format!(
            "control: cannot listen on {}: {err}",
            socket.display()
        ));
    }

    let mut port = None;
    if let (Some(addr), Some(tls)) = (device.listen, device.tls.as_ref()) {
        match server::spawn_tls(Arc::clone(&control), addr, tls, Arc::clone(log), stop.subscribe())
            .await // naked: binding a socket is a local syscall
        {
            Ok(()) => port = Some(addr.port()),
            Err(err) => log.info(format!("control: cannot listen on {addr}: {err}")),
        }
    }

    if let (true, Some(port)) = (device.mdns, port) {
        let node = control.node();
        control.set_mdns(mdns::Mdns::start(
            log,
            &node.name,
            port,
            &node.id,
            &node.fingerprint,
            &node.machine,
            node.claimed,
        ));
    }

    control.arm_if_pending().await; // naked: a disk read under blocking()'s within()
    control.load_update().await; // naked: a disk read under blocking()'s within()
    control.recover_network().await; // naked: disk reads under blocking()'s within()
    control.watch_wifi();
    control.watch_url();
    control.watch_public_ip();
    control.watch_display();
    control.watch_audio();
    control.watch_welcome();
}

async fn run(
    config: Config,
    log: Arc<Log>,
    device: Device,
    settings: std::collections::BTreeMap<String, String>,
) {
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
    let control_session = session.clone();
    let cdp = cdp::CdpClient::new(&log, session, heartbeat.clone(), config.cdp_timeout);

    // The debug screen fills its template in afresh every time, from the
    // settings this agent started with and the device as it is then.
    let paths = device.paths.clone();
    let defaults = device.defaults.clone();
    start_control(device, &config.kiosk_url, control_session, &log, &stop).await;

    let units = systemd::Systemd::connect(&config.unit, &log, heartbeat.clone()).await;
    let offline = offline::Offline::new(&log, &config);
    let debug_screen = debug::Debug::new(&log, &config, &paths, &defaults, &settings);

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
        &debug_screen,
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
