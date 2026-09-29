//! The clock: the time.* settings applied through systemd-timedated and
//! systemd-timesyncd, and everything they report about it.
//!
//! Nothing here measures time on its own. Offsets, delays, jitter and drift
//! are what timesyncd reports over D-Bus (`NTPMessage`, `Frequency`), put
//! together the way `timedatectl timesync-status` does. Applying is
//! idempotent and compares against what the services report, not against
//! files, so it can run once a minute and act only when something moved:
//!
//! * **Servers**: time.ntp.servers goes into a drop-in in `/run`
//!   (`Paths::timesyncd_dropin`), and timesyncd is restarted when its
//!   `SystemNTPServers` differ - the one thing that restarts.
//! * **DHCP's servers**: timesyncd learns per-link servers only from
//!   systemd-networkd, which the image does not have. While time.ntp.servers
//!   is empty the agent hands NetworkManager's `ntp_servers` DHCP option to
//!   timesyncd as its runtime servers. Runtime servers win over every other
//!   kind (`timesyncd-manager.c`), so with servers set they are cleared.
//! * **Sync on/off**: timedated's `SetNTP`, which starts and enables
//!   timesyncd, or stops and disables it.
//! * **Timezone**: timedated's `SetTimezone`, which points `/etc/localtime`
//!   at the zone. That lands on the `/etc` overlay; the settings stay the
//!   source of truth, reapplied at every start. Chromium and glibc follow
//!   `/etc/localtime` without a restart.
//!
//! timesync1 is D-Bus activatable, so asking it anything starts timesyncd.
//! It is only asked while its unit is active, or `time.ntp.enable=0` would
//! not stay off.

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;
use zbus::Connection;

use protocol::keys::{self, DEFAULT_TIMEZONE};
use protocol::{NtpSample, NtpServers, TimeStatus, TimeSummary};

use crate::config::Env;
use crate::deadline::{blocking, within_result};
use crate::log::Log;
use crate::nm::proxy::{DeviceProxy, Dhcp4ConfigProxy, ManagerProxy as NmManagerProxy};
use crate::paths::Paths;
use crate::systemd::Bus;

/// timedated, timesyncd and NetworkManager answer in milliseconds; past this
/// the bus is the problem. The same budget as every systemd call
/// (`systemd.rs`), which is also the connection's own method timeout.
const CALL: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.freedesktop.timedate1",
    default_service = "org.freedesktop.timedate1",
    default_path = "/org/freedesktop/timedate1"
)]
trait Timedate {
    fn set_timezone(&self, timezone: &str, interactive: bool) -> zbus::Result<()>;
    #[zbus(name = "SetNTP")]
    fn set_ntp(&self, use_ntp: bool, interactive: bool) -> zbus::Result<()>;
    fn set_time(&self, usec_utc: i64, relative: bool, interactive: bool) -> zbus::Result<()>;
    fn list_timezones(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn timezone(&self) -> zbus::Result<String>;
    #[zbus(property, name = "LocalRTC")]
    fn local_rtc(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "CanNTP")]
    fn can_ntp(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "NTP")]
    fn ntp(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "NTPSynchronized")]
    fn ntp_synchronized(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "TimeUSec")]
    fn time_usec(&self) -> zbus::Result<u64>;
    #[zbus(property, name = "RTCTimeUSec")]
    fn rtc_time_usec(&self) -> zbus::Result<u64>;
}

/// `NTPMessage`, `(uuuuittayttttbtt)`: leap, version, mode, stratum,
/// precision, root delay, root dispersion, reference, then the origin,
/// receive, transmit and destination timestamps, spike, packet count and
/// jitter. Timestamps are microseconds of the realtime clock.
type Message = (
    u32,
    u32,
    u32,
    u32,
    i32,
    u64,
    u64,
    Vec<u8>,
    u64,
    u64,
    u64,
    u64,
    bool,
    u64,
    u64,
);

#[zbus::proxy(
    interface = "org.freedesktop.timesync1.Manager",
    default_service = "org.freedesktop.timesync1",
    default_path = "/org/freedesktop/timesync1"
)]
trait Timesync {
    #[zbus(name = "SetRuntimeNTPServers")]
    fn set_runtime_ntp_servers(&self, runtime_servers: &[&str]) -> zbus::Result<()>;

