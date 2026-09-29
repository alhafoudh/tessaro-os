//! Golden fixtures for the result text: Webconfig's TypeScript port of
//! `describe` (webconfig/src/describe) renders the same inputs, and its
//! tests compare against the same files (docs/webconfig.md). A change to
//! the words here fails those tests until the port follows.
//!
//! Each file in `tests/describe/` names a function, holds its input and the
//! spans it renders to. `UPDATE_DESCRIBE=1 cargo test -p tessaro-client
//! --test describe` rewrites the outputs from the Rust.

use std::path::Path;

use serde_json::{json, Value};
use tessaro_client::describe::{audio, browser, device, net, time};
use tessaro_client::ping;
use tessaro_client::text::{Fact, Line};

fn line(line: &Line) -> Value {
    Value::Array(
        line.0
            .iter()
            .map(|span| {
                json!([
                    format!("{:?}", span.tone).to_lowercase(),
                    span.text,
                    span.width
                ])
            })
            .collect(),
    )
}

fn lines(lines: &[Line]) -> Value {
    Value::Array(lines.iter().map(line).collect())
}

fn facts(facts: &[Fact]) -> Value {
    Value::Array(
        facts
            .iter()
            .map(|fact| json!({ "label": fact.label, "value": line(&fact.value) }))
            .collect(),
    )
}

fn from<T: serde::de::DeserializeOwned>(input: &Value) -> T {
    serde_json::from_value(input.clone()).expect("the fixture's input")
}

