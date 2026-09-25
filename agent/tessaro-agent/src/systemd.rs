//! The one D-Bus connection this program owns: `org.freedesktop.systemd1` on
//! the system bus, used to restart the browser unit. Everything goes through
//! the bus - there are no `systemctl` shell-outs.
//!
//! Every query is safe: bus trouble degrades to the answers a caller would
//! get for a stopped unit, and `restart` is the only call that fails, so the
//! caller decides what a failed restart means. That is what lets the same
//! binary run in the integration setup, where there is no system bus at all.
//!
//! Every call has a deadline of ours and one of zbus's, and they are not
//! redundant. `within()` bounds *our* wait and pledges it to the watchdog;
//! zbus's own `method_timeout` expires the pending call inside zbus and frees
//! its slot, and bounds the calls zbus makes on its own behalf that `within()`
//! never sees. Before this, none of these calls had any deadline at all, and
//! `main_pid()` runs on every cycle.

use std::cell::RefCell;
use std::time::Duration;

use async_trait::async_trait;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Connection;

use crate::error::{Error, Result};
use crate::log::Log;
use crate::ports::Units;
use crate::watchdog::Heartbeat;

const SERVICE: &str = "org.freedesktop.systemd1";
const IFACE_UNIT: &str = "org.freedesktop.systemd1.Unit";
const IFACE_SERVICE: &str = "org.freedesktop.systemd1.Service";

/// Every call here is to pid 1 over a unix socket and answers in milliseconds
/// or not at all, so this is not a tuning parameter - past it, the bus is
/// broken. Which is also why it is not a setting.
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn restart_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn try_restart_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn reboot(&self) -> zbus::Result<()>;
}

pub struct Systemd<'a> {
    unit: String,
    log: &'a Log,
    heartbeat: Heartbeat,
    connection: Option<Connection>,
    /// The unit's object path, looked up once. systemd derives it from the
    /// unit name and resolves it even for a unit that is not loaded, so it
    /// does not change across restarts - and caching it halves the round
    /// trips of every `active_state()` and `main_pid()`. Dropped on any error.
    unit_path: RefCell<Option<OwnedObjectPath>>,
}

impl<'a> Systemd<'a> {
    pub async fn connect(unit: &str, log: &'a Log, heartbeat: Heartbeat) -> Self {
        let build = async {
            zbus::connection::Builder::system()?
                .method_timeout(METHOD_TIMEOUT)
                .build()
                .await // naked: bounded by the "the system bus" within() below
        };

        let connection = match heartbeat
            .within("the system bus", METHOD_TIMEOUT, build)
            .await
        {
            Ok(Ok(connection)) => Some(connection),
            Ok(Err(err)) => {
                log.info(format!("no system bus: {err}"));
                None
            }
            Err(expired) => {
                log.info(format!("no system bus: {expired}"));
                None
            }
        };

        Self {
            unit: unit.to_string(),
            log,
            heartbeat,
            connection,
            unit_path: RefCell::new(None),
        }
    }

    #[cfg(test)]
    fn without_bus(unit: &str, log: &'a Log) -> Self {
        Self {
            unit: unit.to_string(),
            log,
            heartbeat: Heartbeat::detached(),
            connection: None,
            unit_path: RefCell::new(None),
        }
    }

    fn connection(&self) -> Result<&Connection> {
        self.connection
            .as_ref()
            .ok_or_else(|| Error::Systemd("no system bus".to_string()))
    }

    async fn manager(&self) -> Result<ManagerProxy<'_>> {
        let connection = self.connection()?;
        let build = ManagerProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build();

