//! NetworkManager: the four profiles the device manages, and what
//! `tessaro-ctl net profiles|show|wifi|wifi scan` read.
//!
//! * **The profiles** are rendered from the settings (`profiles.rs`) as
//!   keyfiles under `/run/NetworkManager/system-connections`, and a change to
//!   `ethernet.*`, `wifi.*` or `node.name` switches them as one transaction
//!   (`txn.rs`) the device keeps or rolls back by itself, before the setting
//!   is saved at all. Profiles made by hand are listed and never touched.
//! * **Reading** goes through nmrs: saved profiles, access points, WiFi
//!   devices. Its types already decode what NetworkManager's properties
//!   mean. The calls that change anything - checkpoints, activation,
//!   reloading - are `proxy.rs`, on nmrs's own connection.
//!
//! `net` and `net interfaces` stay on the kernel (`net.rs`): they have to
//! answer on a device whose NetworkManager is the broken thing. So does the
//! transaction's own check that the device still has a route.
//!
//! Every call to NetworkManager is under `within()`, through `nm_call`, and
//! none of it touches the watchdog: this is the control plane.

pub mod nat;
pub mod profiles;
pub mod proxy;
pub mod settings;
pub mod txn;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use protocol::{
    NetChange, NetProfile, NetProfileDetail, Verify, WifiDeviceInfo, WifiNetwork, WifiSecurity,
    WifiStatus,
};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::control::blocking;
use crate::deadline::within;
use crate::log::Log;
use crate::paths::Paths;
use profiles::{Keyfile, NetConfig, Profile, WifiMode};
use proxy::{ActiveProxy, DeviceProxy, ManagerProxy, ProfileProxy, SettingsProxy, WirelessProxy};
use txn::{Activated, Commit, Files, Nat, Ops, Plan, Route, Up};

/// One method call to NetworkManager, which answers in milliseconds when it
/// is well. Listing every profile is one `GetSettings` per profile, so it
/// gets longer.
const CALL: Duration = Duration::from_secs(10);
const LIST: Duration = Duration::from_secs(20);
/// Connecting to the system bus.
const CONNECT: Duration = Duration::from_secs(5);
/// How long a scan may take to report back.
const SCAN: Duration = Duration::from_secs(10);
/// A TCP `--verify` target.
const TCP_VERIFY: Duration = Duration::from_secs(5);
/// One echo at the gateway or a `--verify` host.
const ECHO: Duration = Duration::from_secs(2);

/// A call to NetworkManager, bounded, with its error in words.
async fn nm_call<T, E: std::fmt::Display>(
    what: &'static str,
    limit: Duration,
    call: impl Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match within(what, limit, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!("{what}: {err}")),
        Err(expired) => Err(expired.to_string()),
    }
}

fn path(text: &str) -> Result<ObjectPath<'_>, String> {
    ObjectPath::try_from(text).map_err(|err| format!("{text}: {err}"))
}

/// The interfaces the kernel has, by kind: `ethernet` and `wireless`.
fn interfaces(paths: &Paths) -> Vec<(String, String)> {
    crate::net::snapshot(paths)
        .interfaces
        .into_iter()
        .map(|iface| (iface.name, iface.kind))
        .collect()
}

pub struct Network {
    log: Arc<Log>,
    paths: Paths,
    files: Files,
    /// Connected on first use, so an agent that starts before
    /// NetworkManager still gets there.
    client: tokio::sync::Mutex<Option<nmrs::NetworkManager>>,
    /// Held by the task running a change, for as long as it runs.
    changing: Arc<tokio::sync::Mutex<()>>,
}

