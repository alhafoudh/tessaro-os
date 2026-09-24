//! The network glue: joining a WiFi network, the hotspot's password, and the
//! profiles as `state.json` and `secrets.json` say they are. The profiles and
//! the transaction that switches them are `nm`.

use std::collections::BTreeMap;
use std::sync::Arc;

use protocol::keys;
use protocol::{HotspotCredentials, Secret};

use super::{Caller, Control, Reply};
use crate::deadline::blocking;
use crate::nm::profiles::{self, NetConfig};
use crate::secrets::{self, Secrets};

impl Control {
    /// `net wifi join`: client mode on `ssid`, as one change of the network
    /// keys, with the password staged so it is saved only if the join holds.
    pub(super) async fn join(
        self: &Arc<Self>,
        caller: &Caller,
        ssid: String,
        psk: Option<Secret>,
        security: Option<protocol::WifiSecurity>,
        hidden: bool,
        verify: protocol::Verify,
    ) -> Reply {
        let key = keys::find("network.wifi.ssid").expect("network.wifi.ssid is a key");
        if let Err(err) = keys::validate(key, &ssid) {
            return Reply::err(err);
        }
        if let Some(psk) = &psk {
            if let Err(err) = keys::check_psk(psk.expose()) {
                return Reply::err(err);
            }
        }
        let state = match self.read_state().await {
            Ok(state) => state,
            Err(err) => return Reply::err(err),
        };
        let value = profiles::value_of(&state.settings, &self.defaults);
        let interface = match profiles::effective(&value, "network.wifi.interface").as_str() {
            "auto" => "wlan0".to_string(),
            name => name.to_string(),
        };
        let security = match security {
            Some(security) => security,
            None if hidden => {
                return Reply::err("a hidden network needs --security psk, sae or open")
            }
            None => match self.network.security_of_ssid(&interface, &ssid).await {
                Ok(security) => security,
                Err(err) => return Reply::err(err),
            },
        };
        let word = match security {
            protocol::WifiSecurity::Psk => "psk",
            protocol::WifiSecurity::Sae => "sae",
            protocol::WifiSecurity::Open => "open",
        };
        // Without a new password, only the network whose password is stored
        // can be joined: another one would try it with the wrong one.
        let same_network = profiles::effective(&value, "network.wifi.ssid") == ssid;
        if psk.is_none() && word != "open" && !same_network {
            return Reply::err(format!("{ssid} needs a password"));
        }

        let changes = BTreeMap::from([
            ("network.wifi.mode".to_string(), Some("client".to_string())),
            ("network.wifi.ssid".to_string(), Some(ssid)),
            ("network.wifi.security".to_string(), Some(word.to_string())),
            (
                "network.wifi.hidden".to_string(),
                Some(if hidden { "1" } else { "0" }.to_string()),
            ),
        ]);
        self.change(caller, changes, None, true, verify, psk).await
    }

    /// A new random hotspot password, shown once. Unclaimed devices keep an
    /// open hotspot, like an empty root password.
    pub(super) async fn hotspot_password(
        &self,
        caller: &Caller,
    ) -> Result<HotspotCredentials, String> {
        let _writes = self.writes.lock().await;
        self.require_claimed("its hotspot stays open until claiming sets a password")?;
        let password = secrets::random_hotspot_psk()?;
        let stored = Secret(password.clone());
        self.update_secrets(move |secrets| secrets.hotspot_psk = Some(stored))
            .await?;
        self.log.info(format!(
            "network: a new hotspot password was set by {}",
            caller.describe()
        ));
        let config = self.net_config().await?;
        Ok(HotspotCredentials {
            ssid: config.wifi.hotspot_ssid,
            password,
        })
    }

    /// A new hotspot password, stored; the credentials to show, when the
    /// device has its WiFi interface at all. The profiles are re-rendered
    /// afterwards, by `After::Network`.
    pub(super) async fn set_hotspot_psk(&self) -> Result<Option<HotspotCredentials>, String> {
        let password = secrets::random_hotspot_psk()?;
        let stored = Secret(password.clone());
        self.update_secrets(move |secrets| secrets.hotspot_psk = Some(stored))
            .await?;
        let config = self.net_config().await?;
        let here = self.network.has_wifi(&config.wifi.interface).await;
        Ok(here.then_some(HotspotCredentials {
            ssid: config.wifi.hotspot_ssid,
            password,
        }))
    }

    pub(super) async fn read_secrets(&self) -> Secrets {
        let store = self.secrets.clone();
        let log = Arc::clone(&self.log);
        blocking("reading secrets.json", move || {
            Ok(store.read::<Secrets>(&log))
        })
        .await
        .unwrap_or_default()
    }

    pub(super) async fn update_secrets(
        &self,
        change: impl FnOnce(&mut Secrets) + Send + 'static,
    ) -> Result<(), String> {
        let store = self.secrets.clone();
        let log = Arc::clone(&self.log);
        blocking("updating secrets.json", move || {
            store.update(&log, |secrets: &mut Secrets| {
                change(secrets);
                Ok(())
            })
        })
        .await
    }

    /// The node name these settings give, as the hotspot is named after it.
    pub(super) fn node_name_for(&self, settings: &BTreeMap<String, String>) -> String {
        let value = profiles::value_of(settings, &self.defaults);
        profiles::node_name(&value, &crate::identity::friendly_name(&self.identity.id))
    }

    pub(super) fn net_config_for(
        &self,
        settings: &BTreeMap<String, String>,
        secrets: &Secrets,
    ) -> NetConfig {
        let value = profiles::value_of(settings, &self.defaults);
        NetConfig::from_settings(
            &value,
            secrets.hotspot_psk.clone(),
            secrets.wifi_psk.clone(),
            &self.node_name_for(settings),
        )
    }

    /// The network as `state.json` and `secrets.json` say it is.
    pub(super) async fn net_config(&self) -> Result<NetConfig, String> {
        let state = self.read_state().await?;
        let secrets = self.read_secrets().await;
        Ok(self.net_config_for(&state.settings, &secrets))
    }

    /// Roll back a network change the previous agent never finished, onto
    /// what the saved settings say.
    pub async fn recover_network(&self) {
        match self.net_config().await {
            Ok(config) => self.network.recover(config),
            Err(err) => self.log.info(format!("network: {err}")),
        }
    }

    /// Re-render the profiles from the saved settings, outside a change:
    /// after a claim, an unclaim or a new hotspot password.
    pub(super) async fn refresh_network(&self) {
        let config = match self.net_config().await {
            Ok(config) => config,
            Err(err) => {
                self.log.info(format!("network: {err}"));
                return;
            }
        };
        if let Err(err) = self.network.refresh(&config).await {
            self.log
                .info(format!("network: re-rendering the profiles: {err}"));
        }
    }
}
