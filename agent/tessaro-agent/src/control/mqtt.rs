//! The control plane's side of the MQTT link (`crate::mqtt`): its status
//! and test, the password and client certificate it connects with, what
//! it publishes of the settings, and the page's own subtree. The link
//! itself runs on its own task and reads the rest from here.

use std::sync::Arc;
use std::time::Instant;

use protocol::keys;
use protocol::mqtt::topic::Layout;
use protocol::mqtt::{MqttState, MqttStatus, MqttTested, Presence};
use protocol::{Done, Secret};
use serde_json::{json, Value};
use tessaro_mqtt::Message;

use super::{Caller, Control};
use crate::config::{self, MqttCommands};
use crate::mqtt::link::Target;
use crate::state;

/// The largest page message: what one tessaro.mqtt.publish() may send.
pub const PAGE_MESSAGE_MAX: usize = 256 * 1024;

/// A plain link that takes every command, said wherever the status is.
const PLAIN_AND_FULL: &str = "mqtt.url is not TLS and mqtt.commands is full: anyone who can publish to the broker, or see the link, has the device";

/// What a connection is made with: the settings filled in and the secrets.
#[derive(Clone)]
pub struct Plan {
    pub target: Target,
    pub layout: Layout,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
    pub keepalive: u16,
}

impl Control {
    /// Something the device did or saw, for whoever follows every event:
    /// the MQTT link (`crate::events`).
    pub fn emit(&self, source: &str, name: &str, detail: Value) {
        self.events.emit(source, name, detail);
    }

    pub fn events(&self) -> tokio::sync::broadcast::Receiver<protocol::mqtt::Event> {
        self.events.subscribe()
    }

    /// Where something outside the control plane - the state machine -
    /// emits its events.
    pub fn event_sink(&self) -> Arc<crate::events::Events> {
        Arc::clone(&self.events)
    }

    pub fn mqtt(&self) -> &crate::mqtt::Shared {
        &self.mqtt
    }

    /// The configuration as it changes, for the link to follow.
    pub fn follow(&self) -> config::Follow {
        self.current.subscribe()
    }

    /// mqtt.* as the running agent has them.
    pub fn current_mqtt(&self) -> config::Mqtt {
        self.current.borrow().config.mqtt.clone()
    }

    /// A line in the journal about the link.
    pub fn log_mqtt(&self, line: &str) {
        self.log.info(line);
    }

    /// What `<prefix>/<id>/status` says while the device is connected.
    pub fn presence_message(&self, commands: MqttCommands) -> Presence {
        let node = self.node();
        Presence {
            online: true,
            id: Some(node.id),
            name: Some(node.name),
            tags: node.tags,
            version: Some(node.version),
            machine: Some(node.machine),
            fingerprint: Some(node.fingerprint),
            claimed: Some(node.claimed),
            commands: Some(commands.name().to_string()),
        }
    }

    /// Where and how to connect for `config`, with `url` instead of
    /// mqtt.url when given: the password filled in from its placeholders,
    /// the client certificate when one is kept.
    pub async fn mqtt_plan(
        &self,
        config: &config::Mqtt,
        url: Option<&str>,
    ) -> Result<Plan, String> {
        let url = protocol::mqtt::parse_url(url.unwrap_or(&config.url))?;
        let secrets = self.read_secrets().await;
        let password = match &secrets.mqtt_password {
            Some(template) => {
                let settings = self.current.borrow().settings.clone();
                let live = self.live().await;
                Some(
                    state::expand_text(template.expose(), &settings, &self.defaults, &live)
                        .0
                        .into_bytes(),
                )
            }
            None => None,
        };
        let identity = match (&secrets.mqtt_client_cert, &secrets.mqtt_client_key) {
            (Some(cert), Some(key)) => Some((cert.expose().to_string(), key.expose().to_string())),
            _ => None,
        };
        Ok(Plan {
            target: Target {
                url,
                fingerprint: config.fingerprint.clone(),
                identity,
                proxy: self.proxy,
            },
            layout: Layout::new(&config.prefix, &self.identity.id),
            username: Some(config.username.clone()).filter(|name| !name.is_empty()),
            password,
            keepalive: config.keepalive.clamp(10, 3600) as u16,
        })
    }

    pub async fn mqtt_status(&self) -> MqttStatus {
        let config = self.current.borrow().config.mqtt.clone();
        let secrets = self.read_secrets().await;
        let link = self.mqtt.link();
        let broker = protocol::mqtt::parse_url(&config.url).ok();
        let mut warnings = Vec::new();
        if broker.as_ref().is_some_and(|url| !url.tls()) && config.commands == MqttCommands::Full {
            warnings.push(PLAIN_AND_FULL.to_string());
        }
        MqttStatus {
            state: link.state,
            broker: broker.map(|url| url.to_string()),
            client_id: self.identity.id.clone(),
            username: Some(config.username.clone()).filter(|name| !name.is_empty()),
            password: secrets.mqtt_password.is_some(),
            client_cert: secrets.mqtt_client_cert.is_some(),
            topic: format!("{}/{}", config.prefix, self.identity.id),
            commands: config.commands.name().to_string(),
            seconds: (link.state != MqttState::Off).then(|| link.since.elapsed().as_secs()),
            last_error: link.last_error,
            subscriptions: link.subscriptions,
            queued: link.queued,
            published: link.published,
            received: link.received,
            warnings,
        }
    }

