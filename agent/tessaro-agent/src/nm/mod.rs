//! NetworkManager, for `tessaro-ctl net profiles|show|set|up|down|forget`
//! and `net wifi`.
//!
//! Two halves, and the split is deliberate:
//!
//! * **Reading** goes through nmrs: saved profiles, access points, WiFi
//!   devices. Its types already decode what NetworkManager's properties
//!   mean, and nothing it reads can change anything.
//! * **Changing** goes through `proxy.rs` on nmrs's own connection, as one
//!   transaction (`txn.rs`) the device keeps or rolls back by itself. nmrs's
//!   write calls save to disk at once and know no checkpoints, which is the
//!   opposite of what a change made over the network it is changing needs.
//!
//! `net` and `net interfaces` stay on the kernel (`net.rs`): they have to
//! answer on a device whose NetworkManager is the broken thing. So does the
//! transaction's own check that the device still has a route.
//!
//! Every call to NetworkManager is under `within()`, through `nm_call`, and
//! none of it touches the watchdog: this is the control plane.

pub mod proxy;
pub mod settings;
pub mod txn;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use protocol::{
    netkeys, NetChange, NetKeyInfo, NetProfile, NetProfileDetail, Verify, WifiDeviceInfo,
    WifiNetwork, WifiSecurity, WifiStatus,
};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::control::blocking;
use crate::deadline::within;
use crate::log::Log;
use crate::paths::Paths;
use proxy::{ActiveProxy, DeviceProxy, ManagerProxy, ProfileProxy, SettingsProxy, WirelessProxy};
use settings::Dict;
use txn::{Applied, Files, Ops, Plan, Route, Snapshot, Step};

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

/// What a client asked to change, before it is resolved into a `Plan`.
#[derive(Debug, Clone)]
pub enum Change {
    Set {
        profile: String,
        values: BTreeMap<String, String>,
        psk: Option<String>,
    },
    Up {
        profile: String,
    },
    Down {
        profile: String,
    },
    Forget {
        profile: String,
    },
    Join {
        ssid: String,
        psk: Option<String>,
        security: Option<WifiSecurity>,
        hidden: bool,
        interface: Option<String>,
    },
    Radio {
        on: bool,
    },
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