impl Network {
    pub fn new(log: Arc<Log>, paths: Paths) -> Arc<Self> {
        Arc::new(Self {
            files: Files::new(paths.network_dir()),
            log,
            paths,
            client: tokio::sync::Mutex::new(None),
            changing: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// A change is under way. An update must not be committed meanwhile.
    pub fn busy(&self) -> bool {
        self.changing.try_lock().is_err()
    }

    async fn client(&self) -> Result<nmrs::NetworkManager, String> {
        // naked: in-process; the one holder waits only on the connect deadline below
        let mut client = self.client.lock().await;
        if let Some(nm) = client.as_ref() {
            return Ok(nm.clone());
        }
        let nm = nm_call("NetworkManager", CONNECT, nmrs::NetworkManager::new())
            .await
            .map_err(|err| format!("NetworkManager is not reachable: {err}"))?;
        *client = Some(nm.clone());
        Ok(nm)
    }

    async fn live(&self) -> Result<Live, String> {
        let nm = self.client().await?;
        Ok(Live {
            conn: nm.dbus_connection().clone(),
            paths: self.paths.clone(),
        })
    }

    /// Whether the managed WiFi interface exists right now.
    pub async fn has_wifi(&self, interface: &str) -> bool {
        let paths = self.paths.clone();
        let interface = interface.to_string();
        blocking("reading the network", move || {
            Ok(interfaces(&paths)
                .iter()
                .any(|(name, kind)| *name == interface && kind == "wireless"))
        })
        .await
        .unwrap_or(false)
    }

    // --- reading -------------------------------------------------------------

    pub async fn profiles(&self) -> Result<Vec<NetProfile>, String> {
        let nm = self.client().await?;
        let live = self.live().await?;
        let saved = nm_call("listing profiles", LIST, nm.list_saved_connections()).await?;
        let active = live.active_by_uuid().await?;

        let mut profiles: Vec<NetProfile> = saved
            .into_iter()
            .filter(|profile| profile.connection_type != "loopback")
            .map(|profile| {
                let device = active.get(&profile.uuid).cloned();
                NetProfile {
                    active: device.is_some(),
                    device: device.or(profile.interface_name),
                    kind: match profile.connection_type.as_str() {
                        settings::ETHERNET => "ethernet".to_string(),
                        settings::WIFI => "wifi".to_string(),
                        other => other.to_string(),
                    },
                    managed: profiles::is_managed(&profile.uuid),
                    name: profile.id,
                    uuid: profile.uuid,
                    autoconnect: profile.autoconnect,
                    priority: profile.autoconnect_priority,
                    saved: !profile.unsaved,
                }
            })
            .collect();
        profiles.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.cmp(&b.name)));
        Ok(profiles)
    }

    /// A profile by uuid, or by a name only one profile has.
    async fn resolve(&self, wanted: &str) -> Result<NetProfile, String> {
        let profiles = self.profiles().await?;
        if let Some(profile) = profiles.iter().find(|p| p.uuid == wanted) {
            return Ok(profile.clone());
        }
        let named: Vec<&NetProfile> = profiles.iter().filter(|p| p.name == wanted).collect();
        match named.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!(
                "no profile named {wanted}; `tessaro-ctl net profiles` lists them"
            )),
            several => Err(format!(
                "{} profiles are named {wanted}; name one by uuid: {}",
                several.len(),
                several
                    .iter()
                    .map(|p| p.uuid.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub async fn show(&self, wanted: &str) -> Result<NetProfileDetail, String> {
        let profile = self.resolve(wanted).await?;
        let live = self.live().await?;
        let dict = live.settings_of(&profile.uuid).await?;
        let addresses = match (&profile.device, profile.active) {
            (Some(device), true) => {
                let device = device.clone();
                let paths = self.paths.clone();
                blocking("reading the network", move || {
                    Ok(crate::net::snapshot(&paths)
                        .interfaces
                        .into_iter()
                        .find(|iface| iface.name == device)
                        .map(|iface| iface.addresses)
                        .unwrap_or_default())
                })
                .await?
            }
            _ => Vec::new(),
        };
        Ok(NetProfileDetail {
            ipv4: settings::ip(&dict, "ipv4"),
            ipv6: settings::ip(&dict, "ipv6"),
            wifi: settings::wifi(&dict),
            profile,
            addresses,
        })
    }

    pub async fn wifi(&self) -> Result<WifiStatus, String> {
        let nm = self.client().await?;
        let live = self.live().await?;
        let (enabled, hardware_enabled) = live.radios().await?;
        let devices = nm_call("listing WiFi devices", CALL, nm.list_wifi_devices()).await?;
        let points = nm_call("listing access points", LIST, nm.list_access_points(None))
            .await
            .unwrap_or_default();

        let devices = devices
            .into_iter()
            .map(|device| {
                let current = points
                    .iter()
                    .find(|point| point.is_active && point.interface == device.interface);
                WifiDeviceInfo {
                    state: device.state.to_string().to_lowercase(),
                    ssid: device.active_ssid.clone(),
                    signal: current.map(|point| point.strength),
                    frequency_mhz: current.map(|point| point.frequency_mhz),
                    interface: device.interface,
                }
            })
            .collect();
        Ok(WifiStatus {
            enabled,
            hardware_enabled,
            devices,
        })
    }

    pub async fn scan(
        &self,
        interface: Option<String>,
        rescan: bool,
    ) -> Result<Vec<WifiNetwork>, String> {
        if let Some(name) = &interface {
            crate::ping::check_interface(name)?;
        }
        let nm = self.client().await?;
        let live = self.live().await?;
        if rescan {
            live.scan(interface.as_deref()).await?;
        }
        let points = nm_call(
            "listing access points",
            LIST,
            nm.list_access_points(interface.as_deref()),
        )
        .await?;
        let known = self.known_ssids().await.unwrap_or_default();

        let mut seen = HashSet::new();
        let mut networks: Vec<WifiNetwork> = points
            .into_iter()
            .filter(|point| seen.insert((point.ssid.clone(), point.bssid.clone())))
            .map(|point| WifiNetwork {
                known: !point.ssid.is_empty() && known.contains(&point.ssid),
                security: security_of(&point.security),
                ssid: point.ssid,
                bssid: point.bssid,
                signal: point.strength,
                frequency_mhz: point.frequency_mhz,
                interface: point.interface,
                active: point.is_active,
            })
            .collect();
        networks.sort_by(|a, b| b.signal.cmp(&a.signal).then_with(|| a.ssid.cmp(&b.ssid)));
        Ok(networks)
    }

    async fn known_ssids(&self) -> Result<HashSet<String>, String> {
        let nm = self.client().await?;
        let saved = nm_call("listing profiles", LIST, nm.list_saved_connections()).await?;
        Ok(saved
            .into_iter()
            .filter_map(|profile| match profile.summary {
                nmrs::models::SettingsSummary::Wifi { ssid, .. } => Some(ssid),
                _ => None,
            })
            .collect())
    }

    /// The security of `ssid` as a scan of `interface` sees it, scanning
    /// again once when it is not in the last results: what `net wifi join`
    /// stores as `wifi.security` when it is not given.
    pub async fn security_of_ssid(
        &self,
        interface: &str,
        ssid: &str,
    ) -> Result<WifiSecurity, String> {
        let mut found = self.find_network(interface, ssid).await?;
        if found.is_none() {
            let live = self.live().await?;
            live.scan(Some(interface)).await?;
            found = self.find_network(interface, ssid).await?;
        }
        let point = found.ok_or_else(|| {
            format!(
                "{ssid} is not in range of {interface}; for a hidden network pass --hidden \
                 --security psk|sae|open"
            )
        })?;
        let features = &point.security;
        if features.eap || features.eap_suite_b_192 {
            return Err(format!(
                "{ssid} is an enterprise (802.1X) network, which is not supported"
            ));
        }
        if features.wep40 || features.wep104 || (features.privacy && !features.psk && !features.sae)
        {
            return Err(format!("{ssid} uses WEP, which is not supported"));
        }
        Ok(if features.psk {
            WifiSecurity::Psk
        } else if features.sae {
            WifiSecurity::Sae
        } else {
            WifiSecurity::Open
        })
    }

    async fn find_network(
        &self,
        device: &str,
        ssid: &str,
    ) -> Result<Option<nmrs::models::AccessPoint>, String> {
        let nm = self.client().await?;
        let points = nm_call(
            "listing access points",
            LIST,
            nm.list_access_points(Some(device)),
        )
        .await?;
        Ok(points
            .into_iter()
            .filter(|point| point.ssid == ssid)
            .max_by_key(|point| point.strength))
    }

    pub async fn last(&self) -> Result<Option<NetChange>, String> {
        self.files.last().await
    }

    // --- changing ------------------------------------------------------------

    /// Switch from `old` to `new` as one transaction on a task of its own, so
    /// a client that is cut off by it - the expected case when it re-addresses
    /// the link it came in on - does not stop it half way. `commit` saves the
    /// settings, and runs only once the change has held.
    pub async fn apply(
        self: &Arc<Self>,
        who: String,
        action: String,
        old: NetConfig,
        new: NetConfig,
        verify: Verify,
        commit: Commit,
    ) -> Result<NetChange, String> {
        let lock = Arc::clone(&self.changing)
            .try_lock_owned()
            .map_err(|_| "a network change is already in progress on this device".to_string())?;
        let plan = self.plan(action, &old, &new, verify).await?;
        self.log
            .info(format!("network: {} requested by {who}", plan.action));

        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            let _lock = lock;
            // naked: live() only waits on the connect deadline in client()
            let live = this.live().await?;
            // naked: every step of the transaction is under within() in Live
            txn::run(&live, &this.files, plan, &this.log, commit).await
        });
        // naked: the task bounds itself; this only waits for its answer
        match task.await {
            Ok(outcome) => outcome,
            Err(err) => Err(format!("the network change failed: {err}")),
        }
    }

    /// What going from `old` to `new` takes: the keyfiles, which profiles to
    /// bring up or down, and the NAT.
    async fn plan(
        &self,
        action: String,
        old: &NetConfig,
        new: &NetConfig,
        verify: Verify,
    ) -> Result<Plan, String> {
        let paths = self.paths.clone();
        let present = blocking("reading the network", move || Ok(interfaces(&paths))).await?;
        let has = |name: &str, kind: &str| present.iter().any(|(n, k)| n == name && k == kind);
        let any_ethernet = present.iter().any(|(_, kind)| kind == "ethernet");
        let wifi_here = has(&new.wifi.interface, "wireless");

        let mut up = Vec::new();
        let mut down = Vec::new();
        let mut devices: Vec<String> = Vec::new();

        let ethernet_present = match &new.ethernet.interface {
            Some(name) => has(name, "ethernet"),
            None => any_ethernet,
        };
        if old.ethernet != new.ethernet && ethernet_present {
            up.push(Up {
                profile: new.ethernet_profile(),
                device: new.ethernet.interface.clone(),
            });
            match &new.ethernet.interface {
                Some(name) => devices.push(name.clone()),
                None => devices.extend(
                    present
                        .iter()
                        .filter(|(_, kind)| kind == "ethernet")
                        .map(|(name, _)| name.clone()),
                ),
            }
        }

        // The NAT is its own switch: changing it alone reactivates nothing.
        let mut old_wifi = old.wifi.clone();
        old_wifi.nat = new.wifi.nat;
        if old_wifi != new.wifi && wifi_here {
            match new.wifi_profile() {
                Some(profile) => up.push(Up {
                    profile,
                    device: Some(new.wifi.interface.clone()),
                }),
                None => down.extend([profiles::WIFI_HOTSPOT, profiles::WIFI_CLIENT]),
            }
            devices.push(new.wifi.interface.clone());
        }
        let nat = (old.wifi.nat != new.wifi.nat).then(|| Nat {
            on: new.wifi.nat,
            was: old.wifi.nat,
            interface: new.wifi.interface.clone(),
        });

        // Going back to the hotspot, or off, gives up WiFi as an uplink on
        // purpose: that it takes the default route with it is the point, not
        // a failure - on a device whose only uplink was the client network.
        let note = match (old.wifi.mode, new.wifi.mode) {
            (WifiMode::Client, WifiMode::Hotspot) => Some(format!(
                "{} is the hotspot {} again",
                new.wifi.interface, new.wifi.hotspot_ssid
            )),
            _ => None,
        };
        let leaves_wifi = old.wifi.mode == WifiMode::Client && new.wifi.mode != WifiMode::Client;

        Ok(Plan {
            action,
            devices,
            new: profiles::render(new),
            old: profiles::render(old),
            down,
            up,
            nat,
            verify,
            keep_route: !leaves_wifi,
            note,
        })
    }

    /// Re-render the profiles for `config` outside any transaction - after a
    /// claim, an unclaim or a new hotspot password - and bring the hotspot
    /// back up on its new security if it is the one up. Nothing here can cut
    /// the device off anything but its own hotspot.
    pub async fn refresh(&self, config: &NetConfig) -> Result<(), String> {
        // naked: the lock is only ever held by a change, which bounds itself
        let _lock = self.changing.lock().await;
        let live = self.live().await?;
        live.write_profiles(&profiles::render(config)).await?;
        if config.wifi.mode == WifiMode::Hotspot && self.has_wifi(&config.wifi.interface).await {
            let device = live.device_path(&config.wifi.interface).await?;
            if let Some((_, uuid)) = live.active_on(device.as_str()).await {
                if uuid == profiles::WIFI_HOTSPOT.uuid {
                    live.activate(&Up {
                        profile: profiles::WIFI_HOTSPOT,
                        device: Some(config.wifi.interface.clone()),
                    })
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// Roll back a change the previous agent left unfinished, onto
    /// `current` - what `state.json` renders. NetworkManager may still be
    /// starting, so this keeps trying for a minute.
    pub fn recover(self: &Arc<Self>, current: NetConfig) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let files = profiles::render(&current);
            let nat = Nat {
                on: current.wifi.nat,
                was: current.wifi.nat,
                interface: current.wifi.interface.clone(),
            };
            for attempt in 0..12 {
                if attempt > 0 {
                    // naked: a plain timer between tries
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                // naked: a disk read under blocking()'s within()
                match this.files.unfinished().await {
                    Ok(None) => return,
                    Ok(Some(_)) => {}
                    Err(err) => {
                        this.log.info(format!("network: {err}"));
                        return;
                    }
                }
                // naked: live() only waits on the connect deadline in client()
                let Ok(live) = this.live().await else {
                    continue;
                };
                // naked: the lock is only ever held by a change, which bounds itself
                let _lock = this.changing.lock().await;
                // naked: every step is under within() in Live
                match txn::recover(&live, &this.files, &files, &nat, &this.log).await {
                    Ok(_) => return,
                    Err(err) => this.log.info(format!("network: recovering: {err}")),
                }
            }
            this.log.info(
                "network: NetworkManager never answered; an unfinished change is left as it is",
            );
        });
    }
}

/// What a scan says about an access point's security, the way `net show`
/// says a profile's.
fn security_of(features: &nmrs::models::SecurityFeatures) -> String {
    let mut kinds = Vec::new();
    if features.sae {
        kinds.push("sae");
    }
    if features.psk {
        kinds.push("wpa-psk");
    }
    if features.eap || features.eap_suite_b_192 {
        kinds.push("wpa-eap");
    }
    if kinds.is_empty() && (features.privacy || features.wep40 || features.wep104) {
        kinds.push("wep");
    }
    if kinds.is_empty() {
        kinds.push("open");
    }
    kinds.join("/")
}

/// NetworkManager and the kernel, for real.
pub struct Live {
    conn: zbus::Connection,
    paths: Paths,
}

impl Live {
    async fn manager(&self) -> Result<ManagerProxy<'_>, String> {
        let builder = ManagerProxy::builder(&self.conn).cache_properties(CacheProperties::No);
        nm_call("NetworkManager", CALL, builder.build()).await
    }

    async fn settings(&self) -> Result<SettingsProxy<'_>, String> {
        let builder = SettingsProxy::builder(&self.conn).cache_properties(CacheProperties::No);
        nm_call("NetworkManager settings", CALL, builder.build()).await
    }

    async fn profile(&self, at: &str) -> Result<ProfileProxy<'_>, String> {
        let builder = ProfileProxy::builder(&self.conn)
            .cache_properties(CacheProperties::No)
            .path(at.to_string())
            .map_err(|err| format!("{at}: {err}"))?;
        nm_call("a NetworkManager profile", CALL, builder.build()).await
    }

    async fn device(&self, at: &str) -> Result<DeviceProxy<'_>, String> {
        let builder = DeviceProxy::builder(&self.conn)
            .cache_properties(CacheProperties::No)
            .path(at.to_string())
            .map_err(|err| format!("{at}: {err}"))?;
        nm_call("a NetworkManager device", CALL, builder.build()).await
    }