    /// Connect with the saved settings, or `url` with them, and say how far
    /// it got. Nothing is kept, and the running link is left alone.
    pub(super) async fn mqtt_test(&self, url: Option<String>) -> Result<MqttTested, String> {
        let config = self.current.borrow().config.mqtt.clone();
        let url = url
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty());
        if url.is_none() && config.url.is_empty() {
            return Err("mqtt.url is not set; give a broker URL to test".to_string());
        }
        let plan = self.mqtt_plan(&config, url.as_deref()).await?;
        let broker = plan.target.url.to_string();
        let started = Instant::now();
        let outcome = crate::mqtt::probe(&plan).await; // naked: probe bounds every step with within()
        let millis = started.elapsed().as_millis() as u64;
        Ok(match outcome {
            Ok(fingerprint) => MqttTested {
                broker,
                ok: true,
                stage: None,
                error: None,
                fingerprint,
                millis,
            },
            Err((failure, fingerprint)) => MqttTested {
                broker,
                ok: false,
                stage: Some(failure.stage.to_string()),
                error: Some(failure.error),
                fingerprint,
                millis,
            },
        })
    }

    pub(super) async fn mqtt_password(
        &self,
        caller: &Caller,
        password: Option<Secret>,
    ) -> Result<Done, String> {
        if let Some(password) = &password {
            if password.expose().is_empty() {
                return Err("the password is empty; leave it out to forget it".to_string());
            }
            if password.expose().len() > 65535 {
                return Err("an MQTT password is at most 65535 bytes".to_string());
            }
        }
        let forget = password.is_none();
        self.update_secrets(move |secrets| secrets.mqtt_password = password)
            .await?;
        self.log.info(format!(
            "mqtt: the broker password {} by {}",
            if forget { "forgotten" } else { "set" },
            caller.describe()
        ));
        self.mqtt.reconnect();
        Ok(Done::new(if forget {
            "the broker password is forgotten"
        } else {
            "the broker password is kept; the link connects with it"
        }))
    }

    pub(super) async fn mqtt_cert_set(
        &self,
        caller: &Caller,
        cert: String,
        key: Secret,
    ) -> Result<Done, String> {
        if !cert.contains("-----BEGIN CERTIFICATE-----") {
            return Err("the certificate is not PEM (-----BEGIN CERTIFICATE-----)".to_string());
        }
        if !key.expose().contains("PRIVATE KEY-----") {
            return Err("the key is not a PEM private key".to_string());
        }
        // The pair has to load the way the link loads it.
        native_tls::Identity::from_pkcs8(cert.as_bytes(), key.expose().as_bytes())
            .map_err(|err| format!("the certificate and key do not go together: {err}"))?;
        self.update_secrets(move |secrets| {
            secrets.mqtt_client_cert = Some(Secret(cert));
            secrets.mqtt_client_key = Some(key);
        })
        .await?; // naked: update_secrets is one blocking() under within()
        self.log.info(format!(
            "mqtt: a client certificate set by {}",
            caller.describe()
        ));
        self.mqtt.reconnect();
        Ok(Done::new(
            "the client certificate is kept; the link connects with it",
        ))
    }

    pub(super) async fn mqtt_cert_clear(&self, caller: &Caller) -> Result<Done, String> {
        self.update_secrets(|secrets| {
            secrets.mqtt_client_cert = None;
            secrets.mqtt_client_key = None;
        })
        .await?; // naked: update_secrets is one blocking() under within()
        self.log.info(format!(
            "mqtt: the client certificate forgotten by {}",
            caller.describe()
        ));
        self.mqtt.reconnect();
        Ok(Done::new("the client certificate is forgotten"))
    }

    /// The settings as `state/config` shows them: what is set, by key, the
    /// proxy's password masked, and the revision.
    pub async fn mqtt_config_state(&self) -> Result<Value, String> {
        let state = self.read_state().await?;
        let settings: serde_json::Map<String, Value> = state
            .settings
            .iter()
            .map(|(key, value)| {
                let shown = match key.as_str() {
                    keys::PROXY_URL => keys::masked_proxy(value),
                    _ => value.clone(),
                };
                (key.clone(), Value::String(shown))
            })
            .collect();
        Ok(json!({ "revision": state.revision, "settings": settings }))
    }

    /// The value set for `key` now, `None` when it follows the default.
    pub fn setting_now(&self, key: &str) -> Option<String> {
        self.current.borrow().settings.get(key).cloned()
    }

    /// A message below the device's `page/in/`: to the page, as a
    /// `tessaro:mqtt` event, while mqtt.page is on.
    pub async fn mqtt_page_in(self: &Arc<Self>, sub: &str, message: &Message) {
        let text = String::from_utf8(message.payload.clone()).ok();
        let detail = json!({
            "topic": sub,
            "payload": text,
            "base64": match text {
                Some(_) => None,
                None => Some(openssl::base64::encode_block(&message.payload)),
            },
            "contentType": message.content_type,
        });
        self.bridge_mqtt(detail).await;
    }

    /// What the page publishes, below the device's `page/out/`.
    pub(super) async fn page_publish(
        &self,
        sub: &str,
        payload: Vec<u8>,
        retain: bool,
    ) -> Result<Done, String> {
        let config = self.current.borrow().config.mqtt.clone();
        if !config.page {
            return Err("mqtt.page is off".to_string());
        }
        protocol::mqtt::topic::check_sub(sub)?;
        if payload.len() > PAGE_MESSAGE_MAX {
            return Err(format!(
                "a page message is at most {PAGE_MESSAGE_MAX} bytes"
            ));
        }
        let layout = Layout::new(&config.prefix, &self.identity.id);
        let message = Message::new(layout.page_out(sub), payload).retain(retain);
        // naked: Shared::publish bounds handing it over with within()
        self.mqtt.publish(message).await?;
        Ok(Done::new("published"))
    }

    /// A page of the journal, for the link to publish.
    pub async fn journal(
        &self,
        lines: Option<u32>,
        cursor: Option<String>,
    ) -> Result<protocol::LogPage, String> {
        self.logs(None, lines, cursor).await
    }
}