        flatten(
            self.heartbeat
                .within("systemd1", METHOD_TIMEOUT, build)
                .await,
        )
        .map_err(|err| Error::Systemd(format!("systemd1 is unreachable: {err}")))
    }

    async fn unit_path(&self) -> Result<OwnedObjectPath> {
        if let Some(path) = self.unit_path.borrow().clone() {
            return Ok(path);
        }

        let manager = self.manager().await?;
        let path = flatten(
            self.heartbeat
                .within("GetUnit", METHOD_TIMEOUT, manager.get_unit(&self.unit))
                .await,
        )
        .map_err(|err| Error::Systemd(format!("GetUnit({}) failed: {err}", self.unit)))?;

        *self.unit_path.borrow_mut() = Some(path.clone());
        Ok(path)
    }

    /// One property off the unit object. `MainPID` lives on the Service
    /// interface and `ActiveState` on Unit - different interfaces on the same
    /// object, which is the easy thing to get wrong here.
    async fn unit_property(&self, interface: &str, property: &str) -> Result<OwnedValue> {
        let outcome = self.fetch_property(interface, property).await;
        if outcome.is_err() {
            self.unit_path.borrow_mut().take();
        }
        outcome
    }

    async fn fetch_property(&self, interface: &str, property: &str) -> Result<OwnedValue> {
        let connection = self.connection()?;
        let path = self.unit_path().await?;

        let build = async {
            PropertiesProxy::builder(connection)
                .destination(SERVICE)?
                .path(path)?
                .cache_properties(CacheProperties::No)
                .build()
                .await // naked: bounded by the "properties proxy" within() below
        };
        let properties = flatten(
            self.heartbeat
                .within("properties proxy", METHOD_TIMEOUT, build)
                .await,
        )
        .map_err(|err| Error::Systemd(format!("properties proxy failed: {err}")))?;

        let interface = InterfaceName::try_from(interface)
            .map_err(|err| Error::Systemd(format!("bad interface name: {err}")))?;

        let value = self
            .heartbeat
            .within(
                "Properties.Get",
                METHOD_TIMEOUT,
                properties.get(interface, property),
            )
            .await;

        match value {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(Error::Systemd(format!("{property} query failed: {err}"))),
            Err(expired) => Err(Error::Systemd(format!(
                "{property} query failed: {expired}"
            ))),
        }
    }
}

#[async_trait(?Send)]
impl Units for Systemd<'_> {
    async fn active_state(&self) -> String {
        match self.unit_property(IFACE_UNIT, "ActiveState").await {
            Ok(value) => String::try_from(value).unwrap_or_else(|_| "inactive".to_string()),
            Err(err) => {
                self.log.debug(err);
                "inactive".to_string()
            }
        }
    }

    async fn main_pid(&self) -> u32 {
        match self.unit_property(IFACE_SERVICE, "MainPID").await {
            Ok(value) => u32::try_from(value).unwrap_or(0),
            Err(err) => {
                self.log.debug(err);
                0
            }
        }
    }

    async fn restart(&self) -> Result<()> {
        let manager = self.manager().await?;

        flatten(
            self.heartbeat
                .within(
                    "RestartUnit",
                    METHOD_TIMEOUT,
                    manager.restart_unit(&self.unit, "replace"),
                )
                .await,
        )
        .map_err(|err| Error::Systemd(format!("RestartUnit({}) failed: {err}", self.unit)))?;

        Ok(())
    }
}

/// The control plane's own handle on systemd: any unit, and reboot.
///
/// Separate from `Systemd` on purpose. That one belongs to the state machine
/// and pledges every call to the watchdog; a request from `tessaro-ctl` is
/// not the state machine, and must never be what keeps the watchdog fed. So
/// this one bounds its calls with plain `deadline::within`, on a connection
/// of its own.
pub struct Bus {
    connection: Option<Connection>,
}

impl Bus {
    pub async fn connect(log: &Log) -> Self {
        let build = async {
            zbus::connection::Builder::system()?
                .method_timeout(METHOD_TIMEOUT)
                .build()
                .await // naked: bounded by the "the system bus" within() below
        };

        let connection =
            match crate::deadline::within("the system bus", METHOD_TIMEOUT, build).await {
                Ok(Ok(connection)) => Some(connection),
                Ok(Err(err)) => {
                    log.info(format!("control: no system bus: {err}"));
                    None
                }
                Err(expired) => {
                    log.info(format!("control: no system bus: {expired}"));
                    None
                }
            };

        Self { connection }
    }

    #[cfg(test)]
    pub fn none() -> Self {
        Self { connection: None }
    }

    async fn manager(&self) -> Result<ManagerProxy<'_>> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::Systemd("no system bus".to_string()))?;
        let build = ManagerProxy::builder(connection)
            .cache_properties(CacheProperties::No)
            .build();

