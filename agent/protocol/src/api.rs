//! The HTTP API: every endpoint, once, as a type.
//!
//! An endpoint is a zero-sized type implementing `Endpoint`: its method and
//! path, the query it takes (`Params`), its JSON body (`Body`) and what it
//! answers (`Response`). The agent routes on them (`routes`), the OpenAPI
//! document is built from them (`crate::openapi`), and a client calls one
//! by its type, so asking for the wrong answer or sending the wrong body is
//! a compile error, not a runtime surprise. docs/api.md says how the API
//! behaves; this file is what it is.
//!
//! What the agent does for a request is an `Action`: mostly one `Command`,
//! the same model the page bridge maps its calls onto.
//!
//! Paths are `/api/v1/<group>/...`, the group being the `tessaro-ctl`
//! command group that does the same thing. `{name}` in a path is a field of
//! `Params`; the rest of `Params` is the query string. A body that is not
//! JSON - a piece of an upload - is a `Blob`, and an answer that is not JSON
//! - a download, a screenshot - is an endpoint with `RAW_RESPONSE`.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::files::{FileBegun, FilesListing};
use crate::policy::{
    EffectiveEntry, PolicyDoc, PolicyInfo, PolicyMoved, PolicyRemoved, PolicySaved,
};
use crate::{
    Applied, AudioStatus, AudioTested, CalendarCheck, CameraList, CertInfo, CertsAdded, Claimed,
    Command, Concurrency, Connector, Done, EvalResult, HotspotCredentials, ImageUpload, JobPage,
    JobStarted, KeyInfo, LogPage, Net, NetChange, NetProfile, NetProfileDetail, NodeInfo, OnError,
    PrintJob, PrintQueued, PrinterInfo, PrinterList, PrinterSpec, ProxyStatus, ProxyTested,
    Received, RestartTarget, ScheduleInfo, ScheduleSpec, ScreenPower, ScriptInfo, ScriptSpec,
    Secret, Settings, SshAccess, SshKeyInfo, SshKeyRevoked, Storage, Ticket, TimeStatus,
    TokenCreated, TokenInfo, UpdateBegun, UpdateStatus, Verify, WebSession, WelcomeInfo,
    WifiNetwork, WifiSecurity, WifiStatus,
};

/// The API's version, in every path. A change a client of this version
/// would misread gets a new one.
pub const VERSION: &str = "v1";

/// Where a raw download says how large the whole file is, and its mtime.
pub const HEADER_SIZE: &str = "x-tessaro-size";
pub const HEADER_MTIME: &str = "x-tessaro-mtime";

/// How old a camera snapshot's frame is, milliseconds.
pub const HEADER_FRAME_AGE: &str = "x-tessaro-frame-age";

/// A browser request someone made, not a background refresh: only these
/// keep a browser session alive (docs/webconfig.md).
pub const HEADER_ACTIVITY: &str = "x-tessaro-activity";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

/// What the agent does for one request.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Run the command and answer with its result.
    Run(Command),
    /// Start the command as a job (`Command::is_job`) and answer with a
    /// `JobStarted`.
    Start(Command),
    /// What a job did from step `after` on: a `JobPage`.
    Poll { job: String, after: u64 },
    /// Stop a job.
    Cancel { job: String },
    /// Browser sessions, which the server keeps itself.
    Web(Web),
}

/// What a browser asks of its session (docs/webconfig.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Web {
    /// Who the request is: a `WebSession`.
    Session,
    /// Trade a token for a session cookie.
    SignIn { token: String },
    /// End this browser's session.
    SignOut,
    /// A one-time ticket for this request's token.
    Ticket,
    /// Trade a ticket for a session cookie.
    Redeem { ticket: String },
}

/// A request body that is not JSON: the bytes of an upload, as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Blob(pub Vec<u8>);

impl JsonSchema for Blob {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Blob".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({ "type": "string", "format": "binary" })
    }
}

/// An answer that is not JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw {
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// What an endpoint answers, once the command has run.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Json(Value),
    Raw(Raw),
}

/// No query parameters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

pub trait Endpoint {
    /// The type's own name, `Status`: with the group, the OpenAPI
    /// operation id.
    const NAME: &'static str;
    const METHOD: Method;
    const PATH: &'static str;
    /// What it does, from its doc comment: the first line is the summary.
    const DOC: &'static str;
    /// Answered without a token on a claimed device.
    const PUBLIC: bool = false;
    /// The body is a `Blob`, not JSON.
    const RAW_BODY: bool = false;
    /// The answer is `respond`'s `Raw`, not JSON; this is its content type.
    const RAW_RESPONSE: Option<&'static str> = None;
    /// The headers a raw answer carries, with what each says.
    const RAW_HEADERS: &'static [(&'static str, &'static str)] = &[];

    type Params: Serialize + DeserializeOwned + JsonSchema;
    type Body: Serialize + DeserializeOwned + JsonSchema;
    type Response: Serialize + DeserializeOwned + JsonSchema;

    fn action(params: Self::Params, body: Self::Body) -> Action;

    /// The body of a `RAW_BODY` endpoint, from its bytes.
    fn blob(_bytes: Vec<u8>) -> Option<Self::Body> {
        None
    }

    /// The command's result as the answer. Only the raw endpoints turn it
    /// into something other than the JSON itself.
    fn respond(result: Value) -> Result<Answer, String> {
        Ok(Answer::Json(result))
    }
}

/// One endpoint with its types erased, for the agent's router and the
/// OpenAPI document.
#[derive(Clone, Copy)]
pub struct Route {
    pub name: &'static str,
    pub method: Method,
    pub path: &'static str,
    pub doc: &'static str,
    pub public: bool,
    pub raw_body: bool,
    pub raw_response: Option<&'static str>,
    pub raw_headers: &'static [(&'static str, &'static str)],
    /// The path's `{name}`s and the query's pairs, and the body, into what
    /// to do.
    pub decode: Decode,
    pub respond: fn(Value) -> Result<Answer, String>,
    pub schemas: fn(&mut SchemaGenerator) -> Schemas,
    /// A `Body` of `()`: nothing is sent.
    pub bodiless: bool,
}

pub type Decode = fn(&[(String, String)], &[u8]) -> Result<Action, String>;

pub struct Schemas {
    pub params: Schema,
    pub body: Schema,
    pub response: Schema,
}

impl Route {
    pub fn of<E: Endpoint>() -> Route {
        Route {
            name: E::NAME,
            method: E::METHOD,
            path: E::PATH,
            doc: E::DOC,
            public: E::PUBLIC,
            raw_body: E::RAW_BODY,
            raw_response: E::RAW_RESPONSE,
            raw_headers: E::RAW_HEADERS,
            decode: decode::<E>,
            respond: E::respond,
            schemas: |generator| Schemas {
                params: generator.subschema_for::<E::Params>(),
                body: generator.subschema_for::<E::Body>(),
                response: generator.subschema_for::<E::Response>(),
            },
            bodiless: E::Body::schema_name() == "null",
        }
    }

