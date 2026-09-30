//! The API's OpenAPI 3.1 document, built from the endpoint types in `api`.
//!
//! Every schema is the JSON Schema schemars derives from the Rust type, so
//! the document cannot describe a field the code does not have. The agent
//! serves it at `/api/v1/openapi.json`; `agent/protocol/openapi.json` is the
//! same document checked in, for code generators, and a test in
//! tessaro-agent fails when the two differ.

use schemars::generate::SchemaSettings;
use serde_json::{json, Map, Value};

use crate::api::{self, ApiError, Route};

const REFS: &str = "#/components/schemas/";

/// The document, for an agent of `version`.
pub fn document(version: &str) -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.definitions_path = "/components/schemas".into())
        .into_generator();
    let error = generator.subschema_for::<ApiError>();

    let mut paths: Map<String, Value> = Map::new();
    let mut tags: Vec<&str> = Vec::new();
    for route in api::all() {
        let schemas = (route.schemas)(&mut generator);
        let params = resolve(&generator, schemas.params.as_value().clone());
        let operation = operation(&route, &params, schemas, &error);
        if !tags.contains(&route.tag()) {
            tags.push(route.tag());
        }
        paths
            .entry(route.path)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("a path is an object")
            .insert(route.method.as_str().to_lowercase(), operation);
    }

    let schemas: Map<String, Value> = generator.take_definitions(true).into_iter().collect();
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Tessaro device API",
            "version": version,
            "description": "The API of one Tessaro device, over HTTPS on port 7400 with the \
                device's own self-signed certificate: pin its SHA-256 fingerprint \
                (`/api/v1/device/id`, the mDNS `fp` record). An unclaimed device answers \
                everything without a token; once claimed, everything but identification, \
                the claim and signing in needs `Authorization: Bearer <token>`, or a \
                Webconfig browser session's cookie.",
        },
        "tags": tags.iter().map(|tag| json!({ "name": tag })).collect::<Vec<_>>(),
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {
                "token": { "type": "http", "scheme": "bearer" },
                "session": {
                    "type": "apiKey",
                    "in": "cookie",
                    "name": "__Host-tessaro-{node id}",
                    "description": "Set by signing in (`access/session`, `access/ticket/redeem`) \
                        or claiming from a browser. Browser requests must come from the device's \
                        own origin.",
                },
            },
        },
        "security": [{ "token": [] }, { "session": [] }],
    })
}

fn operation(
    route: &Route,
    params: &Value,
    schemas: api::Schemas,
    error: &schemars::Schema,
) -> Value {
    let (summary, description) = match route.doc.trim().split_once("\n\n") {
        Some((summary, rest)) => (summary, rest),
        None => (route.doc.trim(), ""),
    };
    let mut operation = Map::new();
    operation.insert(
        "operationId".into(),
        json!(format!("{}.{}", route.tag(), route.name)),
    );
    operation.insert("tags".into(), json!([route.tag()]));
    operation.insert("summary".into(), json!(unwrap_lines(summary)));
    if !description.is_empty() {
        operation.insert("description".into(), json!(unwrap_lines(description)));
    }
    if route.public {
        operation.insert("security".into(), json!([]));
    }

    let parameters = parameters(route, params);
    if !parameters.is_empty() {
        operation.insert("parameters".into(), Value::Array(parameters));
    }

    if route.raw_body {
        operation.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": { "application/octet-stream": {
                    "schema": { "type": "string", "format": "binary" }
                }},
            }),
        );
    } else if !route.bodiless {
        operation.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": { "application/json": { "schema": schemas.body.to_value() } },
            }),
        );
    }

    let answer = match route.raw_response {
        Some(content_type) => {
            let mut answer = json!({
                "description": "The bytes.",
                "content": { content_type: { "schema": { "type": "string", "format": "binary" } } },
            });
            if !route.raw_headers.is_empty() {
                let headers: Map<String, Value> = route
                    .raw_headers
                    .iter()
                    .map(|(name, doc)| {
                        let header = json!({ "description": doc, "schema": { "type": "string" } });
                        (name.to_string(), header)
                    })
                    .collect();
                answer["headers"] = Value::Object(headers);
            }
            answer
        }
        None => json!({
            "description": "Done.",
            "content": { "application/json": { "schema": schemas.response.to_value() } },
        }),
    };
    operation.insert(
        "responses".into(),
        json!({
            "200": answer,
            "default": {
                "description": "Refused or failed: `code` says which, `error` why.",
                "content": { "application/json": { "schema": error.as_value() } },
            },
        }),
    );
    Value::Object(operation)
}

/// A `$ref` to a named schema, as the schema itself.
fn resolve(generator: &schemars::SchemaGenerator, schema: Value) -> Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => reference
            .strip_prefix(REFS)
            .and_then(|name| generator.definitions().get(name))
            .cloned()
            .unwrap_or(schema),
        None => schema,
    }
}

/// `Params`' fields as path and query parameters.
fn parameters(route: &Route, params: &Value) -> Vec<Value> {
    let required: Vec<&str> = params["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = params["properties"].as_object() else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, schema)| {
            let in_path = route.path.contains(&format!("{{{name}}}"));
            let mut schema = schema.clone();
            let description = schema
                .as_object_mut()
                .and_then(|schema| schema.remove("description"));
            let mut parameter = json!({
                "name": name,
                "in": if in_path { "path" } else { "query" },
                "required": in_path || required.contains(&name.as_str()),
                "schema": schema,
            });
            if let Some(description) = description {
                parameter["description"] = description;
            }
            parameter
        })
        .collect()
}

/// A doc comment's lines as one paragraph.
fn unwrap_lines(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_has_an_answer_and_every_ref_resolves() {
        let document = document("test");
        let text = document.to_string();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let mut at = 0;
        while let Some(found) = text[at..].find(REFS) {
            let start = at + found + REFS.len();
            let end = start + text[start..].find('"').unwrap();
            let name = &text[start..end];
            assert!(
                schemas.contains_key(name),
                "{name} is not in the components"
            );
            at = end;
        }
        for (path, operations) in document["paths"].as_object().unwrap() {
            for (method, operation) in operations.as_object().unwrap() {
                assert!(
                    operation["responses"]["200"].is_object(),
                    "{method} {path} has no answer"
                );
            }
        }
    }

    #[test]
    fn identification_needs_no_token_and_the_rest_does() {
        let document = document("test");
        assert_eq!(
            document["paths"]["/api/v1/device/id"]["get"]["security"],
            json!([])
        );
        assert!(document["paths"]["/api/v1/device/status"]["get"]
            .get("security")
            .is_none());
        assert_eq!(
            document["security"],
            json!([{ "token": [] }, { "session": [] }])
        );
    }

    #[test]
    fn a_download_declares_its_headers() {
        let document = document("test");
        let headers =
            &document["paths"]["/api/v1/files/content"]["get"]["responses"]["200"]["headers"];
        assert!(headers[api::HEADER_SIZE].is_object());
        assert!(headers[api::HEADER_MTIME].is_object());
    }

    #[test]
    fn a_query_and_a_path_become_parameters() {
        let document = document("test");
        let scan = &document["paths"]["/api/v1/network/wifi/scan"]["get"]["parameters"];
        let names: Vec<&str> = scan
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["interface", "rescan"]);
        let run = &document["paths"]["/api/v1/scripts/{script}/run"]["post"]["parameters"][0];
        assert_eq!(run["in"], "path");
        assert_eq!(run["required"], true);
    }
}