/// What `function` renders `input` to.
fn render(function: &str, input: &Value) -> Value {
    match function {
        "device::status" => {
            let text = device::status(&from(input));
            json!({
                "facts": facts(&text.facts),
                "units": facts(&text.units),
                "more": facts(&text.more),
                "pending": text.pending.as_ref().map(line),
            })
        }
        "device::applied" => lines(&device::applied(
            &from(&input["applied"]),
            input["no_apply"].as_bool().unwrap_or(false),
        )),
        "device::eval" => match device::eval(&from(input)) {
            Ok(shown) => json!({ "line": line(&shown), "thrown": false }),
            Err(thrown) => json!({ "line": line(&thrown), "thrown": true }),
        },
        "audio::summary" => line(&audio::summary(&from(input))),
        "time::summary" => line(&time::summary(&from(input))),
        "ping::event_line" => {
            let events: Vec<protocol::PingEvent> = from(input);
            Value::Array(
                events
                    .iter()
                    .map(|event| line(&ping::event_line(event)))
                    .collect(),
            )
        }
        // --- screen and browser ---
        "browser::policies" => {
            let policies: Vec<protocol::policy::PolicyInfo> = from(input);
            lines(&browser::policies(&policies))
        }
        "browser::policy_saved" => {
            let cases: Vec<protocol::policy::PolicySaved> = from(input);
            Value::Array(
                cases
                    .iter()
                    .map(|one| lines(&browser::policy_saved(one)))
                    .collect(),
            )
        }
        "browser::policy_removed" => {
            let cases: Vec<protocol::policy::PolicyRemoved> = from(input);
            Value::Array(
                cases
                    .iter()
                    .map(|one| lines(&browser::policy_removed(one)))
                    .collect(),
            )
        }
        "browser::effective" => {
            let entries: Vec<protocol::policy::EffectiveEntry> = from(input);
            lines(&browser::effective(&entries))
        }
        // --- network, wifi and certificates ---
        "net::change" => lines(&net::change(&from(input))),
        "net::profile" => lines(&net::profile(&from(input))),
        "net::proxy_test" => {
            let tested: Vec<protocol::ProxyTested> = from(input);
            Value::Array(
                tested
                    .iter()
                    .map(|one| line(&net::proxy_test(one)))
                    .collect(),
            )
        }
        "net::cert" => {
            let certs: Vec<protocol::CertInfo> = from(input);
            Value::Array(certs.iter().map(|one| line(&net::cert(one))).collect())
        }
        "speedtest::event_line" => {
            let events: Vec<protocol::SpeedtestEvent> = from(input);
            let rendered = events
                .iter()
                .map(|event| line(&tessaro_client::speedtest::event_line(event)));
            Value::Array(rendered.collect())
        }
        // --- storage, audio, time and schedules ---
        "time::facts" => facts(&time::facts(&from(input))),
        "time::servers" => facts(&time::servers(&from(input))),
        "time::hint_and_error" => {
            let status: protocol::TimeStatus = from(input);
            json!({
                "hint": time::hint(&status).as_ref().map(line),
                "error": time::error(&status).as_ref().map(line),
            })
        }
        "audio::show" => lines(&audio::show(&from(input))),
        "audio::test" => lines(&audio::test(&from(input))),
        "storage::event_line" => {
            let steps: Vec<protocol::StorageGrowEvent> = from(input);
            Value::Array(
                steps
                    .iter()
                    .map(|step| {
                        tessaro_client::storage::event_line(step)
                            .as_ref()
                            .map_or(Value::Null, line)
                    })
                    .collect(),
            )
        }
        "storage::plan" => {
            let plan = tessaro_client::storage::Plan(from(input));
            let (shown, nothing) = plan.facts();
            json!({ "facts": facts(&shown), "nothing": nothing.as_ref().map(line), "grows": plan.grows() })
        }
        "schedule::moment" => {
            let now = input["now"].as_i64().unwrap();
            let moments: Vec<protocol::Moment> = from(&input["moments"]);
            Value::Array(
                moments
                    .iter()
                    .map(|at| line(&tessaro_client::schedule::moment(at, now)))
                    .collect(),
            )
        }
        "schedule::last_run" => {
            let now = input["now"].as_i64().unwrap();
            let infos: Vec<protocol::ScheduleInfo> = from(&input["infos"]);
            Value::Array(
                infos
                    .iter()
                    .map(|info| line(&tessaro_client::schedule::last_run(info, now)))
                    .collect(),
            )
        }
        "schedule::words" => {
            use tessaro_client::schedule::{duration, format_timeout, parse_timeout, relative};
            let seconds: Vec<u64> = from(&input["seconds"]);
            let pairs: Vec<(i64, i64)> = from(&input["relative"]);
            let typed: Vec<String> = from(&input["typed"]);
            json!({
                "format_timeout": seconds.iter().map(|s| format_timeout(*s)).collect::<Vec<_>>(),
                "duration": seconds.iter().map(|s| duration(*s)).collect::<Vec<_>>(),
                "relative": pairs.iter().map(|(at, now)| relative(*at, *now)).collect::<Vec<_>>(),
                "parse_timeout": typed.iter().map(|t| parse_timeout(t).ok()).collect::<Vec<_>>(),
            })
        }
        "clock::words" => {
            use tessaro_client::clock;
            let spans: Vec<u64> = from(&input["span"]);
            let offsets: Vec<i64> = from(&input["offset"]);
            let zones: Vec<i32> = from(&input["utc_offset"]);
            let precisions: Vec<i32> = from(&input["precision"]);
            let drifts: Vec<f64> = from(&input["drift"]);
            json!({
                "span": spans.iter().map(|v| clock::span(*v)).collect::<Vec<_>>(),
                "offset": offsets.iter().map(|v| clock::offset(*v)).collect::<Vec<_>>(),
                "utc_offset": zones.iter().map(|v| clock::utc_offset(*v)).collect::<Vec<_>>(),
                "precision": precisions.iter().map(|v| clock::precision(*v)).collect::<Vec<_>>(),
                "drift": drifts.iter().map(|v| clock::drift(*v)).collect::<Vec<_>>(),
            })
        }
        // --- access, ssh, files, update and log ---
        "update::status_lines" => lines(&tessaro_client::update::status_lines(&from(input))),
        "update::warning" => {
            let plan = tessaro_client::update::Plan {
                image: input["name"].as_str().unwrap().into(),
                bmap: "x.bmap".into(),
                wipe_data: input["wipe_data"].as_bool().unwrap(),
                repartition: input["repartition"].as_bool().unwrap(),
                verify: true,
                reboot: true,
            };
            json!(plan.warning(input["name"].as_str().unwrap()))
        }
        "transfer::date" => {
            let times: Vec<i64> = from(input);
            json!(times
                .iter()
                .map(|time| tessaro_client::transfer::date(*time))
                .collect::<Vec<_>>())
        }
        "files::summary_line" => {
            let list = |name: &str| -> Vec<String> { from(&input[name]) };
            let summary = tessaro_client::files::Summary {
                sent: list("sent"),
                received: list("received"),
                removed: list("removed"),
                made: list("made"),
                unchanged: list("unchanged"),
                skipped: list("skipped"),
                bytes: input["bytes"].as_u64().unwrap(),
            };
            line(&summary.line(input["verb"].as_str().unwrap()))
        }
        "journal::parse" => {
            let events: Vec<Value> = from(input);
            Value::Array(
                events
                    .iter()
                    .map(|event| {
                        let entry = tessaro_client::journal::Entry::parse(event);
                        json!({
                            "source": entry.source,
                            "priority": entry.priority,
                            "message": entry.message,
                            "clock": entry.clock(),
                        })
                    })
                    .collect(),
            )
        }
        other => panic!("no renderer for {other}; add it to tests/describe.rs"),
    }
}

#[test]
fn the_fixtures_render_as_recorded() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/describe");
    let update = std::env::var_os("UPDATE_DESCRIBE").is_some();
    let mut stale = Vec::new();
    let mut seen = 0;
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/describe")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    for path in paths {
        seen += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let mut fixture: Value = serde_json::from_str(&text).unwrap();
        let function = fixture["function"].as_str().unwrap().to_string();
        let output = render(&function, &fixture["input"]);
        if fixture["output"] != output {
            if update {
                fixture["output"] = output;
                let text = format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap());
                std::fs::write(&path, text).unwrap();
            } else {
                stale.push(path.display().to_string());
            }
        }
    }
    assert!(seen > 0, "no fixtures in {}", dir.display());
    assert!(
        stale.is_empty(),
        "these render differently now; `UPDATE_DESCRIBE=1 cargo test -p tessaro-client \
         --test describe`, then bring webconfig/src/describe along:\n{}",
        stale.join("\n")
    );
}