    async fn wireless(&self, at: &str) -> Result<WirelessProxy<'_>, String> {
        let builder = WirelessProxy::builder(&self.conn)
            .cache_properties(CacheProperties::No)
            .path(at.to_string())
            .map_err(|err| format!("{at}: {err}"))?;
        nm_call("a WiFi device", CALL, builder.build()).await
    }

    async fn active(&self, at: &str) -> Result<ActiveProxy<'_>, String> {
        let builder = ActiveProxy::builder(&self.conn)
            .cache_properties(CacheProperties::No)
            .path(at.to_string())
            .map_err(|err| format!("{at}: {err}"))?;
        nm_call("an active connection", CALL, builder.build()).await
    }

    async fn profile_path(&self, uuid: &str) -> Result<OwnedObjectPath, String> {
        let settings = self.settings().await?;
        nm_call(
            "GetConnectionByUuid",
            CALL,
            settings.get_connection_by_uuid(uuid),
        )
        .await
    }

    async fn device_path(&self, interface: &str) -> Result<OwnedObjectPath, String> {
        let manager = self.manager().await?;
        nm_call(
            "GetDeviceByIpIface",
            CALL,
            manager.get_device_by_ip_iface(interface),
        )
        .await
    }

    async fn interface_of(&self, device: &str) -> Result<String, String> {
        let device = self.device(device).await?;
        nm_call("a device's interface", CALL, device.interface()).await
    }

    async fn settings_of(&self, uuid: &str) -> Result<settings::Dict, String> {
        let at = self.profile_path(uuid).await?;
        let profile = self.profile(at.as_str()).await?;
        nm_call("GetSettings", CALL, profile.get_settings()).await
    }

    async fn radios(&self) -> Result<(bool, bool), String> {
        let manager = self.manager().await?;
        let enabled = nm_call("WirelessEnabled", CALL, manager.wireless_enabled()).await?;
        let hardware = nm_call(
            "WirelessHardwareEnabled",
            CALL,
            manager.wireless_hardware_enabled(),
        )
        .await?;
        Ok((enabled, hardware))
    }

    /// Every active profile's uuid, with the interface it is active on.
    async fn active_by_uuid(&self) -> Result<HashMap<String, String>, String> {
        let manager = self.manager().await?;
        let actives = nm_call("ActiveConnections", CALL, manager.active_connections()).await?;
        let mut out = HashMap::new();
        for at in actives {
            let Ok(active) = self.active(at.as_str()).await else {
                continue;
            };
            let Ok(uuid) = nm_call("an active connection's uuid", CALL, active.uuid()).await else {
                continue;
            };
            let devices = nm_call("an active connection's devices", CALL, active.devices())
                .await
                .unwrap_or_default();
            let mut interface = String::new();
            if let Some(device) = devices.first() {
                interface = self.interface_of(device.as_str()).await.unwrap_or_default();
            }
            out.insert(uuid, interface);
        }
        Ok(out)
    }

    /// The active connection on a device, and its profile's uuid.
    async fn active_on(&self, device_at: &str) -> Option<(OwnedObjectPath, String)> {
        let device = self.device(device_at).await.ok()?;
        let active = nm_call("a device's connection", CALL, device.active_connection())
            .await
            .ok()?;
        if active.as_str() == "/" {
            return None;
        }
        let proxy = self.active(active.as_str()).await.ok()?;
        let uuid = nm_call("an active connection's uuid", CALL, proxy.uuid())
            .await
            .ok()?;
        Some((active, uuid))
    }

    /// Ask every WiFi device, or one, to scan, and wait until it has.
    async fn scan(&self, interface: Option<&str>) -> Result<(), String> {
        let manager = self.manager().await?;
        let devices = nm_call("Devices", CALL, manager.devices()).await?;
        let mut waiting = Vec::new();
        for at in devices {
            let device = self.device(at.as_str()).await?;
            if nm_call("a device's type", CALL, device.device_type()).await? != proxy::DEVICE_WIFI {
                continue;
            }
            if let Some(wanted) = interface {
                if nm_call("a device's interface", CALL, device.interface()).await? != wanted {
                    continue;
                }
            }
            let wireless = self.wireless(at.as_str()).await?;
            let before = nm_call("LastScan", CALL, wireless.last_scan()).await?;
            // Refused when a scan ran a moment ago - or while the device is
            // the hotspot; what it found last is then what the list shows.
            if nm_call("RequestScan", CALL, wireless.request_scan(HashMap::new()))
                .await
                .is_ok()
            {
                waiting.push((at, before));
            }
        }

        let wait = async {
            for (at, before) in &waiting {
                loop {
                    let wireless = self.wireless(at.as_str()).await?;
                    let now = nm_call("LastScan", CALL, wireless.last_scan()).await?;
                    if now != *before {
                        break;
                    }
                    // naked: bounded by the within(SCAN) below
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            Ok::<(), String>(())
        };
        let _ = within("the WiFi scan", SCAN, wait).await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Ops for Live {
    async fn checkpoint(&self, devices: &[String], backstop: Duration) -> Result<String, String> {
        let mut paths = Vec::new();
        for device in devices {
            if let Ok(at) = self.device_path(device).await {
                paths.push(at);
            }
        }
        let paths: Vec<ObjectPath<'_>> = paths.iter().map(|at| at.as_ref()).collect();
        let manager = self.manager().await?;
        let checkpoint = nm_call(
            "CheckpointCreate",
            CALL,
            manager.checkpoint_create(&paths, backstop.as_secs() as u32, proxy::CHECKPOINT_FLAGS),
        )
        .await?;
        Ok(checkpoint.to_string())
    }

    async fn rollback(&self, checkpoint: &str) -> Result<(), String> {
        let manager = self.manager().await?;
        let results = nm_call(
            "CheckpointRollback",
            CALL,
            manager.checkpoint_rollback(&path(checkpoint)?),
        )
        .await?;
        let failed: Vec<String> = results
            .into_iter()
            .filter(|(_, code)| *code != 0)
            .map(|(device, code)| format!("{device} ({code})"))
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(format!("not rolled back: {}", failed.join(", ")))
        }
    }

    async fn destroy(&self, checkpoint: &str) -> Result<(), String> {
        let manager = self.manager().await?;
        nm_call(
            "CheckpointDestroy",
            CALL,
            manager.checkpoint_destroy(&path(checkpoint)?),
        )
        .await
    }

    async fn write_profiles(&self, files: &[Keyfile]) -> Result<(), String> {
        let dir = self.paths.nm_run_dir.clone();
        let files = files.to_vec();
        blocking("writing the network profiles", move || {
            profiles::write(&dir, &files).map_err(|err| format!("{}: {err}", dir.display()))
        })
        .await?;
        let settings = self.settings().await?;
        nm_call("ReloadConnections", CALL, settings.reload_connections())
            .await
            .map(drop)
    }

    async fn activate(&self, up: &Up) -> Result<Activated, String> {
        let manager = self.manager().await?;
        let none = path("/")?;
        let at = self.profile_path(up.profile.uuid).await?;
        let device_at: OwnedObjectPath = match &up.device {
            Some(device) => self.device_path(device).await?,
            None => none.clone().into(),
        };
        let active = nm_call(
            "ActivateConnection",
            CALL,
            manager.activate_connection(&at, &device_at, &none),
        )
        .await?;
        let proxy = self.active(active.as_str()).await?;
        let devices = nm_call("an active connection's devices", CALL, proxy.devices())
            .await
            .unwrap_or_default();
        let mut interface = up.device.clone();
        if let Some(first) = devices.first() {
            interface = self.interface_of(first.as_str()).await.ok().or(interface);
        }
        Ok(Activated {
            active: active.to_string(),
            device: interface,
        })
    }

    async fn deactivate(&self, profile: Profile) -> Result<(), String> {
        let manager = self.manager().await?;
        let actives = nm_call("ActiveConnections", CALL, manager.active_connections()).await?;
        for at in actives {
            let Ok(proxy) = self.active(at.as_str()).await else {
                continue;
            };
            if nm_call("an active connection's uuid", CALL, proxy.uuid())
                .await
                .as_deref()
                == Ok(profile.uuid)
            {
                return nm_call(
                    "DeactivateConnection",
                    CALL,
                    manager.deactivate_connection(&at),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn activated(&self, active: &str, limit: Duration) -> Result<(), String> {
        let proxy = self.active(active).await?;
        let wait = async {
            // Subscribe first, then read: a change that lands in between is
            // seen either way.
            // naked: bounded by the within(limit) below
            let mut changes = proxy
                .receive_activation()
                .await
                .map_err(|err| format!("watching the connection: {err}"))?;
            loop {
                // naked: bounded by the within(limit) below
                match proxy.state().await {
                    Ok(proxy::ACTIVATED) => return Ok(()),
                    Ok(proxy::DEACTIVATED) => {
                        return Err("the connection was deactivated".to_string())
                    }
                    Ok(_) => {}
                    Err(_) => return Err("the connection went away".to_string()),
                }
                tokio::select! {
                    // naked: bounded by the within(limit) below
                    signal = changes.next() => match signal {
                        Some(signal) => {
                            if let Ok(args) = signal.args() {
                                match *args.state() {
                                    proxy::ACTIVATED => return Ok(()),
                                    proxy::DEACTIVATED => return Err(proxy::reason(*args.reason())),
                                    _ => {}
                                }
                            }
                        }
                        None => return Err("the connection went away".to_string()),
                    },
                    // A signal NetworkManager never sends must not be waited
                    // for: read the state again every second.
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        };
        match within("the connection coming up", limit, wait).await {
            Ok(outcome) => outcome,
            Err(expired) => Err(expired.to_string()),
        }
    }

    async fn set_nat(&self, on: bool, interface: &str) -> Result<(), String> {
        // naked: nat::apply runs nft under within()
        nat::apply(on, interface).await
    }

    async fn route(&self) -> Option<Route> {
        let paths = self.paths.clone();
        blocking("reading the network", move || {
            let net = crate::net::snapshot(&paths);
            Ok(net.interface.map(|interface| Route {
                interface,
                gateway: net.gateway.and_then(|gateway| gateway.parse().ok()),
            }))
        })
        .await
        .ok()
        .flatten()
    }

    async fn has_address(&self, interface: &str) -> bool {
        let paths = self.paths.clone();
        let interface = interface.to_string();
        blocking("reading the network", move || {
            Ok(crate::net::snapshot(&paths).interfaces.iter().any(|iface| {
                iface.name == interface && iface.addresses.iter().any(|a| a.scope == "global")
            }))
        })
        .await
        .unwrap_or(false)
    }

    async fn reach(&self, verify: &Verify, route: Option<&Route>) -> Result<String, String> {
        match verify {
            Verify::None => Ok("not checked".to_string()),
            Verify::Gateway => {
                let route = route.ok_or_else(|| "no default route".to_string())?;
                let gateway = route
                    .gateway
                    .ok_or_else(|| format!("no gateway on {}", route.interface))?;
                // naked: answers() waits under within(ECHO)
                if crate::ping::answers(gateway, Some(&route.interface), ECHO).await {
                    Ok(format!("gateway {gateway} answered"))
                } else {
                    Err(format!("gateway {gateway} did not answer"))
                }
            }
            Verify::Host { host } => {
                // naked: resolve() is under within()
                let address: IpAddr = crate::ping::resolve(host).await?;
                // naked: answers() waits under within(ECHO)
                if crate::ping::answers(address, None, ECHO).await {
                    Ok(format!("{host} answered"))
                } else {
                    Err(format!("{host} did not answer"))
                }
            }
            Verify::Tcp { host, port } => {
                let connect = tokio::net::TcpStream::connect((host.as_str(), *port));
                match within("the --verify host", TCP_VERIFY, connect).await {
                    Ok(Ok(_)) => Ok(format!("{host}:{port} accepted a connection")),
                    Ok(Err(err)) => Err(format!("{host}:{port}: {err}")),
                    Err(expired) => Err(format!("{host}:{port}: {expired}")),
                }
            }
        }
    }

    async fn pause(&self, pause: Duration) {
        // naked: a plain timer between tries
        tokio::time::sleep(pause).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_points_say_their_security() {
        let mut features = nmrs::models::SecurityFeatures::default();
        assert_eq!(security_of(&features), "open");
        features.privacy = true;
        assert_eq!(security_of(&features), "wep");
        features.psk = true;
        assert_eq!(security_of(&features), "wpa-psk");
        features.sae = true;
        assert_eq!(security_of(&features), "sae/wpa-psk");
    }

    /// Read-only, against the workstation's own NetworkManager.
    #[tokio::test]
    #[ignore]
    async fn lists_this_hosts_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let env: HashMap<String, String> = HashMap::from([(
            "KIOSK_STATE_DIR".to_string(),
            dir.path().display().to_string(),
        )]);
        let paths = Paths::load(&env);
        let network = Network::new(Arc::new(Log::buffered(false)), paths);
        let profiles = network.profiles().await.unwrap();
        assert!(!profiles.is_empty());
        let wifi = network.wifi().await;
        assert!(wifi.is_ok(), "{wifi:?}");
    }
}