    #[zbus(property, name = "LinkNTPServers")]
    fn link_ntp_servers(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property, name = "SystemNTPServers")]
    fn system_ntp_servers(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property, name = "RuntimeNTPServers")]
    fn runtime_ntp_servers(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property, name = "FallbackNTPServers")]
    fn fallback_ntp_servers(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn server_name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn server_address(&self) -> zbus::Result<(i32, Vec<u8>)>;
    #[zbus(property, name = "PollIntervalUSec")]
    fn poll_interval_usec(&self) -> zbus::Result<u64>;
    #[zbus(property, name = "PollIntervalMinUSec")]
    fn poll_interval_min_usec(&self) -> zbus::Result<u64>;
    #[zbus(property, name = "PollIntervalMaxUSec")]
    fn poll_interval_max_usec(&self) -> zbus::Result<u64>;
    #[zbus(property, name = "RootDistanceMaxUSec")]
    fn root_distance_max_usec(&self) -> zbus::Result<u64>;
    #[zbus(property, name = "NTPMessage")]
    fn ntp_message(&self) -> zbus::Result<Message>;
    #[zbus(property)]
    fn frequency(&self) -> zbus::Result<i64>;
}

/// What the time.* settings ask for, set or image default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub timezone: String,
    pub ntp: bool,
    pub servers: Vec<String>,
}

impl Wanted {
    pub fn from_env(env: &dyn Env) -> Self {
        let timezone = env
            .get("KIOSK_TIMEZONE")
            .map(|zone| zone.trim().to_string())
            .filter(|zone| keys::is_timezone(zone))
            .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
        Self {
            timezone,
            ntp: env.get("KIOSK_NTP").as_deref().map(str::trim) != Some("0"),
            servers: keys::parse_hosts(&env.get("KIOSK_NTP_SERVERS").unwrap_or_default())
                .unwrap_or_default(),
        }
    }
}

/// What one `apply` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Each change made, in words; empty when everything already held.
    pub changes: Vec<String>,
    /// The state the clock is left in, for `Applied.time`.
    pub summary: String,
}

pub struct Time {
    log: Arc<Log>,
    manage: bool,
    unit: String,
    dropin: PathBuf,
}

impl Time {
    pub fn new(log: Arc<Log>, paths: &Paths) -> Arc<Self> {
        Arc::new(Self {
            log,
            manage: paths.manage_clock,
            unit: paths.timesyncd_unit.clone(),
            dropin: paths.timesyncd_dropin.clone(),
        })
    }

    /// Bring the running system to `wanted`. Idempotent: every step compares
    /// first and acts only on a difference.
    pub async fn apply(&self, bus: &Bus, wanted: &Wanted) -> Result<Outcome, String> {
        if !self.manage {
            return Ok(Outcome {
                changes: Vec::new(),
                summary: "saved; this host's clock is not managed (KIOSK_MANAGE_CLOCK=0)"
                    .to_string(),
            });
        }
        let connection = connection(bus)?;
        let timedate = timedate(connection).await?;
        let mut changes = Vec::new();

        // The drop-in first, so a timesyncd that SetNTP starts below reads
        // the servers wanted.
        let dropin = self.dropin.clone();
        let body = dropin_body(&wanted.servers);
        let rewritten = blocking("writing the timesyncd drop-in", move || {
            write_dropin(&dropin, body.as_deref())
        })
        .await?;

        let can_ntp = property(timedate.can_ntp(), "CanNTP").await.unwrap_or(true);
        let ntp = property(timedate.ntp(), "NTP").await?;
        if ntp != wanted.ntp && (can_ntp || !wanted.ntp) {
            within_result("SetNTP", CALL, timedate.set_ntp(wanted.ntp, false)).await?;
            changes.push(format!("NTP {}", on_off(wanted.ntp)));
        }

        let zone = property(timedate.timezone(), "Timezone").await?;
        if zone != wanted.timezone {
            within_result(
                "SetTimezone",
                CALL,
                timedate.set_timezone(&wanted.timezone, false),
            )
            .await
            .map_err(|err| format!("the timezone {} was not set: {err}", wanted.timezone))?;
            changes.push(format!("timezone {}", wanted.timezone));
        }

        if wanted.ntp && can_ntp && bus.active_state(&self.unit).await == "active" {
            let timesync = timesync(connection).await?;
            let system = property(timesync.system_ntp_servers(), "SystemNTPServers").await?;
            let restarted = if rewritten || system != wanted.servers {
                bus.try_restart(&self.unit)
                    .await
                    .map_err(|err| err.to_string())?;
                changes.push(format!("restarted {}", self.unit));
                true
            } else {
                false
            };

            let runtime = if wanted.servers.is_empty() {
                dhcp_servers(connection).await
            } else {
                Vec::new()
            };
            // A restarted timesyncd starts with no runtime servers: only
            // worth telling it when there are some.
            let current = if restarted {
                Vec::new()
            } else {
                property(timesync.runtime_ntp_servers(), "RuntimeNTPServers").await?
            };
            if current != runtime {
                let names: Vec<&str> = runtime.iter().map(String::as_str).collect();
                within_result(
                    "SetRuntimeNTPServers",
                    CALL,
                    timesync.set_runtime_ntp_servers(&names),
                )
                .await?;
                changes.push(if runtime.is_empty() {
                    "cleared DHCP's NTP servers".to_string()
                } else {
                    format!("DHCP's NTP servers {}", runtime.join(" "))
                });
            }
        }

        if !changes.is_empty() {
            self.log.info(format!("time: {}", changes.join(", ")));
        }
        Ok(Outcome {
            changes,
            summary: summary(wanted),
        })
    }

