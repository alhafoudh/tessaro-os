//! The one D-Bus connection this program owns: `org.freedesktop.systemd1` on
//! the system bus, used to restart the browser unit. Everything goes through
//! the bus - there are no `systemctl` shell-outs.
//!
//! Every query is safe: bus trouble degrades to the answers a caller would
//! get for a stopped unit, and `restart` is the only call that fails, so the
//! caller decides what a failed restart means. That is what lets the same
//! binary run in the integration setup, where there is no system bus at all.

use zbus::blocking::fdo::PropertiesProxy;
use zbus::blocking::Connection;
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedObjectPath;

use crate::error::{Error, Result};
use crate::log::Log;
use crate::ports::Units;

const SERVICE: &str = "org.freedesktop.systemd1";
const IFACE_UNIT: &str = "org.freedesktop.systemd1.Unit";
const IFACE_SERVICE: &str = "org.freedesktop.systemd1.Service";

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1",
    // Blocking only. With gen_async off the macro drops the "Blocking"
    // suffix, so the generated type is ManagerProxy.
    gen_async = false
)]
trait Manager {
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn restart_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
}

pub struct Systemd<'a> {
    unit: String,
    log: &'a Log,
    connection: Option<Connection>,
}

impl<'a> Systemd<'a> {
    pub fn new(unit: &str, log: &'a Log) -> Self {
        let connection = match Connection::system() {
            Ok(connection) => Some(connection),
            Err(err) => {
                log.info(format!("no system bus: {err}"));
                None
            }
        };

        Self {
            unit: unit.to_string(),
            log,
            connection,
        }
    }

    #[cfg(test)]
    fn without_bus(unit: &str, log: &'a Log) -> Self {
        Self {
            unit: unit.to_string(),
            log,
            connection: None,
        }
    }

    fn manager(&self) -> Result<ManagerProxy<'_>> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::Systemd("no system bus".to_string()))?;

        ManagerProxy::new(connection)
            .map_err(|err| Error::Systemd(format!("systemd1 is unreachable: {err}")))
    }

    /// One property off the unit object. `MainPID` lives on the Service
    /// interface and `ActiveState` on Unit - different interfaces on the same
    /// object, which is the easy thing to get wrong here.
    fn unit_property(&self, interface: &str, property: &str) -> Result<zbus::zvariant::OwnedValue> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| Error::Systemd("no system bus".to_string()))?;

        let path = self
            .manager()?
            .get_unit(&self.unit)
            .map_err(|err| Error::Systemd(format!("GetUnit({}) failed: {err}", self.unit)))?;

        let properties = PropertiesProxy::builder(connection)
            .destination(SERVICE)
            .and_then(|builder| builder.path(path))
            .and_then(|builder| builder.build())
            .map_err(|err| Error::Systemd(format!("properties proxy failed: {err}")))?;

        let interface = InterfaceName::try_from(interface)
            .map_err(|err| Error::Systemd(format!("bad interface name: {err}")))?;

        properties
            .get(interface, property)
            .map_err(|err| Error::Systemd(format!("{property} query failed: {err}")))
    }
}

impl Units for Systemd<'_> {
    fn active_state(&self) -> String {
        match self.unit_property(IFACE_UNIT, "ActiveState") {
            Ok(value) => String::try_from(value).unwrap_or_else(|_| "inactive".to_string()),
            Err(err) => {
                self.log.debug(err);
                "inactive".to_string()
            }
        }
    }

    fn main_pid(&self) -> u32 {
        match self.unit_property(IFACE_SERVICE, "MainPID") {
            Ok(value) => u32::try_from(value).unwrap_or(0),
            Err(err) => {
                self.log.debug(err);
                0
            }
        }
    }

    fn restart(&self) -> Result<()> {
        self.manager()?
            .restart_unit(&self.unit, "replace")
            .map_err(|err| Error::Systemd(format!("RestartUnit({}) failed: {err}", self.unit)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_bus_every_query_answers_as_if_the_unit_were_stopped() {
        let log = Log::buffered(true);
        let systemd = Systemd::without_bus("tessaro-kiosk.service", &log);

        assert_eq!(systemd.active_state(), "inactive");
        assert_eq!(systemd.main_pid(), 0);
        assert!(systemd.restart().is_err());
    }

    /// Needs a real system bus, so it is not part of the default run:
    ///     cargo test -- --ignored
    #[test]
    #[ignore = "requires a system bus"]
    fn reads_a_real_unit() {
        let log = Log::buffered(true);
        let systemd = Systemd::new("systemd-journald.service", &log);

        assert_eq!(systemd.active_state(), "active");
        assert!(systemd.main_pid() > 0);
    }

    #[test]
    #[ignore = "requires a system bus"]
    fn a_missing_unit_looks_stopped() {
        let log = Log::buffered(true);
        let systemd = Systemd::new("definitely-not-a-unit.service", &log);

        assert_eq!(systemd.active_state(), "inactive");
        assert_eq!(systemd.main_pid(), 0);
        // Marshalling and interface lookup are exercised even though systemd
        // answers NoSuchUnit.
        assert!(systemd.restart().is_err());
    }
}
