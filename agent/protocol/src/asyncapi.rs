//! The MQTT link's AsyncAPI 3.0 document: every topic a device publishes
//! and takes (`mqtt::topic`), and a command channel for every endpoint of
//! `api`, with the JSON Schemas schemars derives from the same types the
//! HTTP API's OpenAPI document is built from (`openapi`), so the two cannot
//! drift. `agent/protocol/asyncapi.json` is the document checked in, for
//! code generators and integrators; a test in `tessaro-agent`'s `api`
//! fails when it is stale.

use schemars::generate::SchemaSettings;
use serde_json::{json, Map, Value};

use crate::api::{self, ApiError};
use crate::mqtt::{envelope, Event, MqttStatus, Presence};

/// The document, for a device of `version`.
pub fn document(version: &str) -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.definitions_path = "/components/schemas".into())
        .into_generator();
    let error = generator.subschema_for::<ApiError>();
    let presence = generator.subschema_for::<Presence>();
    let event = generator.subschema_for::<Event>();
    let mqtt = generator.subschema_for::<MqttStatus>();
    let status = generator.subschema_for::<crate::Status>();

    let device = json!({
        "prefix": { "description": "mqtt.prefix, filled in (default tessaro)." },
        "device": { "description": "The device's node id, device.id: its MQTT client id too." },
    });
    let mut channels = Map::new();
    let mut operations = Map::new();
    let mut add = |name: &str, channel: Value, operation: Value| {
        channels.insert(name.to_string(), channel);
        operations.insert(name.to_string(), operation);
    };

    add(
        "status",
        json!({
            "address": "{prefix}/{device}/status",
            "parameters": device,
            "description": "Retained: the device and whether it is connected. The will, published by the \
                broker when the link drops without a goodbye, is `{\"online\": false}`.",
            "messages": { "status": { "payload": presence.to_value() } },
        }),
        send("status"),
    );
    add(
        "state",
        json!({
            "address": "{prefix}/{device}/state/{part}",
            "parameters": {
                "prefix": device["prefix"], "device": device["device"],
                "part": { "enum": ["device", "config", "mqtt"], "description": "device: the status; \
                    config: the settings that are set, with the revision; mqtt: the link." },
            },
            "description": "Retained snapshots, published when they change and every mqtt.state.interval seconds.",
            "messages": {
                "device": { "payload": status.to_value() },
                "config": { "payload": { "type": "object", "properties": {
                    "revision": { "type": "integer" },
                    "settings": { "type": "object", "additionalProperties": { "type": "string" } },
                } } },
                "mqtt": { "payload": mqtt.to_value() },
            },
        }),
        send("state"),
    );
    add(
        "event",
        json!({
            "address": "{prefix}/{device}/event/{source}/{name}",
            "parameters": {
                "prefix": device["prefix"], "device": device["device"],
                "source": { "description": "What it happened to: cec, presence, scanner, config, script, \
                    browser, display, printer, player, update." },
                "name": { "description": "What happened." },
            },
            "description": "Everything the device did or saw, QoS 1, kept for the broker while the link is down.",
            "messages": { "event": { "payload": event.to_value() } },
        }),
        send("event"),
    );
    add(
        "jobs",
        json!({
            "address": "{prefix}/{device}/jobs/{job}",
            "parameters": { "prefix": device["prefix"], "device": device["device"],
                "job": { "description": "The id a command that runs as a job answered with." } },
            "description": "A job's steps as they happen, then its end.",
            "messages": { "step": { "payload": { "oneOf": [
                { "type": "object", "properties": { "index": { "type": "integer" }, "step": {} } },
                { "type": "object", "properties": { "done": { "type": "boolean" },
                    "error": { "type": ["string", "null"] } } },
            ] } } },
        }),
        send("jobs"),
    );
    add(
        "frame",
        json!({
            "address": "{prefix}/{device}/screen/frame",
            "parameters": device,
            "description": "What is on screen, a JPEG, every mqtt.screenshot.interval seconds when it changed. QoS 0.",
            "messages": { "frame": { "contentType": "image/jpeg", "payload": { "type": "string", "format": "binary" } } },
        }),
        send("frame"),
    );
    add(
        "logs",
        json!({
            "address": "{prefix}/{device}/logs",
            "parameters": device,
            "description": "The journal, an entry a message, from mqtt.logs' priority up. QoS 0.",
            "messages": { "entry": { "payload": { "type": "object" } } },
        }),
        send("logs"),
    );
    add(
        "pageOut",
        json!({
            "address": "{prefix}/{device}/page/out/{topic}",
            "parameters": { "prefix": device["prefix"], "device": device["device"],
                "topic": { "description": "Levels the page names: tessaro.mqtt.publish(topic, payload)." } },
            "description": "What the kiosk page publishes.",
            "messages": { "message": { "payload": {} } },
        }),
        send("pageOut"),
    );
    add(
        "pageIn",
        json!({
            "address": "{prefix}/{device}/page/in/{topic}",
            "parameters": { "prefix": device["prefix"], "device": device["device"],
                "topic": { "description": "Levels of the publisher's: the page gets them as tessaro:mqtt." } },
            "description": "What reaches the kiosk page, as a tessaro:mqtt event with its topic below page/in/.",
            "messages": { "message": { "payload": {} } },
        }),
        receive("pageIn"),
    );
    channels.insert(
        "reply".into(),
        json!({
            "address": null,
            "description": "The request's response topic. The correlation data comes back as it went; the \
                `status` user property is `ok` or the refusal's code, `device` names who answered.",
            "messages": { "refused": { "payload": error.to_value() } },
        }),
    );

    for route in api::all() {
        let schemas = (route.schemas)(&mut generator);
        let name = format!("{}.{}", route.tag(), route.name);
        let (summary, _) = route
            .doc
            .trim()
            .split_once("\n\n")
            .unwrap_or((route.doc.trim(), ""));
        let request = match route.raw_body {
            true => json!({
                "contentType": "application/octet-stream",
                "payload": { "type": "string", "format": "binary" },
                "headers": { "type": "object", "properties": { envelope::PARAMS: {
                    "type": "string", "description": "The params, as a JSON object." } } },
            }),
            false => json!({
                "payload": { "type": "object", "properties": {
                    "params": schemas.params.to_value(),
                    "body": schemas.body.to_value(),
                } },
            }),
        };
        let answer = match route.raw_response {
            Some(content_type) => {
                json!({ "contentType": content_type, "payload": { "type": "string", "format": "binary" } })
            }
            None => json!({ "payload": schemas.response.to_value() }),
        };
        channels.insert(
            name.clone(),
            json!({
                "address": format!("{{prefix}}/{{device}}/cmd/{}/{}", route.tag(), route.name),
                "parameters": device,
                "description": format!("{} The same as {} {} over HTTP. Also on {{prefix}}/tag/TAG/cmd/{}/{} for every device with TAG.",
                    unwrap(summary), route.method.as_str(), route.path, route.tag(), route.name),
                "messages": { "request": request, "answer": answer },
            }),
        );
        operations.insert(
            name.clone(),
            json!({
                "action": "receive",
                "channel": { "$ref": format!("#/channels/{name}") },
                "summary": unwrap(summary),
                "messages": [{ "$ref": format!("#/channels/{name}/messages/request") }],
                "reply": {
                    "channel": { "$ref": "#/channels/reply" },
                    "messages": [
                        { "$ref": format!("#/channels/{name}/messages/answer") },
                        { "$ref": "#/channels/reply/messages/refused" },
                    ],
                },
            }),
        );
    }

    let schemas: Map<String, Value> = generator.take_definitions(true).into_iter().collect();
    json!({
        "asyncapi": "3.0.0",
        "info": {
            "title": "Tessaro device over MQTT",
            "version": version,
            "description": "What a Tessaro device publishes to its MQTT broker and what it takes from it, \
                once mqtt.url is set (docs/mqtt.md). MQTT v5: a request names its response topic and \
                correlation data. The broker is trusted: whoever may publish to a device's command \
                topics has as much of it as mqtt.commands gives.",
        },
        "defaultContentType": envelope::JSON,
        "channels": channels,
        "operations": operations,
        "components": { "schemas": schemas },
    })
}

fn send(channel: &str) -> Value {
    json!({ "action": "send", "channel": { "$ref": format!("#/channels/{channel}") } })
}

fn receive(channel: &str) -> Value {
    json!({ "action": "receive", "channel": { "$ref": format!("#/channels/{channel}") } })
}

fn unwrap(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_endpoint_has_a_command_channel_and_every_ref_resolves() {
        let document = document("test");
        for route in api::all() {
            let name = format!("{}.{}", route.tag(), route.name);
            assert!(document["channels"][&name]["address"].is_string(), "{name}");
            assert_eq!(document["operations"][&name]["action"], "receive");
        }
        let text = document.to_string();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let mut at = 0;
        while let Some(found) = text[at..].find("#/components/schemas/") {
            let start = at + found + "#/components/schemas/".len();
            let end = start + text[start..].find('"').unwrap();
            assert!(
                schemas.contains_key(&text[start..end]),
                "{} is not in the components",
                &text[start..end]
            );
            at = end;
        }
    }
}
