//! tessaro-agent: supervises the Tessaro kiosk browser.
//!
//! It runs as a plain systemd service on the device. Everything it needs
//! arrives as environment variables, parsed by systemd from
//! `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` and `/etc/default/tessaro-kiosk`
//! - there is no config file of its own and no command line to speak of.

mod agent;
mod cdp;
mod config;
mod error;
mod http;
mod log;
mod offline;
mod ports;
mod probe;
mod systemd;
mod url;

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

fn main() -> ExitCode {
    let config = config::Config::from_env();
    let log = log::Log::new(config.debug);

    if config.kiosk_url.is_empty() {
        log.info("KIOSK_URL is empty; nothing to watch");
        return ExitCode::FAILURE;
    }

    // The loop checks this between naps rather than dying inside a handler,
    // so a stop always unwinds through the normal path.
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT, SIGHUP] {
        if let Err(err) = signal_hook::flag::register(signal, Arc::clone(&stop)) {
            log.info(format!("could not handle signal {signal}: {err}"));
        }
    }

    let probe = probe::Probe::new(
        &log,
        http::UreqHttp::new(config.probe_connect_timeout, config.probe_timeout),
        config.probe_connect_timeout,
        config.probe_timeout,
    );

    // One timeout for the whole CDP round trip, and it is the *connect*
    // timeout: DevTools is on the loopback, so an answer is either immediate
    // or never coming, and waiting out the probe's read timeout would only
    // delay noticing a wedged browser.
    let cdp = cdp::CdpClient::new(
        &log,
        &config.cdp_url,
        http::UreqHttp::new(config.probe_connect_timeout, config.probe_connect_timeout),
        config.probe_connect_timeout,
    );

    let units = systemd::Systemd::new(&config.unit, &log);
    let offline = offline::Offline::new(&log, &config);

    agent::Agent::new(&config, &log, &probe, &cdp, &units, &offline, stop).run();

    ExitCode::SUCCESS
}