    /// Everything timedated and timesyncd report. Never fails: what cannot
    /// be read is `None`, and `error` says why.
    pub async fn status(&self, bus: &Bus, wanted: &Wanted) -> TimeStatus {
        let mut status = empty_status(wanted);
        let Some(connection) = bus.connection() else {
            status.error = Some("no system bus".to_string());
            return status;
        };
        let mut errors = Vec::new();

        match timedate(connection).await {
            Ok(timedate) => {
                status.timezone = property(timedate.timezone(), "Timezone").await.ok();
                status.local_rtc = property(timedate.local_rtc(), "LocalRTC").await.ok();
                status.can_ntp = property(timedate.can_ntp(), "CanNTP").await.ok();
                status.ntp = property(timedate.ntp(), "NTP").await.ok();
                status.synchronized = property(timedate.ntp_synchronized(), "NTPSynchronized")
                    .await
                    .ok();
                status.now_usec = property(timedate.time_usec(), "TimeUSec").await.ok();
                // 0 on a board without a hardware clock, or one that
                // cannot be read.
                status.rtc_usec = property(timedate.rtc_time_usec(), "RTCTimeUSec")
                    .await
                    .ok()
                    .filter(|usec| *usec > 0);
            }
            Err(err) => errors.push(err),
        }
        if let Some(now) = status.now_usec {
            if let Ok(Some(local)) =
                blocking("reading the local time", move || Ok(local_clock(now))).await
            {
                status.local_time = Some(local.text);
                status.zone_abbreviation = Some(local.abbreviation);
                status.utc_offset_seconds = Some(local.offset);
            }
        }

        let state = bus.active_state(&self.unit).await;
        let active = state == "active";
        status.timesyncd = Some(state);
        status.servers.dhcp = dhcp_servers(connection).await;
        if active {
            match timesync(connection).await {
                Ok(timesync) => self.read_timesync(&timesync, &mut status).await,
                Err(err) => errors.push(err),
            }
        }

        if !errors.is_empty() {
            status.error = Some(errors.join("; "));
        }
        status
    }

    async fn read_timesync(&self, timesync: &TimesyncProxy<'_>, status: &mut TimeStatus) {
        let list = |servers: Result<Vec<String>, String>| servers.unwrap_or_default();
        status.servers.runtime =
            list(property(timesync.runtime_ntp_servers(), "RuntimeNTPServers").await);
        status.servers.system =
            list(property(timesync.system_ntp_servers(), "SystemNTPServers").await);
        status.servers.link = list(property(timesync.link_ntp_servers(), "LinkNTPServers").await);
        status.servers.fallback =
            list(property(timesync.fallback_ntp_servers(), "FallbackNTPServers").await);
        status.server_name = property(timesync.server_name(), "ServerName")
            .await
            .ok()
            .filter(|name| !name.is_empty());
        status.server_address = property(timesync.server_address(), "ServerAddress")
            .await
            .ok()
            .and_then(|(family, bytes)| address(family, &bytes));
        status.poll_interval_usec = property(timesync.poll_interval_usec(), "PollIntervalUSec")
            .await
            .ok()
            .filter(|usec| *usec > 0);
        status.poll_interval_min_usec =
            property(timesync.poll_interval_min_usec(), "PollIntervalMinUSec")
                .await
                .ok();
        status.poll_interval_max_usec =
            property(timesync.poll_interval_max_usec(), "PollIntervalMaxUSec")
                .await
                .ok();
        status.root_distance_max_usec =
            property(timesync.root_distance_max_usec(), "RootDistanceMaxUSec")
                .await
                .ok();
        status.last = property(timesync.ntp_message(), "NTPMessage")
            .await
            .ok()
            .and_then(sample);
        // Frequency is only worth showing once there has been an answer:
        // before that it is whatever the kernel booted with.
        if status.last.is_some() {
            status.frequency = property(timesync.frequency(), "Frequency").await.ok();
        }
    }