    /// The path's `{name}` segments, if `path` is this route's.
    pub fn matches(&self, path: &str) -> Option<Vec<(String, String)>> {
        let mut captured = Vec::new();
        let mut pattern = self.path.split('/');
        let mut given = path.split('/');
        loop {
            match (pattern.next(), given.next()) {
                (None, None) => return Some(captured),
                (Some(expected), Some(segment)) => {
                    if let Some(name) = expected.strip_prefix('{').and_then(|n| n.strip_suffix('}'))
                    {
                        if segment.is_empty() {
                            return None;
                        }
                        captured.push((name.to_string(), percent_decode(segment)?));
                    } else if expected != segment {
                        return None;
                    }
                }
                _ => return None,
            }
        }
    }

    /// A literal path beats one with a `{name}` in the same place.
    pub fn is_literal(&self) -> bool {
        !self.path.contains('{')
    }

    /// The `tessaro-ctl` group it belongs to: the segment after `/api/v1/`.
    pub fn tag(&self) -> &'static str {
        self.path
            .trim_start_matches("/api/")
            .trim_start_matches(VERSION)
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("")
    }
}

fn decode<E: Endpoint>(pairs: &[(String, String)], body: &[u8]) -> Result<Action, String> {
    let query = serde_urlencoded::to_string(pairs).map_err(|err| err.to_string())?;
    let params: E::Params =
        serde_urlencoded::from_str(&query).map_err(|err| format!("the query: {err}"))?;
    let body: E::Body = if E::RAW_BODY {
        E::blob(body.to_vec()).ok_or("this endpoint takes no raw body")?
    } else if body.is_empty() {
        serde_json::from_value(Value::Null).map_err(|_| "a JSON body is needed".to_string())?
    } else {
        serde_json::from_slice(body).map_err(|err| format!("the body: {err}"))?
    };
    Ok(E::action(params, body))
}

/// The path and query a client sends for `params`: every `{name}` filled
/// in, every other field in the query string.
pub fn target<E: Endpoint>(params: &E::Params) -> Result<String, String> {
    let encoded = serde_urlencoded::to_string(params).map_err(|err| err.to_string())?;
    let mut pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(&encoded).map_err(|err| err.to_string())?;
    let mut path = String::new();
    for (index, segment) in E::PATH.split('/').enumerate() {
        if index > 0 {
            path.push('/');
        }
        match segment.strip_prefix('{').and_then(|n| n.strip_suffix('}')) {
            Some(name) => {
                let at = pairs
                    .iter()
                    .position(|(key, _)| key == name)
                    .ok_or_else(|| format!("{} needs {name}", E::PATH))?;
                let (_, value) = pairs.remove(at);
                if value.is_empty() {
                    return Err(format!("{name} must not be empty"));
                }
                path.push_str(&percent_encode(&value));
            }
            None => path.push_str(segment),
        }
    }
    if !pairs.is_empty() {
        path.push('?');
        path.push_str(&serde_urlencoded::to_string(&pairs).map_err(|err| err.to_string())?);
    }
    Ok(path)
}

fn percent_encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = segment.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Every refusal, whatever the endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    pub error: String,
    pub code: ErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// 400: the query or the body is not what the endpoint takes.
    BadRequest,
    /// 401: the device is claimed and this needs a token.
    TokenRequired,
    /// 401: the token is not one of the device's.
    InvalidToken,
    /// 404: no such endpoint, or no such job.
    NotFound,
    /// 413: the body is too large.
    TooLarge,
    /// 422: the device refused: `error` says why.
    Refused,
    /// 429: too many invalid tokens from this address; wait a minute.
    RateLimited,
    /// 500: the device failed to answer.
    Internal,
    /// 403: a browser request from another site, or a browser write with
    /// the wrong content type.
    CrossOrigin,
}

impl ErrorCode {
    pub fn status(self) -> u16 {
        match self {
            ErrorCode::BadRequest => 400,
            ErrorCode::CrossOrigin => 403,
            ErrorCode::TokenRequired | ErrorCode::InvalidToken => 401,
            ErrorCode::NotFound => 404,
            ErrorCode::TooLarge => 413,
            ErrorCode::Refused => 422,
            ErrorCode::RateLimited => 429,
            ErrorCode::Internal => 500,
        }
    }
}

fn yes() -> bool {
    true
}