        flatten(crate::deadline::within("systemd1", METHOD_TIMEOUT, build).await)
            .map_err(|err| Error::Systemd(format!("systemd1 is unreachable: {err}")))
    }

    pub async fn restart(&self, unit: &str) -> Result<()> {
        let manager = self.manager().await?;
        flatten(
            crate::deadline::within(
                "RestartUnit",
                METHOD_TIMEOUT,
                manager.restart_unit(unit, "replace"),
            )
            .await,
        )
        .map_err(|err| Error::Systemd(format!("RestartUnit({unit}) failed: {err}")))?;
        Ok(())
    }

    /// Restart `unit` if it is running; leave it stopped if it is not.
    pub async fn try_restart(&self, unit: &str) -> Result<()> {
        let manager = self.manager().await?;
        flatten(
            crate::deadline::within(
                "TryRestartUnit",
                METHOD_TIMEOUT,
                manager.try_restart_unit(unit, "replace"),
            )
            .await,
        )
        .map_err(|err| Error::Systemd(format!("TryRestartUnit({unit}) failed: {err}")))?;
        Ok(())
    }

    /// The system bus connection, for the other services the control plane
    /// talks to (timedated, timesyncd). `None` without a bus.
    pub fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()
    }

    pub async fn reboot(&self) -> Result<()> {
        let manager = self.manager().await?;
        flatten(crate::deadline::within("Reboot", METHOD_TIMEOUT, manager.reboot()).await)
            .map_err(|err| Error::Systemd(format!("Reboot failed: {err}")))
    }

    /// `active`, `inactive`, `failed`, ... or `unknown` when the bus cannot
    /// say.
    pub async fn active_state(&self, unit: &str) -> String {
        match self.fetch_active_state(unit).await {
            Ok(state) => state,
            Err(_) => "unknown".to_string(),
        }
    }

    async fn fetch_active_state(&self, unit: &str) -> Result<String> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::Systemd("no system bus".to_string()))?;
        let manager = self.manager().await?;
        let path = flatten(
            crate::deadline::within("GetUnit", METHOD_TIMEOUT, manager.get_unit(unit)).await,
        )
        .map_err(Error::Systemd)?;

        let build = async {
            PropertiesProxy::builder(connection)
                .destination(SERVICE)?
                .path(path)?
                .cache_properties(CacheProperties::No)
                .build()
                .await // naked: bounded by the "properties proxy" within() below
        };
        let properties =
            flatten(crate::deadline::within("properties proxy", METHOD_TIMEOUT, build).await)
                .map_err(Error::Systemd)?;

        let interface = InterfaceName::try_from(IFACE_UNIT)
            .map_err(|err| Error::Systemd(format!("bad interface name: {err}")))?;
        let value = flatten(
            crate::deadline::within(
                "Properties.Get",
                METHOD_TIMEOUT,
                properties.get(interface, "ActiveState"),
            )
            .await
            .map(|outcome| outcome.map_err(zbus::Error::from)),
        )
        .map_err(Error::Systemd)?;

        String::try_from(value).map_err(|err| Error::Systemd(err.to_string()))
    }
}

/// A deadline around a zbus call, as one error in the journal's words.
fn flatten<T>(
    outcome: std::result::Result<zbus::Result<T>, crate::deadline::Expired>,
) -> std::result::Result<T, String> {
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(expired) => Err(expired.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_bus_every_query_answers_as_if_the_unit_were_stopped() {
        let log = Log::buffered(true);
        let systemd = Systemd::without_bus("tessaro-kiosk.service", &log);

        assert_eq!(systemd.active_state().await, "inactive");
        assert_eq!(systemd.main_pid().await, 0);
        assert!(systemd.restart().await.is_err());
    }

    /// Needs a real system bus, so it is not part of the default run:
    ///     cargo test -- --ignored
    /// It is also the smoke test for zbus on a current-thread runtime, which
    /// is what production runs.
    #[tokio::test]
    #[ignore = "requires a system bus"]
    async fn reads_a_real_unit() {
        let log = Log::buffered(true);
        let systemd =
            Systemd::connect("systemd-journald.service", &log, Heartbeat::detached()).await;

        assert_eq!(systemd.active_state().await, "active");
        assert!(systemd.main_pid().await > 0);
        // Second time round the object path comes from the cache.
        assert!(systemd.main_pid().await > 0);
    }

    #[tokio::test]
    #[ignore = "requires a system bus"]
    async fn a_missing_unit_looks_stopped() {
        let log = Log::buffered(true);
        let systemd =
            Systemd::connect("definitely-not-a-unit.service", &log, Heartbeat::detached()).await;

        assert_eq!(systemd.active_state().await, "inactive");
        assert_eq!(systemd.main_pid().await, 0);
        // Marshalling and interface lookup are exercised even though systemd
        // answers NoSuchUnit.
        assert!(systemd.restart().await.is_err());
    }
}