    /// The timezone and sync state only: cheap enough for `device status`.
    pub async fn summary(&self, bus: &Bus) -> Option<TimeSummary> {
        let timedate = timedate(bus.connection()?).await.ok()?;
        Some(TimeSummary {
            timezone: property(timedate.timezone(), "Timezone").await.ok(),
            synchronized: property(timedate.ntp_synchronized(), "NTPSynchronized")
                .await
                .ok(),
            ntp: property(timedate.ntp(), "NTP").await.ok(),
        })
    }

    /// Every zone timedated knows: the ones `SetTimezone` takes.
    pub async fn zones(&self, bus: &Bus) -> Result<Vec<String>, String> {
        let timedate = timedate(connection(bus)?).await?;
        within_result("ListTimezones", CALL, timedate.list_timezones()).await
    }

    /// For `config set`: a zone this device can switch to.
    pub async fn check_zone(&self, bus: &Bus, zone: &str) -> Result<(), String> {
        let zones = self
            .zones(bus)
            .await
            .map_err(|err| format!("{zone} cannot be checked: {err}"))?;
        if zones.iter().any(|known| known == zone) {
            Ok(())
        } else {
            Err(format!(
                "this device has no timezone {zone}; `tessaro-ctl time zones` lists them"
            ))
        }
    }

    /// Restart timesyncd so it asks its servers now.
    pub async fn sync(&self, bus: &Bus) -> Result<String, String> {
        if !self.manage {
            return Err("this host's clock is not managed (KIOSK_MANAGE_CLOCK=0)".to_string());
        }
        let timedate = timedate(connection(bus)?).await?;
        if !property(timedate.ntp(), "NTP").await? {
            return Err(
                "NTP is off; switch it on with `tessaro-ctl time ntp on`, or set the clock with `tessaro-ctl time set`"
                    .to_string(),
            );
        }
        bus.restart(&self.unit)
            .await
            .map_err(|err| err.to_string())?;
        self.log
            .info(format!("time: restarted {} to sync now", self.unit));
        Ok(format!("{} restarted; it asks its servers now", self.unit))
    }

    /// Set the clock by hand: to `usec` since the epoch, or to `local`, a
    /// wall-clock time in the device's timezone.
    pub async fn set_clock(
        &self,
        bus: &Bus,
        who: &str,
        usec: Option<u64>,
        local: Option<String>,
    ) -> Result<String, String> {
        if !self.manage {
            return Err("this host's clock is not managed (KIOSK_MANAGE_CLOCK=0)".to_string());
        }
        let usec = match (usec, local) {
            (Some(usec), None) => usec,
            (None, Some(local)) => {
                let fields = protocol::parse_local_time(&local)?;
                blocking("reading the local time", move || {
                    from_local(fields)
                        .ok_or_else(|| format!("{local} does not exist in this device's timezone"))
                })
                .await?
            }
            _ => return Err("give either a time or microseconds since the epoch".to_string()),
        };
        let timedate = timedate(connection(bus)?).await?;
        if property(timedate.ntp(), "NTP").await? {
            return Err(
                "the clock is kept by NTP; switch it off first with `tessaro-ctl time ntp off`"
                    .to_string(),
            );
        }
        let usec_utc = i64::try_from(usec).map_err(|_| "the time is out of range".to_string())?;
        within_result("SetTime", CALL, timedate.set_time(usec_utc, false, false)).await?;
        let shown = blocking("reading the local time", move || Ok(local_clock(usec)))
            .await
            .ok()
            .flatten()
            .map(|local| format!("{} {}", local.text, local.abbreviation))
            .unwrap_or_else(|| format!("{usec} µs since the epoch"));
        self.log
            .info(format!("time: clock set to {shown} by {who}"));
        Ok(format!("the clock is now {shown}"))
    }
}