// --- the bodies and queries ------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigQuery {
    /// One key; all of them without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetConfig {
    pub values: BTreeMap<String, String>,
    /// Refuse unless the settings are still at this revision.
    #[serde(default)]
    pub if_revision: Option<u64>,
    /// Restart what reads the keys. Without it the change waits for the next
    /// restart.
    #[serde(default = "yes")]
    pub apply: bool,
    /// What the device checks before it keeps a change to network keys.
    #[serde(default)]
    pub verify: Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnsetConfig {
    pub keys: Vec<String>,
    #[serde(default)]
    pub if_revision: Option<u64>,
    #[serde(default = "yes")]
    pub apply: bool,
    #[serde(default)]
    pub verify: Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LogsQuery {
    /// Only this unit, or a pattern of them (`tessaro-schedule-*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The last this many entries, 100 by default. Ignored with `cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Only what came after this, from the last page's `cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestartBody {
    pub what: RestartTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NavigateBody {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyRef {
    /// The browser policy's name.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyBody {
    /// The document as typed: one JSON object of Chromium policies, `//`
    /// and `/* */` comments and trailing commas allowed. At most
    /// `POLICY_TEXT_MAX` bytes.
    pub text: String,
    /// Refuse unless the stored document is still at this revision; `""`
    /// refuses unless there is none yet. Unconditional without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_revision: Option<String>,
    /// Where it goes in the priority order, from 1, the highest. A new one
    /// goes to the bottom without it; a stored one stays where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyPositionBody {
    /// From 1, the highest priority; past the last, the last.
    pub position: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvalBody {
    /// At most `EVAL_MAX` bytes of JavaScript.
    pub code: String,
    /// `EVAL_TIMEOUT_MS` by default, `EVAL_TIMEOUT_MAX_MS` at most.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Wait for a returned Promise and answer with what it settles to.
    #[serde(default = "yes")]
    pub await_promise: bool,
    /// Run as if the user had just touched the page.
    #[serde(default)]
    pub user_gesture: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyboardBody {
    pub show: bool,
    /// The field to focus when showing; the one with the focus without it.
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenPowerBody {
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NameBody {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenRef {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignInBody {
    /// One of the device's tokens, `tsr_...`.
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RedeemBody {
    /// From `access/ticket`, once.
    pub ticket: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PasswordBody {
    /// Generated, and answered once, without it.
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshKeyBody {
    /// One `.pub` line.
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshKeyQuery {
    /// Its `SHA256:` fingerprint, a unique prefix of it, or its exact
    /// comment.
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OffsetQuery {
    /// Where this piece starts: what the device said it has.
    pub offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitBody {
    /// Re-create `/data` too: settings, the claim, the browser profile.
    #[serde(default)]
    pub wipe_data: bool,
    #[serde(default = "yes")]
    pub reboot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeedtestBody {
    /// Largest payload, one of `SPEEDTEST_SIZES`.
    #[serde(default)]
    pub max_size: Option<u64>,
    /// Samples per payload size.
    #[serde(default)]
    pub tests: Option<u32>,
    /// Go around network.proxy.url, straight to Cloudflare.
    #[serde(default)]
    pub direct: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileQuery {
    /// Its name (`connection.id`) or uuid.
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WifiScanQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// Scan first. Without it, what the device last saw.
    #[serde(default = "yes")]
    pub rescan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WifiJoinBody {
    pub ssid: String,
    /// Without it the stored one: rejoining the same network.
    #[serde(default)]
    pub psk: Option<Secret>,
    /// Found by scanning when not given; a hidden network needs it.
    #[serde(default)]
    pub security: Option<WifiSecurity>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub verify: Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PingBody {
    pub host: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub interface: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CertBody {
    /// One or more PEM certificates.
    pub pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CertQuery {
    /// Its SHA-256 fingerprint, a unique prefix of it, or its exact subject.
    pub cert: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrowBody {
    /// Only say what would change.
    #[serde(default)]
    pub check: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioTestBody {
    /// Record from the input instead of playing a tone.
    #[serde(default)]
    pub input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimeSetBody {
    /// Microseconds since the epoch, UTC.
    #[serde(default)]
    pub usec: Option<u64>,
    /// `YYYY-MM-DD HH:MM[:SS]`, in the device's timezone.
    #[serde(default)]
    pub local: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptRef {
    /// Its id or its name.
    pub script: String,
}

/// What is given replaces what the script has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptChange {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub on_error: Option<OnError>,
    /// `0` removes the timeout.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    #[serde(default)]
    pub concurrency: Option<Concurrency>,
    #[serde(default)]
    pub bridge: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleRef {
    /// Its id or its name.
    pub schedule: String,
}

/// What is given replaces what the schedule has; a given `calendar`
/// replaces the whole list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleChange {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub calendar: Option<Vec<String>>,
    /// The script it runs, by id or name.
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterRef {
    pub printer: String,
}

/// A camera by its own node, as `camera list` names it: `video0`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraRef {
    pub device: String,
}

/// Where a document goes and how. Without a body, `path` names a file in
/// the store to print instead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrintQuery {
    pub printer: String,
    /// A file in the store, from its root, for a document sent with no
    /// body: one larger than `PRINT_DATA_MAX`.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub copies: Option<u32>,
    /// The paper, as the printer names it; the printer's own `media` when
    /// not given.
    #[serde(default)]
    pub media: Option<String>,
    /// What CUPS calls the job.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrintJobsQuery {
    /// Only this printer's.
    #[serde(default)]
    pub printer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrintJobRef {
    pub job: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CalendarBody {
    pub calendar: Vec<String>,
    /// How many run times to answer with.
    #[serde(default)]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesQuery {
    /// From the store's root; the root itself when empty.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadBody {
    pub path: String,
    pub size: u64,
    /// Seconds since the epoch, stamped on the file once it is complete.
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadQuery {
    pub path: String,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadQuery {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    /// At most `UPDATE_CHUNK`.
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathBody {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveBody {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteBody {
    pub paths: Vec<String>,
    /// Directories with everything in them.
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobRef {
    pub job: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobQuery {
    pub job: String,
    /// The first step to answer with: the last page's `next`.
    #[serde(default)]
    pub after: u64,
}

// --- the endpoints ---------------------------------------------------------

macro_rules! endpoints {
    ($(
        $(#[doc = $doc:literal])*
        $name:ident: $method:ident $path:literal ($params:ty, $body:ty) -> $response:ty
            $({ $($extra:tt)* })?
            = |$p:pat_param, $b:pat_param| $action:expr;
    )*) => {
        $(
            $(#[doc = $doc])*
            pub struct $name;

            impl Endpoint for $name {
                const NAME: &'static str = stringify!($name);
                const METHOD: Method = Method::$method;
                const PATH: &'static str = $path;
                const DOC: &'static str = concat!($($doc, "\n"),*);
                type Params = $params;
                type Body = $body;
                type Response = $response;
                $($($extra)*)?

                fn action($p: $params, $b: $body) -> Action {
                    $action
                }
            }
        )*

        /// Every endpoint.
        pub fn routes() -> Vec<Route> {
            vec![$(Route::of::<$name>()),*]
        }
    };
}

/// A raw answer from a command's JSON: its base64 `data` as the body.
fn raw_data(result: &Value, content_type: &'static str) -> Result<Raw, String> {
    let data = result["data"]
        .as_str()
        .ok_or("the answer has no data")?
        .as_bytes();
    let body = data_encoding::BASE64
        .decode(data)
        .map_err(|_| "the answer's data is not base64".to_string())?;
    Ok(Raw {
        content_type,
        headers: Vec::new(),
        body,
    })
}

pub mod device {
    use super::*;

    endpoints! {
        /// The device's identity: node id, name, version, machine and TLS
        /// fingerprint, and whether it is claimed.
        Id: Get "/api/v1/device/id" (Empty, ()) -> NodeInfo
            { const PUBLIC: bool = true; }
            = |_, _| Action::Run(Command::Id);

        /// One round trip: the agent is answering.
        Ping: Get "/api/v1/device/ping" (Empty, ()) -> Done
            { const PUBLIC: bool = true; }
            = |_, _| Action::Run(Command::Ping);

        /// What the device is doing: the page on screen, units, a change on
        /// probation, audio, storage, the clock.
        Status: Get "/api/v1/device/status" (Empty, ()) -> crate::Status
            = |_, _| Action::Run(Command::Status);

        /// What the welcome page shows: the setup QR code and address,
        /// whether the device is online. Asking keeps the online check
        /// running for a while.
        Welcome: Get "/api/v1/device/welcome" (Empty, ()) -> WelcomeInfo
            = |_, _| Action::Run(Command::Welcome);

        /// A page of the journal. Following it is asking again with the
        /// page's `cursor`.
        Logs: Get "/api/v1/device/logs" (LogsQuery, ()) -> LogPage
            = |query, _| Action::Run(Command::Logs {
                unit: query.unit,
                lines: query.lines,
                cursor: query.cursor,
            });

        /// Restart the browser, the compositor (and with it the browser and
        /// the agent), or the agent.
        Restart: Post "/api/v1/device/restart" (Empty, RestartBody) -> Done
            = |_, body| Action::Run(Command::Restart { what: body.what });

        /// Reboot, once the answer is out.
        Reboot: Post "/api/v1/device/reboot" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::Reboot);

        /// Wipe `/data` and start over as a new, unclaimed device.
        FactoryReset: Post "/api/v1/device/factory-reset" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::FactoryReset);
    }
}

pub mod access {
    use super::*;

    endpoints! {
        /// Take an unclaimed device: the first claim wins, and answers with
        /// the token, the root password and the hotspot's password, once.
        Claim: Post "/api/v1/access/claim" (Empty, NameBody) -> Claimed
            { const PUBLIC: bool = true; }
            = |_, body| Action::Run(Command::Claim { name: body.name });

        /// Remove every owner: every token, the root password, the SSH keys.
        Unclaim: Post "/api/v1/access/unclaim" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::Unclaim);

        /// The tokens, never their values.
        Tokens: Get "/api/v1/access/tokens" (Empty, ()) -> Vec<TokenInfo>
            = |_, _| Action::Run(Command::TokenList);

        /// Issue another token, answered once.
        TokenCreate: Post "/api/v1/access/tokens" (Empty, NameBody) -> TokenCreated
            = |_, body| Action::Run(Command::TokenCreate { name: body.name });

        /// Revoke a token by its id.
        TokenRevoke: Delete "/api/v1/access/tokens/{id}" (TokenRef, ()) -> Done
            = |token, _| Action::Run(Command::TokenRevoke { id: token.id });

        /// Set the root password, or generate one and answer with it.
        Password: Post "/api/v1/access/password" (Empty, PasswordBody) -> crate::Password
            = |_, body| Action::Run(Command::PasswordSet { password: body.password });

        /// Who this request is: claimed or not, and by which token, if any.
        /// Webconfig asks it first.
        Session: Get "/api/v1/access/session" (Empty, ()) -> WebSession
            { const PUBLIC: bool = true; }
            = |_, _| Action::Web(Web::Session);

        /// Sign a browser in with a token: the answer sets its session
        /// cookie.
        SignIn: Post "/api/v1/access/session" (Empty, SignInBody) -> WebSession
            { const PUBLIC: bool = true; }
            = |_, body| Action::Web(Web::SignIn { token: body.token });

        /// Sign this browser out: its session ends and the cookie is cleared.
        SignOut: Delete "/api/v1/access/session" (Empty, ()) -> Done
            { const PUBLIC: bool = true; }
            = |_, _| Action::Web(Web::SignOut);

        /// A one-time ticket, redeemable for a minute, that signs a browser
        /// in as this request's token.
        TicketCreate: Post "/api/v1/access/ticket" (Empty, ()) -> Ticket
            = |_, _| Action::Web(Web::Ticket);

        /// Sign a browser in with a ticket: the answer sets its session
        /// cookie.
        Redeem: Post "/api/v1/access/ticket/redeem" (Empty, RedeemBody) -> WebSession
            { const PUBLIC: bool = true; }
            = |_, body| Action::Web(Web::Redeem { ticket: body.ticket });
    }
}

pub mod ssh {
    use super::*;

    endpoints! {
        /// Add a public key to root's `authorized_keys`. The answer carries
        /// the device's host keys, so the client can check them.
        Authorize: Post "/api/v1/ssh/keys" (Empty, SshKeyBody) -> SshAccess
            = |_, body| Action::Run(Command::SshAuthorize { key: body.key });

        /// The authorized keys.
        Keys: Get "/api/v1/ssh/keys" (Empty, ()) -> Vec<SshKeyInfo>
            = |_, _| Action::Run(Command::SshKeyList);

        /// Remove one authorized key.
        Revoke: Delete "/api/v1/ssh/keys" (SshKeyQuery, ()) -> SshKeyRevoked
            = |query, _| Action::Run(Command::SshKeyRevoke { key: query.key });
    }
}

pub mod config {
    use super::*;

    endpoints! {
        /// Every setting: what it does, what it takes, its default and its
        /// value here.
        Keys: Get "/api/v1/config/keys" (Empty, ()) -> Vec<KeyInfo>
            = |_, _| Action::Run(Command::Keys);

        /// The settings, or one of them, and their revision.
        Get: Get "/api/v1/config" (ConfigQuery, ()) -> Settings
            = |query, _| Action::Run(Command::Get { key: query.key });

        /// Change settings, restarting exactly what reads them.
        Set: Post "/api/v1/config/set" (Empty, SetConfig) -> Applied
            = |_, body| Action::Run(Command::Set {
                values: body.values,
                if_revision: body.if_revision,
                apply: body.apply,
                verify: body.verify,
            });

        /// Put settings back to the image's defaults.
        Unset: Post "/api/v1/config/unset" (Empty, UnsetConfig) -> Applied
            = |_, body| Action::Run(Command::Unset {
                keys: body.keys,
                if_revision: body.if_revision,
                apply: body.apply,
                verify: body.verify,
            });
    }
}

pub mod screen {
    use super::*;

    endpoints! {
        /// The output modes every connected connector advertises.
        Modes: Get "/api/v1/screen/modes" (Empty, ()) -> Vec<Connector>
            = |_, _| Action::Run(Command::Modes);

        /// What is on screen, as a JPEG.
        Screenshot: Get "/api/v1/screen/screenshot" (Empty, ()) -> super::Blob
            {
                const RAW_RESPONSE: Option<&'static str> = Some("image/jpeg");
                fn respond(result: Value) -> Result<Answer, String> {
                    raw_data(&result, "image/jpeg").map(Answer::Raw)
                }
            }
            = |_, _| Action::Run(Command::Screenshot);

        /// Keep the guarded changes (a resolution, a rotation) that are on
        /// probation.
        Confirm: Post "/api/v1/screen/confirm" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::Confirm);

        /// Whether the display is on.
        Power: Get "/api/v1/screen/power" (Empty, ()) -> ScreenPower
            = |_, _| Action::Run(Command::ScreenPower { on: None });

        /// Switch the display on or off.
        PowerSet: Post "/api/v1/screen/power" (Empty, ScreenPowerBody) -> ScreenPower
            = |_, body| Action::Run(Command::ScreenPower { on: Some(body.on) });

        /// Show or hide the on-screen keyboard.
        Keyboard: Post "/api/v1/screen/keyboard" (Empty, KeyboardBody) -> Done
            = |_, body| Action::Run(Command::Keyboard {
                show: body.show,
                selector: body.selector,
            });
    }
}

pub mod browser {
    use super::*;

    endpoints! {
        /// Show a URL now, until the next restart of the browser.
        Navigate: Post "/api/v1/browser/navigate" (Empty, NavigateBody) -> Done
            = |_, body| Action::Run(Command::Navigate { url: body.url });

        /// Reload the page, past the cache.
        Reload: Post "/api/v1/browser/reload" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::Reload);

        /// Empty the browser's HTTP cache.
        ClearCache: Post "/api/v1/browser/clear-cache" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::ClearCache);

        /// Run JavaScript in the page on screen.
        Eval: Post "/api/v1/browser/eval" (Empty, EvalBody) -> EvalResult
            = |_, body| Action::Run(Command::Eval {
                code: body.code,
                timeout_ms: body.timeout_ms,
                await_promise: body.await_promise,
                user_gesture: body.user_gesture,
            });

        /// The stored browser policies, without their text.
        Policies: Get "/api/v1/browser/policies" (Empty, ()) -> Vec<PolicyInfo>
            = |_, _| Action::Run(Command::BrowserPolicyList);

        /// One stored browser policy, its text as typed.
        Policy: Get "/api/v1/browser/policies/{name}" (PolicyRef, ()) -> PolicyDoc
            = |target, _| Action::Run(Command::BrowserPolicyGet { name: target.name });

        /// Store a browser policy, replacing one of that name. The browser
        /// restarts when the merged policy changes.
        PolicySet: Put "/api/v1/browser/policies/{name}" (PolicyRef, PolicyBody) -> PolicySaved
            = |target, body| Action::Run(Command::BrowserPolicySet {
                name: target.name,
                text: body.text,
                if_revision: body.if_revision,
                position: body.position,
            });

        /// Move a browser policy in the priority order, the others shifting
        /// to make room. The browser restarts when the merged policy changes.
        PolicyMove: Put "/api/v1/browser/policies/{name}/position" (PolicyRef, PolicyPositionBody) -> PolicyMoved
            = |target, body| Action::Run(Command::BrowserPolicyMove {
                name: target.name,
                position: body.position,
            });

        /// Remove a browser policy. The browser restarts when the merged
        /// policy changes.
        PolicyRemove: Delete "/api/v1/browser/policies/{name}" (PolicyRef, ()) -> PolicyRemoved
            = |target, _| Action::Run(Command::BrowserPolicyRemove { name: target.name });

        /// The merged policy Chromium reads, each entry with where it
        /// comes from.
        Effective: Get "/api/v1/browser/policy" (Empty, ()) -> Vec<EffectiveEntry>
            = |_, _| Action::Run(Command::BrowserPolicyEffective);
    }
}

pub mod network {
    use super::*;

    endpoints! {
        /// Addresses, the default route, DNS and every interface.
        Show: Get "/api/v1/network" (Empty, ()) -> Net
            = |_, _| Action::Run(Command::Net);

        /// NetworkManager's profiles, managed and hand-made.
        Profiles: Get "/api/v1/network/profiles" (Empty, ()) -> Vec<NetProfile>
            = |_, _| Action::Run(Command::NetProfiles);

        /// One profile's addressing, DNS and WiFi settings, never its
        /// secrets.
        Profile: Get "/api/v1/network/profile" (ProfileQuery, ()) -> NetProfileDetail
            = |query, _| Action::Run(Command::NetShow { profile: query.profile });

        /// What the last network change did, for a client whose connection
        /// went with it.
        Last: Get "/api/v1/network/last" (Empty, ()) -> Option<NetChange>
            = |_, _| Action::Run(Command::NetLast);

        /// The WiFi radio and what each WiFi device is connected to.
        Wifi: Get "/api/v1/network/wifi" (Empty, ()) -> WifiStatus
            = |_, _| Action::Run(Command::Wifi);

        /// The WiFi networks in range.
        WifiScan: Get "/api/v1/network/wifi/scan" (WifiScanQuery, ()) -> Vec<WifiNetwork>
            = |query, _| Action::Run(Command::WifiScan {
                interface: query.interface,
                rescan: query.rescan,
            });

        /// Join a WiFi network as a client; the hotspot goes down. Rolled
        /// back if the device cannot reach it.
        WifiJoin: Post "/api/v1/network/wifi/join" (Empty, WifiJoinBody) -> Applied
            = |_, body| Action::Run(Command::WifiJoin {
                ssid: body.ssid,
                psk: body.psk,
                security: body.security,
                hidden: body.hidden,
                verify: body.verify,
            });

        /// A new random hotspot password, answered once. Claimed devices
        /// only.
        HotspotPassword: Post "/api/v1/network/hotspot/password" (Empty, ()) -> HotspotCredentials
            = |_, _| Action::Run(Command::HotspotPassword);

        /// Ping a host from the device, as a job of `PingEvent`s.
        Ping: Post "/api/v1/network/ping" (Empty, PingBody) -> JobStarted
            = |_, body| Action::Start(Command::NetPing {
                host: body.host,
                count: body.count,
                interval_ms: body.interval_ms,
                timeout_ms: body.timeout_ms,
                interface: body.interface,
            });

        /// Measure the device's internet connection against
        /// speed.cloudflare.com, as a job of `SpeedtestEvent`s.
        Speedtest: Post "/api/v1/network/speedtest" (Empty, SpeedtestBody) -> JobStarted
            = |_, body| Action::Start(Command::Speedtest {
                max_size: body.max_size,
                tests: body.tests,
                direct: body.direct,
            });

        /// The proxy: its URL masked, the bypass list and the local proxy's
        /// unit.
        Proxy: Get "/api/v1/network/proxy" (Empty, ()) -> ProxyStatus
            = |_, _| Action::Run(Command::ProxyStatus);

        /// Fetch Cloudflare's trace through the proxy.
        ProxyTest: Post "/api/v1/network/proxy/test" (Empty, ()) -> ProxyTested
            = |_, _| Action::Run(Command::ProxyTest);

        /// The extra certificate authorities the device trusts.
        Certs: Get "/api/v1/network/certs" (Empty, ()) -> Vec<CertInfo>
            = |_, _| Action::Run(Command::NetCertList);

        /// Trust more certificate authorities, in Chromium and the agent.
        CertAdd: Post "/api/v1/network/certs" (Empty, CertBody) -> CertsAdded
            = |_, body| Action::Run(Command::NetCertAdd { pem: body.pem });

        /// Stop trusting one of them.
        CertRevoke: Delete "/api/v1/network/certs" (CertQuery, ()) -> CertInfo
            = |query, _| Action::Run(Command::NetCertRevoke { cert: query.cert });
    }
}

pub mod storage {
    use super::*;

    endpoints! {
        /// The disk the device runs from, its partitions and how full each
        /// filesystem is.
        Show: Get "/api/v1/storage" (Empty, ()) -> Storage
            = |_, _| Action::Run(Command::Storage);

        /// Grow `/data` over the unallocated space, as a job of
        /// `StorageGrowEvent`s: the plan, then each step.
        Grow: Post "/api/v1/storage/grow" (Empty, GrowBody) -> JobStarted
            = |_, body| Action::Start(Command::StorageGrow { check: body.check });
    }
}

pub mod camera {
    use super::*;

    endpoints! {
        /// Every USB camera, what its mirror captures, and the virtual
        /// camera everything else reads it through.
        List: Get "/api/v1/camera" (Empty, ()) -> CameraList
            = |_, _| Action::Run(Command::CameraList);

        /// The newest frame of a camera, as a JPEG, taken by its mirror
        /// without a mirror slot of its own. Its age is in the
        /// `x-tessaro-frame-age` header. The mirror hands frames out only
        /// while they are asked for, so poll it for a preview.
        Snapshot: Get "/api/v1/camera/{device}/snapshot" (CameraRef, ()) -> super::Blob
            {
                const RAW_RESPONSE: Option<&'static str> = Some("image/jpeg");
                const RAW_HEADERS: &'static [(&'static str, &'static str)] = &[
                    (HEADER_FRAME_AGE, "How old the frame is, milliseconds."),
                ];
                fn respond(result: Value) -> Result<Answer, String> {
                    let mut raw = raw_data(&result, "image/jpeg")?;
                    raw.headers.push((HEADER_FRAME_AGE, result["age_ms"].to_string()));
                    Ok(Answer::Raw(raw))
                }
            }
            = |camera, _| Action::Run(Command::CameraSnapshot { device: camera.device });
    }
}

pub mod audio {
    use super::*;

    endpoints! {
        /// Every output and input, and which the audio.* settings put in
        /// use.
        Show: Get "/api/v1/audio" (Empty, ()) -> AudioStatus
            = |_, _| Action::Run(Command::AudioStatus);

        /// A tone on the output in use, or a few seconds recorded from the
        /// input and its level.
        Test: Post "/api/v1/audio/test" (Empty, AudioTestBody) -> AudioTested
            = |_, body| Action::Run(Command::AudioTest { input: body.input });
    }
}

pub mod time {
    use super::*;

    endpoints! {
        /// The clock as systemd reports it: timezone, sync, the last NTP
        /// exchange.
        Show: Get "/api/v1/time" (Empty, ()) -> TimeStatus
            = |_, _| Action::Run(Command::TimeStatus);

        /// Every timezone the device knows.
        Zones: Get "/api/v1/time/zones" (Empty, ()) -> Vec<String>
            = |_, _| Action::Run(Command::TimeZones);

        /// Ask the NTP servers again now.
        Sync: Post "/api/v1/time/sync" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::TimeSync);

        /// Set the clock by hand, with time.ntp.enable off.
        Set: Post "/api/v1/time/set" (Empty, TimeSetBody) -> Done
            = |_, body| Action::Run(Command::TimeSet {
                usec: body.usec,
                local: body.local,
            });
    }
}

pub mod script {
    use super::*;

    endpoints! {
        /// Every script and its recent runs.
        List: Get "/api/v1/scripts" (Empty, ()) -> Vec<ScriptInfo>
            = |_, _| Action::Run(Command::ScriptList);

        /// Add a script.
        Create: Post "/api/v1/scripts" (Empty, ScriptSpec) -> ScriptInfo
            = |_, spec| Action::Run(Command::ScriptCreate { spec });

        /// Change what is given of one script.
        Change: Patch "/api/v1/scripts/{script}" (ScriptRef, ScriptChange) -> ScriptInfo
            = |target, change| Action::Run(Command::ScriptSet {
                script: target.script,
                name: change.name,
                description: change.description,
                body: change.body,
                on_error: change.on_error,
                timeout_s: change.timeout_s,
                concurrency: change.concurrency,
                bridge: change.bridge,
            });

        /// Remove a script no schedule runs, and its units.
        Remove: Delete "/api/v1/scripts/{script}" (ScriptRef, ()) -> Done
            = |target, _| Action::Run(Command::ScriptRemove { script: target.script });

        /// Run it now, as a job of `ScriptEvent`s: its output, then how it
        /// ended.
        Run: Post "/api/v1/scripts/{script}/run" (ScriptRef, ()) -> JobStarted
            = |target, _| Action::Start(Command::ScriptRun { script: target.script });
    }
}

pub mod schedule {
    use super::*;

    endpoints! {
        /// Every schedule, when it runs next and how its last run ended.
        List: Get "/api/v1/schedules" (Empty, ()) -> Vec<ScheduleInfo>
            = |_, _| Action::Run(Command::ScheduleList);

        /// Add a schedule; its calendar is checked first.
        Create: Post "/api/v1/schedules" (Empty, ScheduleSpec) -> ScheduleInfo
            = |_, spec| Action::Run(Command::ScheduleCreate { spec });

        /// Change what is given of one schedule.
        Change: Patch "/api/v1/schedules/{schedule}" (ScheduleRef, ScheduleChange) -> ScheduleInfo
            = |target, change| Action::Run(Command::ScheduleSet {
                schedule: target.schedule,
                name: change.name,
                calendar: change.calendar,
                script: change.script,
                enabled: change.enabled,
            });

        /// Remove a schedule and its timer.
        Remove: Delete "/api/v1/schedules/{schedule}" (ScheduleRef, ()) -> Done
            = |target, _| Action::Run(Command::ScheduleRemove { schedule: target.schedule });

        /// Check `OnCalendar` expressions, saving nothing.
        Check: Post "/api/v1/schedules/check" (Empty, CalendarBody) -> CalendarCheck
            = |_, body| Action::Run(Command::ScheduleCheck {
                calendar: body.calendar,
                count: body.count,
            });
    }
}

pub mod printer {
    use super::*;

    endpoints! {
        /// Every printer, which one is the default, and whether pages may
        /// print (printer.enable).
        List: Get "/api/v1/printers" (Empty, ()) -> PrinterList
            = |_, _| Action::Run(Command::PrinterList);

        /// Printers found on USB and the network, as a job of
        /// `PrinterFound`s.
        Discover: Post "/api/v1/printers/discover" (Empty, ()) -> JobStarted
            = |_, _| Action::Start(Command::PrinterDiscover);

        /// Add a printer. A driverless one must answer while it is set up.
        Create: Post "/api/v1/printers" (Empty, PrinterSpec) -> PrinterInfo
            = |_, spec| Action::Run(Command::PrinterCreate { spec });

        /// One printer in full, its supplies included.
        Show: Get "/api/v1/printers/{printer}" (PrinterRef, ()) -> PrinterInfo
            = |target, _| Action::Run(Command::PrinterShow { printer: target.printer });

        /// Remove a printer and the jobs it still holds.
        Remove: Delete "/api/v1/printers/{printer}" (PrinterRef, ()) -> Done
            = |target, _| Action::Run(Command::PrinterRemove { printer: target.printer });

        /// Make it the printer `window.print()` uses.
        SetDefault: Post "/api/v1/printers/{printer}/default" (PrinterRef, ()) -> Done
            = |target, _| Action::Run(Command::PrinterDefault { printer: target.printer });

        /// Print a test page.
        Test: Post "/api/v1/printers/{printer}/test" (PrinterRef, ()) -> PrintQueued
            = |target, _| Action::Run(Command::PrinterTest { printer: target.printer });

        /// Print the body, at most `PRINT_DATA_MAX` bytes: a PDF for a
        /// driverless printer, the printer's own bytes for a raw one. With
        /// no body, the store's file at `path`.
        Print: Post "/api/v1/printers/{printer}/print" (PrintQuery, Blob) -> PrintQueued
            {
                const RAW_BODY: bool = true;
                fn blob(bytes: Vec<u8>) -> Option<Blob> {
                    Some(Blob(bytes))
                }
            }
            = |query, data| Action::Run(Command::PrinterPrint {
                printer: query.printer,
                data: (!data.0.is_empty()).then(|| data_encoding::BASE64.encode(&data.0)),
                path: query.path,
                copies: query.copies,
                media: query.media,
                title: query.title,
            });

        /// The jobs not printed yet.
        Jobs: Get "/api/v1/printers/jobs" (PrintJobsQuery, ()) -> Vec<PrintJob>
            = |query, _| Action::Run(Command::PrinterJobs { printer: query.printer });

        /// Cancel a job.
        Cancel: Delete "/api/v1/printers/jobs/{job}" (PrintJobRef, ()) -> Done
            = |target, _| Action::Run(Command::PrinterCancel { job: target.job });
    }
}

pub mod update {
    use super::*;

    endpoints! {
        /// Where an update stands: the upload, its checks, the last one
        /// applied.
        Status: Get "/api/v1/update" (Empty, ()) -> UpdateStatus
            = |_, _| Action::Run(Command::UpdateStatus);

        /// Start, or resume, uploading an image. The answer says where to
        /// resume.
        Begin: Post "/api/v1/update/begin" (Empty, ImageUpload) -> UpdateBegun
            = |_, upload| Action::Run(Command::UpdateBegin(upload));

        /// The next piece of the image, at most `UPDATE_CHUNK` bytes, as
        /// the body.
        Image: Put "/api/v1/update/image" (OffsetQuery, Blob) -> Received
            {
                const RAW_BODY: bool = true;
                fn blob(bytes: Vec<u8>) -> Option<Blob> {
                    Some(Blob(bytes))
                }
            }
            = |query, data| Action::Run(Command::UpdateChunk {
                offset: query.offset,
                data: data_encoding::BASE64.encode(&data.0),
            });

        /// Apply the prepared update at the next boot.
        Commit: Post "/api/v1/update/commit" (Empty, CommitBody) -> Done
            = |_, body| Action::Run(Command::UpdateCommit {
                wipe_data: body.wipe_data,
                reboot: body.reboot,
            });

        /// Drop the upload or the prepared update.
        Cancel: Post "/api/v1/update/cancel" (Empty, ()) -> Done
            = |_, _| Action::Run(Command::UpdateCancel);
    }
}

pub mod files {
    use super::*;

    endpoints! {
        /// What is stored in a path, like `ls`: a directory's entries, or
        /// everything under it.
        List: Get "/api/v1/files" (FilesQuery, ()) -> FilesListing
            = |query, _| Action::Run(Command::FilesList {
                path: query.path,
                recursive: query.recursive,
            });

        /// Start, or resume, storing one file. The answer says where to
        /// resume.
        Begin: Post "/api/v1/files/upload" (Empty, UploadBody) -> FileBegun
            = |_, body| Action::Run(Command::FilesBegin {
                path: body.path,
                size: body.size,
                mtime: body.mtime,
            });

        /// The next piece of the file begun for `path`, at most
        /// `UPDATE_CHUNK` bytes, as the body.
        Upload: Put "/api/v1/files/upload" (UploadQuery, Blob) -> Received
            {
                const RAW_BODY: bool = true;
                fn blob(bytes: Vec<u8>) -> Option<Blob> {
                    Some(Blob(bytes))
                }
            }
            = |query, data| Action::Run(Command::FilesChunk {
                path: query.path,
                offset: query.offset,
                data: data_encoding::BASE64.encode(&data.0),
            });

        /// Up to `len` bytes of a stored file from `offset`, as the body.
        /// The whole file's size and mtime are in the `x-tessaro-size` and
        /// `x-tessaro-mtime` headers; an empty body is the end of it.
        Read: Get "/api/v1/files/content" (ReadQuery, ()) -> Blob
            {
                const RAW_RESPONSE: Option<&'static str> = Some("application/octet-stream");
                const RAW_HEADERS: &'static [(&'static str, &'static str)] = &[
                    (HEADER_SIZE, "The whole file's size, bytes."),
                    (HEADER_MTIME, "The file's mtime, seconds since the epoch."),
                ];
                fn respond(result: Value) -> Result<Answer, String> {
                    let mut raw = raw_data(&result, "application/octet-stream")?;
                    raw.headers.push((HEADER_SIZE, result["size"].to_string()));
                    raw.headers.push((HEADER_MTIME, result["mtime"].to_string()));
                    Ok(Answer::Raw(raw))
                }
            }
            = |query, _| Action::Run(Command::FilesRead {
                path: query.path,
                offset: query.offset,
                len: query.len,
            });

        /// Make a directory, and any missing above it.
        Mkdir: Post "/api/v1/files/mkdir" (Empty, PathBody) -> Done
            = |_, body| Action::Run(Command::FilesMkdir { path: body.path });

        /// Move or rename, like `mv`.
        Move: Post "/api/v1/files/move" (Empty, MoveBody) -> Done
            = |_, body| Action::Run(Command::FilesMove { from: body.from, to: body.to });

        /// Remove files, or directories with everything in them.
        Delete: Post "/api/v1/files/delete" (Empty, DeleteBody) -> Done
            = |_, body| Action::Run(Command::FilesDelete {
                paths: body.paths,
                recursive: body.recursive,
            });
    }
}

pub mod jobs {
    use super::*;

    endpoints! {
        /// What a job did from step `after` on, and whether it has ended.
        /// A job is kept for a few minutes after it ends.
        Poll: Get "/api/v1/jobs/{job}" (JobQuery, ()) -> JobPage
            = |query, _| Action::Poll { job: query.job, after: query.after };

        /// Stop a job.
        Cancel: Delete "/api/v1/jobs/{job}" (JobRef, ()) -> Done
            = |target, _| Action::Cancel { job: target.job };
    }
}

/// Every endpoint of every group.
pub fn all() -> Vec<Route> {
    [
        device::routes(),
        access::routes(),
        ssh::routes(),
        config::routes(),
        screen::routes(),
        browser::routes(),
        network::routes(),
        storage::routes(),
        audio::routes(),
        camera::routes(),
        time::routes(),
        script::routes(),
        schedule::routes(),
        printer::routes(),
        update::routes(),
        files::routes(),
        jobs::routes(),
    ]
    .concat()
}

/// The route for a request, a literal path before one with a `{name}`.
pub fn find<'a>(
    routes: &'a [Route],
    method: Method,
    path: &str,
) -> Option<(&'a Route, Vec<(String, String)>)> {
    let mut found: Option<(&Route, Vec<(String, String)>)> = None;
    for route in routes.iter().filter(|route| route.method == method) {
        if let Some(captured) = route.matches(path) {
            match &found {
                Some((best, _)) if best.is_literal() || !route.is_literal() => {}
                _ => found = Some((route, captured)),
            }
        }
    }
    found
}

/// Whether any route answers `path`, whatever the method: a wrong method
/// is then a 405 rather than a 404.
pub fn known(routes: &[Route], path: &str) -> bool {
    routes.iter().any(|route| route.matches(path).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_is_under_the_version_and_unique() {
        let routes = all();
        let mut seen = std::collections::HashSet::new();
        for route in &routes {
            assert!(route.path.starts_with("/api/v1/"), "{}", route.path);
            assert!(
                seen.insert((route.method, route.path)),
                "{} {} twice",
                route.method.as_str(),
                route.path
            );
            assert!(!route.doc.trim().is_empty(), "{} has no doc", route.path);
        }
    }

    #[test]
    fn only_identification_claim_and_signing_in_are_public() {
        let public: Vec<&str> = all()
            .iter()
            .filter(|route| route.public)
            .map(|route| route.path)
            .collect();
        assert_eq!(
            public,
            [
                "/api/v1/device/id",
                "/api/v1/device/ping",
                "/api/v1/access/claim",
                "/api/v1/access/session",
                "/api/v1/access/session",
                "/api/v1/access/session",
                "/api/v1/access/ticket/redeem"
            ]
        );
    }

    #[test]
    fn a_target_fills_the_path_and_the_query() {
        assert_eq!(
            target::<script::Run>(&ScriptRef {
                script: "night".into()
            })
            .unwrap(),
            "/api/v1/scripts/night/run"
        );
        assert_eq!(
            target::<config::Get>(&ConfigQuery { key: None }).unwrap(),
            "/api/v1/config"
        );
        assert_eq!(
            target::<config::Get>(&ConfigQuery {
                key: Some("browser.url".into())
            })
            .unwrap(),
            "/api/v1/config?key=browser.url"
        );
        assert_eq!(
            target::<jobs::Poll>(&JobQuery {
                job: "a b/c".into(),
                after: 3
            })
            .unwrap(),
            "/api/v1/jobs/a%20b%2Fc?after=3"
        );
    }

    #[test]
    fn a_request_decodes_into_its_action() {
        let routes = all();
        let (route, captured) = find(&routes, Method::Get, "/api/v1/jobs/a%20b").unwrap();
        let mut pairs = captured;
        pairs.push(("after".into(), "7".into()));
        assert_eq!(
            (route.decode)(&pairs, b"").unwrap(),
            Action::Poll {
                job: "a b".into(),
                after: 7
            }
        );

        let (route, _) = find(&routes, Method::Post, "/api/v1/config/set").unwrap();
        let action = (route.decode)(&[], br#"{"values":{"a":"b"}}"#).unwrap();
        assert!(matches!(
            action,
            Action::Run(Command::Set {
                apply: true,
                verify: Verify::Gateway,
                ..
            })
        ));
        assert!((route.decode)(&[], b"").is_err());

        let (route, _) = find(&routes, Method::Get, "/api/v1/network/wifi/scan").unwrap();
        assert_eq!(
            (route.decode)(&[], b"").unwrap(),
            Action::Run(Command::WifiScan {
                interface: None,
                rescan: true
            })
        );
        assert_eq!(
            (route.decode)(&[("rescan".into(), "false".into())], b"").unwrap(),
            Action::Run(Command::WifiScan {
                interface: None,
                rescan: false
            })
        );
    }

    #[test]
    fn a_literal_path_wins_over_a_name() {
        let routes = all();
        let (route, _) = find(&routes, Method::Post, "/api/v1/schedules/check").unwrap();
        assert_eq!(route.path, "/api/v1/schedules/check");
        let (route, captured) = find(&routes, Method::Delete, "/api/v1/schedules/check").unwrap();
        assert_eq!(route.path, "/api/v1/schedules/{schedule}");
        assert_eq!(captured, [("schedule".to_string(), "check".to_string())]);
        assert!(find(&routes, Method::Put, "/api/v1/schedules").is_none());
        assert!(known(&routes, "/api/v1/schedules"));
        assert!(!known(&routes, "/api/v1/nothing"));
    }

    #[test]
    fn a_raw_body_arrives_as_bytes() {
        let routes = all();
        let (route, _) = find(&routes, Method::Put, "/api/v1/files/upload").unwrap();
        assert!(route.raw_body);
        let pairs = [
            ("path".to_string(), "a.txt".to_string()),
            ("offset".to_string(), "0".to_string()),
        ];
        assert_eq!(
            (route.decode)(&pairs, b"abc").unwrap(),
            Action::Run(Command::FilesChunk {
                path: "a.txt".into(),
                offset: 0,
                data: "YWJj".into()
            })
        );
    }

    #[test]
    fn a_download_is_raw_with_its_size() {
        let answer = <files::Read as Endpoint>::respond(serde_json::json!({
            "data": "YWJj", "size": 10, "mtime": 5
        }))
        .unwrap();
        let Answer::Raw(raw) = answer else {
            panic!("not raw")
        };
        assert_eq!(raw.body, b"abc");
        assert!(raw.headers.contains(&(HEADER_SIZE, "10".to_string())));
        assert!(raw.headers.contains(&(HEADER_MTIME, "5".to_string())));
    }

    #[test]
    fn jobs_start_the_stepped_commands() {
        for route in all() {
            let Ok(action) = (route.decode)(
                &[("host".into(), "a".into())],
                br#"{"host":"a.test","check":true}"#,
            ) else {
                continue;
            };
            if let Action::Start(command) = action {
                assert!(command.is_job(), "{}", route.path);
            }
        }
    }
}
