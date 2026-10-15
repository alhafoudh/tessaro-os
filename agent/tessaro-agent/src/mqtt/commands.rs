//! A command from the broker: the endpoint its topic names, run exactly as
//! the HTTP API runs it (`api::Dispatch`), and its answer to the request's
//! response topic (`protocol::mqtt::envelope` says how both travel).
//!
//! A change to `mqtt.*` or `network.*` that arrives this way can cut the
//! very link it came over, and then nothing could put it back. So such a
//! change is on probation: unless the link is up again `PROBATION` after
//! it, the keys go back to what they were.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use protocol::api::{self, Action, Answer, ApiError, ErrorCode, Method};
use protocol::mqtt::{envelope, MqttState};
use protocol::Command;
use tessaro_mqtt::Message;
use tokio::sync::{mpsc, oneshot};

use super::Out;
use crate::api::Dispatch;
use crate::config::MqttCommands;
use crate::control::{Caller, Control};

/// How long the link has to be back after a change that may cut it.
const PROBATION: Duration = Duration::from_secs(60);

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    control: &Arc<Control>,
    dispatch: &Dispatch,
    outbox: &mpsc::Sender<Out>,
    mode: MqttCommands,
    id: &str,
    group: &str,
    name: &str,
    request: Message,
) {
    let client = request
        .response_topic
        .clone()
        .unwrap_or_else(|| "no answer asked".to_string());
    let caller = Caller::Mqtt { client };
    let routes = api::all();
    let refuse = |code: ErrorCode, why: String| (Err::<Answer, _>((code, why)), None);
    let (result, after) = match api::by_name(&routes, group, name) {
        None => refuse(
            ErrorCode::NotFound,
            format!("{group}/{name} is not an endpoint; the API's document lists them"),
        ),
        Some(route) if mode == MqttCommands::Read && route.method != Method::Get => refuse(
            ErrorCode::Refused,
            "mqtt.commands is read: only what reads may be asked over MQTT".to_string(),
        ),
        Some(route) => {
            match envelope::read_request(&request.payload, &request.user, route.raw_body) {
                Err(err) => refuse(ErrorCode::BadRequest, err),
                Ok(decoded) => match (route.decode)(&decoded.params, &decoded.body) {
                    Err(err) => refuse(ErrorCode::BadRequest, err),
                    Ok(action) => {
                        let watched = watched_keys(&action);
                        let before: BTreeMap<String, Option<String>> = watched
                            .iter()
                            .map(|key| (key.clone(), control.setting_now(key)))
                            .collect();
                        // naked: act bounds every call it makes through Control
                        let done = dispatch.act(&caller, route, action).await;
                        if let (Ok(Answer::Json(applied)), false) =
                            (&done.result, before.is_empty())
                        {
                            if let Some(revision) = applied["revision"].as_u64() {
                                probation(Arc::clone(control), before, revision);
                            }
                        }
                        (done.result, done.after)
                    }
                },
            }
        }
    };

    let (status, payload, content_type, headers) = match result {
        Ok(Answer::Json(value)) => (
            envelope::OK.to_string(),
            value.to_string().into_bytes(),
            envelope::JSON,
            Vec::new(),
        ),
        Ok(Answer::Raw(raw)) => (
            envelope::OK.to_string(),
            raw.body,
            raw.content_type,
            raw.headers
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
        ),
        Err((code, error)) => {
            let body = serde_json::json!(ApiError { error, code });
            let status = serde_json::to_value(code)
                .ok()
                .and_then(|code| code.as_str().map(str::to_string))
                .unwrap_or_else(|| "refused".to_string());
            (
                status,
                body.to_string().into_bytes(),
                envelope::JSON,
                Vec::new(),
            )
        }
    };
    let answer = super::run::answer(&request, id, &status, payload, content_type, &headers);
    let (sent, out) = oneshot::channel();
    match answer {
        Some(message) => {
            let out = Out::Publish {
                message,
                sent: Some(sent),
            };
            // naked: the outbox, which the link empties as fast as it writes
            let _ = outbox.send(out).await;
        }
        None => {
            let _ = sent.send(());
        }
    }
    if let Some(after) = after {
        dispatch.after(after, out);
    }
}

/// The keys of a change that may cut the link: `mqtt.*` and `network.*`.
fn watched_keys(action: &Action) -> Vec<String> {
    let keys: Vec<&String> = match action {
        Action::Run(Command::Set { values, .. }) => values.keys().collect(),
        Action::Run(Command::Unset { keys, .. }) => keys.iter().collect(),
        _ => Vec::new(),
    };
    keys.into_iter()
        .filter(|key| key.starts_with("mqtt.") || key.starts_with("network."))
        .cloned()
        .collect()
}

/// Unless the link is up `PROBATION` after the change that made
/// `revision`, put `before` back - and only if nothing changed since.
fn probation(control: Arc<Control>, before: BTreeMap<String, Option<String>>, revision: u64) {
    tokio::spawn(async move {
        // naked: a pause, the probation itself
        tokio::time::sleep(PROBATION).await;
        if control.mqtt().link().state == MqttState::Connected {
            return;
        }
        let keys: Vec<&str> = before.keys().map(String::as_str).collect();
        control.log_mqtt(&format!(
            "mqtt: the link is not back {}s after {} changed over it; putting them back",
            PROBATION.as_secs(),
            keys.join(" ")
        ));
        let caller = Caller::Mqtt {
            client: "its own rollback".to_string(),
        };
        let set: BTreeMap<String, String> = before
            .iter()
            .filter_map(|(key, value)| value.clone().map(|value| (key.clone(), value)))
            .collect();
        let unset: Vec<String> = before
            .iter()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.clone())
            .collect();
        let mut revision = Some(revision);
        if !set.is_empty() {
            let reply = control
                .handle(
                    &caller,
                    Command::Set {
                        values: set,
                        if_revision: revision,
                        apply: true,
                        verify: Default::default(),
                    },
                )
                // naked: Control bounds every call it makes
                .await;
            match reply.result {
                Ok(applied) => revision = applied["revision"].as_u64(),
                Err(err) => {
                    control.log_mqtt(&format!("mqtt: putting the settings back: {err}"));
                    return;
                }
            }
            if let Some(after) = reply.after {
                // naked: run_after is under within() inside Bus
                control.run_after(after).await;
            }
        }
        if !unset.is_empty() {
            let reply = control
                .handle(
                    &caller,
                    Command::Unset {
                        keys: unset,
                        if_revision: revision,
                        apply: true,
                        verify: Default::default(),
                    },
                )
                // naked: Control bounds every call it makes
                .await;
            if let Err(err) = &reply.result {
                control.log_mqtt(&format!("mqtt: putting the settings back: {err}"));
            }
            if let Some(after) = reply.after {
                // naked: run_after is under within() inside Bus
                control.run_after(after).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_link_and_network_keys_are_on_probation() {
        let set = Action::Run(Command::Set {
            values: BTreeMap::from([
                ("mqtt.url".to_string(), "mqtt://b".to_string()),
                ("network.wifi.mode".to_string(), "client".to_string()),
                ("browser.url".to_string(), "https://x.test".to_string()),
            ]),
            if_revision: None,
            apply: true,
            verify: Default::default(),
        });
        assert_eq!(watched_keys(&set), vec!["mqtt.url", "network.wifi.mode"]);
        let unset = Action::Run(Command::Unset {
            keys: vec!["audio.volume".to_string()],
            if_revision: None,
            apply: true,
            verify: Default::default(),
        });
        assert!(watched_keys(&unset).is_empty());
        assert!(watched_keys(&Action::Run(Command::Status)).is_empty());
    }
}