fn connection(bus: &Bus) -> Result<&Connection, String> {
    bus.connection().ok_or_else(|| "no system bus".to_string())
}

async fn timedate(connection: &Connection) -> Result<TimedateProxy<'_>, String> {
    within_result(
        "timedate1",
        CALL,
        TimedateProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build(),
    )
    .await
}

async fn timesync(connection: &Connection) -> Result<TimesyncProxy<'_>, String> {
    within_result(
        "timesync1",
        CALL,
        TimesyncProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build(),
    )
    .await
}

/// One property read, under the deadline, named for the error.
async fn property<T>(
    read: impl std::future::Future<Output = zbus::Result<T>>,
    what: &'static str,
) -> Result<T, String> {
    within_result(what, CALL, read).await
}

/// The NTP servers every NetworkManager device's DHCPv4 lease offers, in
/// order, without repeats. Empty when NetworkManager is not there or no
/// lease has any.
async fn dhcp_servers(connection: &Connection) -> Vec<String> {
    let mut servers: Vec<String> = Vec::new();
    let Ok(manager) = within_result(
        "NetworkManager",
        CALL,
        NmManagerProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build(),
    )
    .await
    else {
        return servers;
    };
    let Ok(devices) = property(manager.devices(), "Devices").await else {
        return servers;
    };
    for path in devices {
        let Ok(builder) = DeviceProxy::builder(connection).path(path) else {
            continue;
        };
        let Ok(device) = within_result(
            "NetworkManager device",
            CALL,
            builder.cache_properties(CacheProperties::No).build(),
        )
        .await
        else {
            continue;
        };
        let Ok(config) = property(device.dhcp4_config(), "Dhcp4Config").await else {
            continue;
        };
        if config.as_str() == "/" {
            continue;
        }
        let Ok(builder) = Dhcp4ConfigProxy::builder(connection).path(config) else {
            continue;
        };
        let Ok(lease) = within_result(
            "NetworkManager DHCP4Config",
            CALL,
            builder.cache_properties(CacheProperties::No).build(),
        )
        .await
        else {
            continue;
        };
        let Ok(options) = property(lease.options(), "Options").await else {
            continue;
        };
        for server in ntp_option(&options) {
            if !servers.contains(&server) {
                servers.push(server);
            }
        }
    }
    servers
}

/// The `ntp_servers` DHCP option, space separated, as timesyncd would take
/// the names: only valid hosts, lower-cased.
fn ntp_option(options: &HashMap<String, OwnedValue>) -> Vec<String> {
    options
        .get("ntp_servers")
        .and_then(|value| String::try_from(value.clone()).ok())
        .map(|text| keys::parse_hosts(&text).unwrap_or_default())
        .unwrap_or_default()
}

/// The drop-in for time.ntp.servers, or `None` to have none and leave
/// timesyncd to DHCP's servers and its fallback.
fn dropin_body(servers: &[String]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    Some(format!(
        "# Written by tessaro-agent from time.ntp.servers; `tessaro-ctl time show`.\n\
         [Time]\n\
         NTP={}\n",
        servers.join(" ")
    ))
}

/// Write or remove the drop-in; whether anything changed.
fn write_dropin(path: &std::path::Path, body: Option<&str>) -> Result<bool, String> {
    match body {
        Some(body) => crate::store::replace_if_changed(path, body.as_bytes(), 0o644)
            .map_err(|err| format!("{}: {err}", path.display())),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(format!("{}: {err}", path.display())),
        },
    }
}