    pub fn keys(&self) -> Vec<NetKeyInfo> {
        netkeys::NET_KEYS
            .iter()
            .map(|key| NetKeyInfo {
                name: key.name.to_string(),
                values: key.kind.describe(),
                wifi: key.wifi,
                doc: key.doc.to_string(),
            })
            .collect()
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

    pub async fn last(&self) -> Result<Option<NetChange>, String> {
        self.files.last().await
    }

    // --- changing ------------------------------------------------------------

    /// Run a change to the end on a task of its own, so a client that is
    /// cut off by it - the expected case when it re-addresses the link it
    /// came in on - does not stop it half way.
    pub async fn change(
        self: &Arc<Self>,
        who: String,
        change: Change,
        verify: Verify,
    ) -> Result<NetChange, String> {
        let lock = Arc::clone(&self.changing)
            .try_lock_owned()
            .map_err(|_| "a network change is already in progress on this device".to_string())?;
        let plan = self.plan(change, verify).await?;
        self.log.info(format!(
            "network: {} {} requested by {who}",
            plan.action,
            plan.profile.as_deref().unwrap_or("")
        ));

        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            let _lock = lock;
            // naked: live() only waits on the connect deadline in client()
            let live = this.live().await?;
            // naked: every step of the transaction is under within() in Live
            txn::run(&live, &this.files, plan, &this.log).await
        });
        // naked: the task bounds itself; this only waits for its answer
        match task.await {
            Ok(outcome) => outcome,
            Err(err) => Err(format!("the network change failed: {err}")),
        }
    }

    /// Roll back a change the previous agent left unfinished. NetworkManager
    /// may still be starting, so this keeps trying for a minute.
    pub fn recover(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
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
                match txn::recover(&live, &this.files, &this.log).await {
                    Ok(_) => return,
                    Err(err) => this.log.info(format!("network: recovering: {err}")),
                }
            }
            this.log.info(
                "network: NetworkManager never answered; an unfinished change is left as it is",
            );
        });
    }

    async fn plan(&self, change: Change, verify: Verify) -> Result<Plan, String> {
        let live = self.live().await?;
        let route = live.route().await;
        let route_device: Vec<String> = route.iter().map(|r| r.interface.clone()).collect();
        let with_route = |mut devices: Vec<String>| {
            for device in &route_device {
                if !devices.contains(device) {
                    devices.push(device.clone());
                }
            }
            devices
        };

        match change {
            Change::Set {
                profile,
                values,
                psk,
            } => {
                if values.is_empty() && psk.is_none() {
                    return Err("nothing to change".to_string());
                }
                let target = self.resolve(&profile).await?;
                let dict = live.settings_of(&target.uuid).await?;
                let patched = settings::patch(&dict, &values, psk.as_deref())?;

                let carries_route =
                    target.active && route.as_ref().map(|r| &r.interface) == target.device.as_ref();
                if carries_route && !settings::autoconnect(&patched) && settings::autoconnect(&dict)
                {
                    return Err(format!(
                        "{} carries the default route; with autoconnect off the device would \
                         come back from its next boot with no network",
                        target.name
                    ));
                }
                let device = target.device.clone().filter(|_| target.active);
                Ok(Plan {
                    action: "set".to_string(),
                    note: (!target.active).then(|| {
                        "the profile is not active; the change applies when it next comes up"
                            .to_string()
                    }),
                    profile: Some(target.name.clone()),
                    uuid: Some(target.uuid.clone()),
                    devices: with_route(device.iter().cloned().collect()),
                    touched: vec![target.uuid.clone()],
                    step: Step::Update {
                        uuid: target.uuid,
                        settings: patched,
                        device,
                    },
                    verify,
                })
            }
            Change::Up { profile } => {
                let target = self.resolve(&profile).await?;
                Ok(Plan {
                    action: "up".to_string(),
                    profile: Some(target.name.clone()),
                    uuid: Some(target.uuid.clone()),
                    devices: with_route(target.device.iter().cloned().collect()),
                    touched: Vec::new(),
                    step: Step::Activate {
                        uuid: target.uuid,
                        device: target.device,
                    },
                    verify,
                    note: None,
                })
            }
            Change::Down { profile } => {
                let target = self.resolve(&profile).await?;
                if !target.active {
                    return Err(format!("{} is not active", target.name));
                }
                Ok(Plan {
                    action: "down".to_string(),
                    note: target.autoconnect.then(|| {
                        "until the next boot: it autoconnects then; set \
                         connection.autoconnect=no to keep it down"
                            .to_string()
                    }),
                    profile: Some(target.name.clone()),
                    uuid: Some(target.uuid.clone()),
                    devices: with_route(target.device.iter().cloned().collect()),
                    touched: Vec::new(),
                    step: Step::Deactivate { uuid: target.uuid },
                    verify,
                })
            }
            Change::Forget { profile } => {
                let target = self.resolve(&profile).await?;
                Ok(Plan {
                    action: "forget".to_string(),
                    profile: Some(target.name.clone()),
                    uuid: Some(target.uuid.clone()),
                    devices: with_route(target.device.iter().cloned().collect()),
                    touched: vec![target.uuid.clone()],
                    step: Step::Delete { uuid: target.uuid },
                    verify,
                    note: None,
                })
            }
            Change::Join {
                ssid,
                psk,
                security,
                hidden,
                interface,
            } => {
                self.plan_join(
                    &live, ssid, psk, security, hidden, interface, verify, with_route,
                )
                .await
            }
            Change::Radio { on } => {
                let nm = self.client().await?;
                let devices = nm_call("listing WiFi devices", CALL, nm.list_wifi_devices()).await?;
                if devices.is_empty() {
                    return Err("this device has no WiFi".to_string());
                }
                Ok(Plan {
                    action: if on { "wifi on" } else { "wifi off" }.to_string(),
                    profile: None,
                    uuid: None,
                    devices: with_route(devices.into_iter().map(|d| d.interface).collect()),
                    touched: Vec::new(),
                    step: Step::Radio { on },
                    verify,
                    note: None,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn plan_join(
        &self,
        live: &Live,
        ssid: String,
        psk: Option<String>,
        security: Option<WifiSecurity>,
        hidden: bool,
        interface: Option<String>,
        verify: Verify,
        with_route: impl Fn(Vec<String>) -> Vec<String>,
    ) -> Result<Plan, String> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err("an SSID is 1 to 32 bytes".to_string());
        }
        let nm = self.client().await?;
        let devices = nm_call("listing WiFi devices", CALL, nm.list_wifi_devices()).await?;
        let device = match interface {
            Some(name) => devices
                .iter()
                .find(|d| d.interface == name)
                .map(|d| d.interface.clone())
                .ok_or_else(|| format!("{name} is not a WiFi device"))?,
            None => devices
                .first()
                .map(|d| d.interface.clone())
                .ok_or_else(|| "this device has no WiFi".to_string())?,
        };

        let key_mgmt = match (security, hidden) {
            (Some(security), _) => key_mgmt(security).to_string(),
            (None, true) => {
                return Err("a hidden network needs --security psk, sae or open".to_string())
            }
            (None, false) => {
                let mut found = self.find_network(&device, &ssid).await?;
                if found.is_none() {
                    live.scan(Some(&device)).await?;
                    found = self.find_network(&device, &ssid).await?;
                }
                let point = found.ok_or_else(|| {
                    format!(
                        "{ssid} is not in range of {device}; for a hidden network pass \
                         --hidden --security psk|sae|open"
                    )
                })?;
                let features = &point.security;
                if features.eap || features.eap_suite_b_192 {
                    return Err(format!(
                        "{ssid} is an enterprise (802.1X) network, which is not supported yet"
                    ));
                }
                if features.wep40
                    || features.wep104
                    || (features.privacy && !features.psk && !features.sae)
                {
                    return Err(format!("{ssid} uses WEP, which is not supported"));
                }
                if features.psk {
                    "wpa-psk".to_string()
                } else if features.sae {
                    "sae".to_string()
                } else {
                    "open".to_string()
                }
            }
        };
        let existing = self.profile_for_ssid(&ssid).await?;
        if key_mgmt != "open" && psk.is_none() {
            // A network joined before comes up on the password it has.
            let profile = existing.ok_or_else(|| format!("{ssid} needs a password"))?;
            return Ok(Plan {
                action: "join".to_string(),
                profile: Some(profile.name.clone()),
                uuid: Some(profile.uuid.clone()),
                devices: with_route(vec![device.clone()]),
                touched: Vec::new(),
                step: Step::Activate {
                    uuid: profile.uuid,
                    device: Some(device),
                },
                verify,
                note: None,
            });
        }

        let (step, touched, profile, uuid) = match existing {
            Some(profile) => {
                let mut dict = live.settings_of(&profile.uuid).await?;
                settings::wifi_security(&mut dict, &key_mgmt, psk.as_deref())?;
                if hidden {
                    let wifi = dict.entry(settings::WIFI.to_string()).or_default();
                    wifi.insert(
                        "hidden".to_string(),
                        OwnedValue::try_from(Value::from(true)).map_err(|e| e.to_string())?,
                    );
                }
                (
                    Step::Update {
                        uuid: profile.uuid.clone(),
                        settings: dict,
                        device: Some(device.clone()),
                    },
                    vec![profile.uuid.clone()],
                    profile.name,
                    Some(profile.uuid),
                )
            }
            None => {
                let builder = nmrs::builders::WifiConnectionBuilder::new(ssid.clone());
                let builder = match (key_mgmt.as_str(), psk.as_deref()) {
                    ("sae", Some(psk)) => builder.sae(psk),
                    ("wpa-psk", Some(psk)) => builder.wpa_psk(psk),
                    _ => builder.open(),
                };
                if let Some(psk) = &psk {
                    netkeys::check_psk(psk)?;
                }
                let built = builder.hidden(hidden).autoconnect(true).build();
                (
                    Step::Add {
                        settings: to_dict(built)?,
                        device: device.clone(),
                    },
                    Vec::new(),
                    ssid.clone(),
                    None,
                )
            }
        };

        Ok(Plan {
            action: "join".to_string(),
            profile: Some(profile),
            uuid,
            devices: with_route(vec![device]),
            touched,
            step,
            verify,
            note: None,
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

    async fn profile_for_ssid(&self, ssid: &str) -> Result<Option<NetProfile>, String> {
        let nm = self.client().await?;
        let saved = nm_call("listing profiles", LIST, nm.list_saved_connections()).await?;
        let uuid = saved.into_iter().find_map(|profile| match profile.summary {
            nmrs::models::SettingsSummary::Wifi { ssid: known, .. } if known == ssid => {
                Some(profile.uuid)
            }
            _ => None,
        });
        match uuid {
            Some(uuid) => self.resolve(&uuid).await.map(Some),
            None => Ok(None),
        }
    }
}

fn key_mgmt(security: WifiSecurity) -> &'static str {
    match security {
        WifiSecurity::Open => "open",
        WifiSecurity::Psk => "wpa-psk",
        WifiSecurity::Sae => "sae",
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

/// nmrs's builder output as the owned dict the proxies take.
fn to_dict(
    built: HashMap<&'static str, HashMap<&'static str, Value<'static>>>,
) -> Result<Dict, String> {
    built
        .into_iter()
        .map(|(section, values)| {
            let values = values
                .into_iter()
                .map(|(key, value)| {
                    OwnedValue::try_from(value)
                        .map(|value| (key.to_string(), value))
                        .map_err(|err| format!("encoding {section}.{key}: {err}"))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            Ok((section.to_string(), values))
        })
        .collect()
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

    async fn settings_of(&self, uuid: &str) -> Result<Dict, String> {
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

    /// The active connection on `interface`, and its profile's uuid.
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
            // Refused when a scan ran a moment ago; what it found is then
            // fresh enough.
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
        // A scan that has not reported back by now still leaves whatever the
        // device saw before, which is what the list shows.
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

    async fn snapshot(&self, uuid: &str) -> Result<Option<Snapshot>, String> {
        let Ok(at) = self.profile_path(uuid).await else {
            return Ok(None);
        };
        let profile = self.profile(at.as_str()).await?;
        let mut dict = nm_call("GetSettings", CALL, profile.get_settings()).await?;
        for section in [settings::WIFI_SECURITY, "802-1x"] {
            if dict.contains_key(section) {
                if let Ok(secrets) = nm_call("GetSecrets", CALL, profile.get_secrets(section)).await
                {
                    settings::merge(&mut dict, secrets);
                }
            }
        }
        let unsaved = nm_call("Unsaved", CALL, profile.unsaved()).await?;
        Ok(Some(Snapshot {
            uuid: uuid.to_string(),
            saved: !unsaved,
            settings: settings::encode(&dict)?,
        }))
    }

    async fn restore(&self, snapshot: &Snapshot) -> Result<(), String> {
        let dict = settings::decode(&snapshot.settings)?;
        match self.profile_path(&snapshot.uuid).await {
            Ok(at) => {
                let profile = self.profile(at.as_str()).await?;
                let flags = if snapshot.saved {
                    proxy::TO_DISK
                } else {
                    proxy::IN_MEMORY
                };
                nm_call(
                    "Update2",
                    CALL,
                    profile.update2(&dict, flags, HashMap::new()),
                )
                .await
                .map(drop)
            }
            // Gone - forgotten - and it was on disk: put it back there. One
            // that only ever lived in memory is not worth resurrecting.
            Err(_) if snapshot.saved => {
                let settings = self.settings().await?;
                nm_call(
                    "AddConnection2",
                    CALL,
                    settings.add_connection2(&dict, proxy::TO_DISK, HashMap::new()),
                )
                .await
                .map(drop)
            }
            Err(_) => Ok(()),
        }
    }

    async fn apply(&self, step: &Step) -> Result<Applied, String> {
        let manager = self.manager().await?;
        let none = path("/")?;
        match step {
            Step::Update {
                uuid,
                settings,
                device,
            } => {
                let at = self.profile_path(uuid).await?;
                let profile = self.profile(at.as_str()).await?;
                nm_call(
                    "Update2",
                    CALL,
                    profile.update2(settings, proxy::IN_MEMORY, HashMap::new()),
                )
                .await?;
                let Some(device) = device else {
                    return Ok(Applied::default());
                };
                let device_at = self.device_path(device).await?;
                let current = self.active_on(device_at.as_str()).await;
                let active = match current {
                    Some((active, active_uuid)) if &active_uuid == uuid => {
                        // Reapply keeps the link up and only redoes addressing;
                        // what it cannot do live, a fresh activation does.
                        let proxy = self.device(device_at.as_str()).await?;
                        let empty = Dict::new();
                        match nm_call("Reapply", CALL, proxy.reapply(&empty, 0, 0)).await {
                            Ok(()) => active,
                            Err(_) => {
                                nm_call(
                                    "ActivateConnection",
                                    CALL,
                                    manager.activate_connection(&at, &device_at, &none),
                                )
                                .await?
                            }
                        }
                    }
                    _ => {
                        nm_call(
                            "ActivateConnection",
                            CALL,
                            manager.activate_connection(&at, &device_at, &none),
                        )
                        .await?
                    }
                };
                Ok(Applied {
                    active: Some(active.to_string()),
                    device: Some(device.clone()),
                    created: None,
                })
            }
            Step::Activate { uuid, device } => {
                let at = self.profile_path(uuid).await?;
                let device_at = match device {
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
                let mut interface = device.clone();
                if let Some(first) = devices.first() {
                    interface = self.interface_of(first.as_str()).await.ok().or(interface);
                }
                Ok(Applied {
                    active: Some(active.to_string()),
                    device: interface,
                    created: None,
                })
            }
            Step::Deactivate { uuid } => {
                let actives =
                    nm_call("ActiveConnections", CALL, manager.active_connections()).await?;
                for at in actives {
                    let proxy = self.active(at.as_str()).await?;
                    if nm_call("an active connection's uuid", CALL, proxy.uuid())
                        .await
                        .as_deref()
                        == Ok(uuid.as_str())
                    {
                        nm_call(
                            "DeactivateConnection",
                            CALL,
                            manager.deactivate_connection(&at),
                        )
                        .await?;
                        return Ok(Applied::default());
                    }
                }
                Err(format!("{uuid} is not active"))
            }
            Step::Delete { uuid } => {
                self.delete(uuid).await?;
                Ok(Applied::default())
            }
            Step::Add { settings, device } => {
                let device_at = self.device_path(device).await?;
                let options = HashMap::from([("persist", Value::from("memory"))]);
                let (profile_at, active, _) = nm_call(
                    "AddAndActivateConnection2",
                    CALL,
                    manager.add_and_activate_connection2(settings, &device_at, &none, options),
                )
                .await?;
                let profile = self.profile(profile_at.as_str()).await?;
                let created = nm_call("GetSettings", CALL, profile.get_settings())
                    .await
                    .ok()
                    .and_then(|dict| settings::uuid(&dict));
                Ok(Applied {
                    active: Some(active.to_string()),
                    device: Some(device.clone()),
                    created,
                })
            }
            Step::Radio { on } => {
                self.set_radio(*on).await?;
                Ok(Applied::default())
            }
        }
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

    async fn save(&self, uuid: &str) -> Result<(), String> {
        let at = self.profile_path(uuid).await?;
        let profile = self.profile(at.as_str()).await?;
        nm_call("Save", CALL, profile.save()).await
    }

    async fn delete(&self, uuid: &str) -> Result<(), String> {
        let Ok(at) = self.profile_path(uuid).await else {
            return Ok(());
        };
        let profile = self.profile(at.as_str()).await?;
        nm_call("Delete", CALL, profile.delete()).await
    }

    async fn radio(&self) -> Result<bool, String> {
        self.radios().await.map(|(enabled, _)| enabled)
    }

    async fn set_radio(&self, on: bool) -> Result<(), String> {
        let manager = self.manager().await?;
        nm_call("WirelessEnabled", CALL, manager.set_wireless_enabled(on)).await
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

    #[test]
    fn a_built_profile_converts() {
        let built = nmrs::builders::WifiConnectionBuilder::new("Office")
            .wpa_psk("hunter2hunter2")
            .build();
        let dict = to_dict(built).unwrap();
        assert!(settings::is_wifi(&dict));
        assert_eq!(settings::ssid(&dict).as_deref(), Some("Office"));
        assert!(settings::uuid(&dict).is_some());
        assert_eq!(
            settings::text(&dict, settings::WIFI_SECURITY, "key-mgmt").as_deref(),
            Some("wpa-psk")
        );
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