fn summary(wanted: &Wanted) -> String {
    let servers = if !wanted.ntp {
        String::new()
    } else if wanted.servers.is_empty() {
        ", servers from DHCP or the fallback".to_string()
    } else {
        format!(", servers {}", wanted.servers.join(" "))
    };
    format!(
        "timezone {}, NTP {}{servers}",
        wanted.timezone,
        on_off(wanted.ntp)
    )
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn empty_status(wanted: &Wanted) -> TimeStatus {
    TimeStatus {
        error: None,
        setting_timezone: wanted.timezone.clone(),
        setting_servers: wanted.servers.clone(),
        timezone: None,
        now_usec: None,
        local_time: None,
        zone_abbreviation: None,
        utc_offset_seconds: None,
        rtc_usec: None,
        local_rtc: None,
        can_ntp: None,
        ntp: None,
        synchronized: None,
        timesyncd: None,
        server_name: None,
        server_address: None,
        poll_interval_usec: None,
        poll_interval_min_usec: None,
        poll_interval_max_usec: None,
        root_distance_max_usec: None,
        frequency: None,
        last: None,
        servers: NtpServers::default(),
    }
}

/// `NTPMessage` as a sample, or `None` before the first answer.
fn sample(message: Message) -> Option<NtpSample> {
    let (
        leap,
        version,
        _mode,
        stratum,
        precision,
        root_delay,
        root_dispersion,
        reference,
        origin,
        receive,
        transmit,
        destination,
        spike,
        packet_count,
        jitter,
    ) = message;
    // timedatectl's own sanity check: an answer that cannot be ordered
    // in time is not one.
    if packet_count == 0
        || destination < origin
        || transmit < receive
        || destination - origin < transmit - receive
    {
        return None;
    }
    let (offset_usec, delay_usec) =
        NtpSample::offset_and_delay(origin, receive, transmit, destination);
    Some(NtpSample {
        leap,
        version,
        stratum,
        precision,
        root_delay_usec: root_delay,
        root_dispersion_usec: root_dispersion,
        reference: reference_id(stratum, &reference),
        offset_usec,
        delay_usec,
        jitter_usec: jitter,
        packet_count,
        spike,
        received_usec: destination,
    })
}

/// The reference ID as timedatectl shows it: a clock's name at stratum 0 or
/// 1 (`GPS`, `PPS`), otherwise the four bytes in hex - an upstream IPv4
/// address, or a hash of an IPv6 one.
fn reference_id(stratum: u32, bytes: &[u8]) -> String {
    if stratum <= 1 {
        bytes
            .iter()
            .take_while(|byte| **byte != 0)
            .filter(|byte| byte.is_ascii_graphic())
            .map(|byte| *byte as char)
            .collect()
    } else {
        bytes.iter().map(|byte| format!("{byte:02X}")).collect()
    }
}

/// `ServerAddress`, `(iay)`: an address family and its bytes.
fn address(family: i32, bytes: &[u8]) -> Option<String> {
    match (family, bytes.len()) {
        (libc::AF_INET, 4) => {
            let octets: [u8; 4] = bytes.try_into().ok()?;
            Some(std::net::Ipv4Addr::from(octets).to_string())
        }
        (libc::AF_INET6, 16) => {
            let octets: [u8; 16] = bytes.try_into().ok()?;
            Some(std::net::Ipv6Addr::from(octets).to_string())
        }
        _ => None,
    }
}

extern "C" {
    /// POSIX `tzset(3)`, which the libc crate does not bind.
    fn tzset();
}

/// A moment as the device's wall clock shows it.
pub(crate) struct Local {
    /// `2026-09-25 14:03:12`.
    pub(crate) text: String,
    /// `CEST`.
    pub(crate) abbreviation: String,
    /// Seconds east of UTC.
    offset: i32,
}

/// `usec` in the timezone `/etc/localtime` names now. glibc re-reads that
/// file on `tzset` when it changed, so a new timezone shows at once.
/// Reads a file: call it from `blocking`.
pub(crate) fn local_clock(usec: u64) -> Option<Local> {
    let seconds = libc::time_t::try_from(usec / 1_000_000).ok()?;
    // SAFETY: tzset and localtime_r only read the process environment and
    // /etc/localtime; `tm` is a plain C struct localtime_r fills in, and
    // tm_zone points into glibc's static zone data, copied at once.
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut tm).is_null() {
            return None;
        }
        let abbreviation = if tm.tm_zone.is_null() {
            String::new()
        } else {
            CStr::from_ptr(tm.tm_zone).to_string_lossy().into_owned()
        };
        Some(Local {
            text: format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            ),
            abbreviation,
            offset: i32::try_from(tm.tm_gmtoff).unwrap_or(0),
        })
    }
}

/// A wall-clock time in the device's timezone, as microseconds since the
/// epoch; `None` for one that does not exist there (skipped by a DST
/// change) or that mktime cannot place.
fn from_local([year, month, day, hour, minute, second]: [i32; 6]) -> Option<u64> {
    // SAFETY: as in `local_clock`; mktime reads `tm` and normalizes it.
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_sec = second;
        tm.tm_isdst = -1;
        let seconds = libc::mktime(&mut tm);
        // mktime moves a time that does not exist to one that does: a
        // day or an hour that changed is a time the zone skips.
        if seconds == -1 || tm.tm_mday != day || tm.tm_hour != hour {
            return None;
        }
        u64::try_from(seconds)
            .ok()
            .map(|seconds| seconds * 1_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn wanted_defaults_to_utc_with_ntp_on() {
        let wanted = Wanted::from_env(&env(&[]));
        assert_eq!(
            wanted,
            Wanted {
                timezone: "UTC".to_string(),
                ntp: true,
                servers: Vec::new()
            }
        );
        let wanted = Wanted::from_env(&env(&[
            ("KIOSK_TIMEZONE", "Europe/Bratislava"),
            ("KIOSK_NTP", "0"),
            ("KIOSK_NTP_SERVERS", "a.test,10.0.0.1"),
        ]));
        assert_eq!(wanted.timezone, "Europe/Bratislava");
        assert!(!wanted.ntp);
        assert_eq!(wanted.servers, ["a.test", "10.0.0.1"]);
        // A value that is not a zone never reaches SetTimezone.
        assert_eq!(
            Wanted::from_env(&env(&[("KIOSK_TIMEZONE", "../x")])).timezone,
            "UTC"
        );
    }

    #[test]
    fn the_dropin_carries_the_servers_or_is_not_there() {
        assert_eq!(dropin_body(&[]), None);
        let body = dropin_body(&["a.test".to_string(), "10.0.0.1".to_string()]).unwrap();
        assert!(body.contains("[Time]\nNTP=a.test 10.0.0.1\n"), "{body}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timesyncd.conf.d/50-tessaro.conf");
        assert!(write_dropin(&path, Some(&body)).unwrap());
        assert!(!write_dropin(&path, Some(&body)).unwrap());
        assert!(write_dropin(&path, None).unwrap());
        assert!(!write_dropin(&path, None).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn dhcp_offers_ntp_servers_as_a_space_separated_option() {
        let mut options = HashMap::new();
        options.insert(
            "ntp_servers".to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::from("10.0.0.1 Time.Test 10.0.0.1"))
                .unwrap(),
        );
        assert_eq!(ntp_option(&options), ["10.0.0.1", "time.test"]);
        assert!(ntp_option(&HashMap::new()).is_empty());
    }

    #[test]
    fn a_sample_is_what_timedatectl_would_show() {
        let message: Message = (
            0,
            4,
            4,
            2,
            -25,
            1_000,
            2_000,
            vec![0xC0, 0xA8, 0x01, 0x01],
            1_000_000,
            1_000_600,
            1_000_700,
            1_000_300,
            false,
            7,
            150,
        );
        let sample = sample(message).unwrap();
        assert_eq!(sample.offset_usec, 500);
        assert_eq!(sample.delay_usec, 200);
        assert_eq!(sample.reference, "C0A80101");
        assert_eq!(sample.stratum, 2);
        assert_eq!(sample.jitter_usec, 150);
        assert_eq!(sample.received_usec, 1_000_300);
    }

    #[test]
    fn no_answer_yet_is_no_sample() {
        let message: Message = (0, 0, 0, 0, 0, 0, 0, Vec::new(), 0, 0, 0, 0, false, 0, 0);
        assert!(sample(message).is_none());
    }

    #[test]
    fn stratum_one_references_are_clock_names() {
        assert_eq!(reference_id(1, b"GPS\0"), "GPS");
        assert_eq!(reference_id(3, &[10, 0, 0, 1]), "0A000001");
    }

    #[test]
    fn server_addresses_by_family() {
        assert_eq!(
            address(libc::AF_INET, &[10, 0, 0, 1]).as_deref(),
            Some("10.0.0.1")
        );
        let mut v6 = [0u8; 16];
        v6[15] = 1;
        assert_eq!(address(libc::AF_INET6, &v6).as_deref(), Some("::1"));
        assert_eq!(address(libc::AF_INET, &[1, 2]), None);
    }

    #[test]
    fn the_summary_names_what_is_in_force() {
        let mut wanted = Wanted::from_env(&env(&[]));
        assert_eq!(
            summary(&wanted),
            "timezone UTC, NTP on, servers from DHCP or the fallback"
        );
        wanted.servers = vec!["a.test".to_string()];
        assert_eq!(summary(&wanted), "timezone UTC, NTP on, servers a.test");
        wanted.ntp = false;
        assert_eq!(summary(&wanted), "timezone UTC, NTP off");
    }
}
