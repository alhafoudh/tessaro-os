//! The device's API: HTTP over the local unix socket and HTTPS on TCP, the
//! same routes on both (docs/api.md).
//!
//! Every endpoint is a type in `protocol::api`; this module routes a request
//! to one, decodes it into an `Action` and runs it through `Control`. What
//! differs between the ways in is who is asking:
//!
//! * **`/run/tessaro-agent.sock`** is mode 0600, root. No token, no TLS -
//!   whoever can open it is already root on the device. Deliberately not
//!   group accessible: Chromium runs as `weston`, and a compromised browser
//!   must not be one `connect()` away from the control plane.
//! * **TCP** (`access.listen`, default `0.0.0.0:7400`) is TLS with the
//!   device's own self-signed identity, which clients pin. An unclaimed
//!   device answers everything without a token; a claimed one wants
//!   `Authorization: Bearer <token>` for all but the public endpoints.
//!   Failed tokens are counted per address, and an address that keeps
//!   failing is refused for a minute.
//!
//! Besides the API, the same port serves Webconfig at `/` and Swagger UI at
//! `/api/docs/`, and `/api/v1/openapi.json` (`protocol::openapi`). A
//! browser signs in to a claimed device with a session cookie instead of a
//! token (`sessions`, docs/webconfig.md).
//!
//! Every wait on a client is bounded - the TLS handshake, each body, each
//! connection's whole life - so a client that connects and says nothing
//! costs one task until its deadline, never a stuck server.

mod jobs;
pub mod sessions;
mod statics;

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    HeaderName, HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, LOCATION,
    ORIGIN, SET_COOKIE,
};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use protocol::api::{self, Action, Answer, ApiError, ErrorCode, Method, Route, Web};
use protocol::{JobStarted, Via, WebSession};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::control::{After, Caller, Control};
use crate::deadline::within;
use crate::identity::Tls;
use crate::log::Log;
use crate::paths::Paths;
use crate::sync::lock;

use jobs::Jobs;
use sessions::{Owner, Sessions};

/// Sent by every browser that sends `Origin` too: whose page made the request.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Webconfig's pages: its own scripts, styles and fonts only; screenshots
/// and downloads are `blob:` URLs, the setup QR code an SVG `data:` one.
const WEBCONFIG_CSP: &str = "default-src 'self'; img-src 'self' blob: data:; \
    style-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; \
    form-action 'self'";

const HANDSHAKE: Duration = Duration::from_secs(10);
/// A connection is closed, gracefully, after this long: a client keeps one
/// open between requests and opens another when it is gone.
const REMOTE_LIFE: Duration = Duration::from_secs(10 * 60);
const LOCAL_LIFE: Duration = Duration::from_secs(60 * 60);
/// What a request in flight gets to finish when its connection is closed.
const GRACE: Duration = Duration::from_secs(30);
/// Reading a JSON body, and a piece of an upload, which can be large on a
/// slow link.
const BODY_LIMIT: Duration = Duration::from_secs(10);
const UPLOAD_LIMIT: Duration = Duration::from_secs(120);
/// The largest JSON body: an image's bmap is the largest thing sent as
/// JSON, a script for `eval` the next.
const BODY_MAX: usize = 8 * 1024 * 1024;
/// The largest raw body, one piece of an upload with room for nothing else.
const UPLOAD_MAX: usize = protocol::UPDATE_CHUNK;
/// The longest the work that takes this process or the network down with
/// it (`After`) waits for its answer to go out.
const AFTER_LIMIT: Duration = Duration::from_secs(30);

/// Failed tokens from one address within `WINDOW` before it is refused.
const STRIKES: u32 = 5;
const WINDOW: Duration = Duration::from_secs(60);
const PENALTY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
enum Origin {
    Local,
    Remote(SocketAddr),
}

pub struct Server {
    control: Arc<Control>,
    log: Arc<Log>,
    routes: Vec<Route>,
    jobs: Arc<Jobs>,
    limiter: Limiter,
    sessions: Arc<Sessions>,
    /// The OpenAPI document, built once.
    openapi: Bytes,
    webconfig_root: PathBuf,
    api_docs: PathBuf,
}

impl Server {
    pub fn new(control: Arc<Control>, paths: &Paths, log: Arc<Log>) -> Arc<Self> {
        let document = protocol::openapi::document(env!("CARGO_PKG_VERSION"));
        Arc::new(Self {
            sessions: control.sessions(),
            control,
            log,
            routes: api::all(),
            jobs: Arc::new(Jobs::default()),
            limiter: Limiter::default(),
            openapi: Bytes::from(serde_json::to_vec_pretty(&document).unwrap_or_default()),
            webconfig_root: paths.webconfig_root.clone(),
            api_docs: paths.api_docs.clone(),
        })
    }
}

pub fn spawn_unix(
    server: Arc<Server>,
    path: PathBuf,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    // A socket left behind by the previous run would make bind() fail.
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_socket() {
            std::fs::remove_file(&path)?;
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let listener = UnixListener::bind(&path)?;
    // /run is root-only-writable, and nobody but root can connect() to a
    // socket they cannot write to, so the umask window before this is closed.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    server
        .log
        .info(format!("api: listening on {}", path.display()));

    tokio::spawn(async move {
        let mut stop = shutdown.clone();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted, // naked: an accept loop waits for as long as the agent runs
                _ = stop.changed() => break,
            };
            match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(connection(
                        Arc::clone(&server),
                        stream,
                        Origin::Local,
                        shutdown.clone(),
                    ));
                }
                Err(err) => server
                    .log
                    .info(format!("api: accept on the socket failed: {err}")),
            }
        }
        let _ = std::fs::remove_file(&path);
    });
    Ok(())
}

pub async fn spawn_tls(
    server: Arc<Server>,
    addr: SocketAddr,
    tls: &Tls,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let identity =
        native_tls::Identity::from_pkcs8(&tls.cert_pem, &tls.key_pem).map_err(io::Error::other)?;
    let acceptor = native_tls::TlsAcceptor::builder(identity)
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .build()
        .map_err(io::Error::other)?;
    let acceptor = tokio_native_tls::TlsAcceptor::from(acceptor);

    let listener = TcpListener::bind(addr).await?; // naked: bind() is a local syscall, not a wait
    server.log.info(format!(
        "api: listening on {addr} (TLS, fingerprint {})",
        tls.fingerprint
    ));

    tokio::spawn(async move {
        let mut stop = shutdown.clone();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted, // naked: an accept loop waits for as long as the agent runs
                _ = stop.changed() => break,
            };
            let (tcp, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(err) => {
                    server.log.info(format!("api: accept failed: {err}"));
                    continue;
                }
            };

            let acceptor = acceptor.clone();
            let server = Arc::clone(&server);
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                match within("a TLS handshake", HANDSHAKE, acceptor.accept(tcp)).await {
                    Ok(Ok(stream)) => {
                        // naked: connection bounds the connection's whole life
                        connection(server, stream, Origin::Remote(peer), shutdown).await
                    }
                    Ok(Err(err)) => server
                        .log
                        .debug(format!("api: TLS handshake with {peer} failed: {err}")),
                    Err(expired) => server.log.debug(format!("api: {peer}: {expired}")),
                }
            });
        }
    });
    Ok(())
}

/// One connection, its requests one after another, for at most its life.
async fn connection<S>(
    server: Arc<Server>,
    stream: S,
    origin: Origin,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let life = match origin {
        Origin::Local => LOCAL_LIFE,
        Origin::Remote(_) => REMOTE_LIFE,
    };
    let service = {
        let server = Arc::clone(&server);
        hyper::service::service_fn(move |request| {
            let server = Arc::clone(&server);
            async move {
                // naked: respond waits only through Control, Jobs and blocking(), each bounded
                Ok::<_, Infallible>(server.respond(request, origin).await)
            }
        })
    };
    let connection = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);

    let open = tokio::select! {
        ended = within("an API connection", life, connection.as_mut()) => ended.is_err(),
        _ = shutdown.changed() => true,
    };
    if open {
        connection.as_mut().graceful_shutdown();
        let _ = within("closing an API connection", GRACE, connection).await;
    }
}

impl Server {
    async fn respond(&self, request: Request<Incoming>, origin: Origin) -> Response<Body> {
        let path = request.uri().path().to_string();
        let method = match *request.method() {
            hyper::Method::GET | hyper::Method::HEAD => Method::Get,
            hyper::Method::POST => Method::Post,
            hyper::Method::PUT => Method::Put,
            hyper::Method::PATCH => Method::Patch,
            hyper::Method::DELETE => Method::Delete,
            _ => {
                return error(
                    StatusCode::METHOD_NOT_ALLOWED,
                    ErrorCode::BadRequest,
                    "no such method",
                )
            }
        };

        if path == "/api/v1/openapi.json" && method == Method::Get {
            return bytes(StatusCode::OK, "application/json", self.openapi.clone());
        }
        if path == "/api/docs" {
            return redirect("/api/docs/");
        }
        if let Some(rest) = path.strip_prefix("/api/docs/") {
            // naked: statics::read is blocking() under within()
            return match statics::read(&self.api_docs, rest, false).await {
                Some(file) => static_file(file),
                None => not_found(),
            };
        }
        if path.starts_with("/api/") {
            // naked: api waits only through Control, Jobs and the body read, each bounded
            return self.api(request, method, &path, origin).await;
        }
        if method != Method::Get {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                ErrorCode::BadRequest,
                "only the API under /api/ takes writes",
            );
        }
        // naked: statics::read is blocking() under within()
        match statics::read(&self.webconfig_root, &path, true).await {
            Some(file) => {
                let html = file.content_type.starts_with("text/html");
                let mut response = static_file(file);
                if html {
                    // Webconfig loads nothing from elsewhere and runs no
                    // inline script: whatever got into a page could not.
                    response.headers_mut().insert(
                        HeaderName::from_static("content-security-policy"),
                        HeaderValue::from_static(WEBCONFIG_CSP),
                    );
                }
                response
            }
            None => not_found(),
        }
    }

    async fn api(
        &self,
        request: Request<Incoming>,
        method: Method,
        path: &str,
        origin: Origin,
    ) -> Response<Body> {
        let Some((route, captured)) = api::find(&self.routes, method, path) else {
            return if api::known(&self.routes, path) {
                error(
                    StatusCode::METHOD_NOT_ALLOWED,
                    ErrorCode::BadRequest,
                    "that path takes another method",
                )
            } else {
                not_found()
            };
        };
        let route = *route;

        if let Err(refusal) = guard(request.headers(), method, route.raw_body) {
            return refused(refusal);
        }
        let (admitted, stale) = self.authenticate(origin, &request, route.public);
        let (caller, via) = match admitted {
            Ok(admitted) => admitted,
            Err(refusal) => {
                let mut response = refused(refusal);
                if stale {
                    set_cookie(&mut response, &self.sessions.clear_cookie());
                }
                return response;
            }
        };
        let from_browser = request.headers().contains_key(ORIGIN)
            || request.headers().contains_key(SEC_FETCH_SITE);
        let active = is_active(request.headers());
        let cookie = self.cookie(request.headers()).map(str::to_string);

        let mut pairs = captured;
        let query = request.uri().query().unwrap_or("");
        match serde_urlencoded::from_str::<Vec<(String, String)>>(query) {
            Ok(given) => pairs.extend(given),
            Err(err) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::BadRequest,
                    &format!("the query: {err}"),
                )
            }
        }

        let (max, limit) = match route.raw_body {
            true => (UPLOAD_MAX, UPLOAD_LIMIT),
            false => (BODY_MAX, BODY_LIMIT),
        };
        let body = Limited::new(request.into_body(), max);
        let body = match within("reading a request body", limit, body.collect()).await {
            Ok(Ok(collected)) => collected.to_bytes(),
            Ok(Err(err)) if err.is::<http_body_util::LengthLimitError>() => {
                return error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    ErrorCode::TooLarge,
                    &format!("the body is larger than {max} bytes"),
                )
            }
            Ok(Err(err)) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::BadRequest,
                    &format!("reading the body: {err}"),
                )
            }
            Err(expired) => {
                return error(
                    StatusCode::REQUEST_TIMEOUT,
                    ErrorCode::BadRequest,
                    &expired.to_string(),
                )
            }
        };

        let action = match (route.decode)(&pairs, &body) {
            Ok(action) => action,
            Err(err) => return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, &err),
        };

        let claim = route.path == <api::access::Claim as api::Endpoint>::PATH;
        let mut response = match action {
            Action::Run(command) => {
                let reply = self.control.handle(&caller, command).await; // naked: Control bounds every call it makes
                                                                         // A browser that claims gets its session in the same answer:
                                                                         // on the hotspot the phone is dropped right after it, and a
                                                                         // second request to sign in would never arrive.
                let session = match (&reply.result, claim && from_browser) {
                    (Ok(claimed), true) => self.claim_session(claimed),
                    _ => None,
                };
                let mut response = match reply.result.and_then(route.respond) {
                    Ok(Answer::Json(value)) => json(StatusCode::OK, &value),
                    Ok(Answer::Raw(raw)) => {
                        let mut response =
                            bytes(StatusCode::OK, raw.content_type, Bytes::from(raw.body));
                        for (name, value) in raw.headers {
                            if let Ok(value) = HeaderValue::from_str(&value) {
                                response.headers_mut().insert(name, value);
                            }
                        }
                        response
                    }
                    Err(err) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::Refused, &err),
                };
                if let Some(set) = session {
                    set_cookie(&mut response, &set);
                }
                if let Some(after) = reply.after {
                    let (sent, out) = tokio::sync::oneshot::channel();
                    response.body_mut().sent = Some(sent);
                    self.after(after, out);
                }
                response
            }
            Action::Web(web) => {
                // naked: web waits only through Control, whose reads are blocking() under within()
                self.web(web, &caller, via, cookie.as_deref()).await
            }
            // naked: stream waits only through Control, whose reads are blocking() and the bus
            Action::Start(command) => match self
                .control
                .stream(&caller, command)
                .await
                .and_then(|stream| self.jobs.start(stream))
            {
                Ok(job) => json(StatusCode::OK, &serde_json::json!(JobStarted { job })),
                Err(err) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::Refused, &err),
            },
            Action::Poll { job, after } => match self.jobs.page(&job, after) {
                Some(page) => json(StatusCode::OK, &serde_json::json!(page)),
                None => error(
                    StatusCode::NOT_FOUND,
                    ErrorCode::NotFound,
                    &format!("no job {job}"),
                ),
            },
            Action::Cancel { job } => match self.jobs.cancel(&job) {
                true => json(
                    StatusCode::OK,
                    &serde_json::json!(protocol::Done::new("cancelled")),
                ),
                false => error(
                    StatusCode::NOT_FOUND,
                    ErrorCode::NotFound,
                    &format!("no job {job}"),
                ),
            },
        };

        let has_cookie = response.headers().contains_key(SET_COOKIE);
        match (&cookie, via) {
            // A session someone is using lasts another timeout from now,
            // in the browser too.
            (Some(id), Via::Session) if active && !has_cookie => {
                set_cookie(
                    &mut response,
                    &self.sessions.set_cookie(id, self.control.session_timeout()),
                );
            }
            // A cookie from a session that has ended - every browser brings
            // one after the agent restarted - is forgotten.
            _ if stale && !has_cookie => set_cookie(&mut response, &self.sessions.clear_cookie()),
            _ => {}
        }
        response
    }

    /// The session cookie a request carries, if any.
    fn cookie<'a>(&self, headers: &'a hyper::HeaderMap) -> Option<&'a str> {
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|header| self.sessions.session_in(header))
    }

    /// The `Set-Cookie` for a session of the token a claim just issued.
    fn claim_session(&self, claimed: &Value) -> Option<String> {
        let token_id = claimed["token_id"].as_str()?;
        let owner = Owner {
            token_id: token_id.to_string(),
            token_sha: self.control.token_sha(token_id)?,
        };
        let timeout = self.control.session_timeout();
        match self.sessions.create(owner, timeout) {
            Ok(id) => Some(self.sessions.set_cookie(&id, timeout)),
            Err(err) => {
                self.log
                    .info(format!("api: no session for the claim: {err}"));
                None
            }
        }
    }

    /// What a browser asks of its session.
    async fn web(
        &self,
        web: Web,
        caller: &Caller,
        via: Via,
        cookie: Option<&str>,
    ) -> Response<Body> {
        let peer = match caller {
            Caller::Token { peer, .. } | Caller::Anonymous { peer } => Some(*peer),
            Caller::Local | Caller::Page => None,
        };
        let timeout = self.control.session_timeout();
        match web {
            Web::Session => {
                let session = self.web_session(caller, via).await; // naked: web_session reads through blocking() under within()
                json(StatusCode::OK, &serde_json::json!(session))
            }
            Web::SignOut => {
                if let Some(id) = cookie {
                    self.sessions.end(id);
                }
                let mut response = json(
                    StatusCode::OK,
                    &serde_json::json!(protocol::Done::new("signed out")),
                );
                set_cookie(&mut response, &self.sessions.clear_cookie());
                response
            }
            Web::Ticket => {
                let Caller::Token { id, .. } = caller else {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        ErrorCode::Refused,
                        "a ticket is issued for a token; ask with `Authorization: Bearer`",
                    );
                };
                if via != Via::Token {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        ErrorCode::Refused,
                        "a browser session cannot issue tickets",
                    );
                }
                let Some(token_sha) = self.control.token_sha(id) else {
                    return error(
                        StatusCode::UNAUTHORIZED,
                        ErrorCode::InvalidToken,
                        "invalid token",
                    );
                };
                let owner = Owner {
                    token_id: id.clone(),
                    token_sha,
                };
                match self.sessions.ticket(owner) {
                    Ok(ticket) => json(
                        StatusCode::OK,
                        &serde_json::json!(protocol::Ticket {
                            ticket,
                            expires_in: sessions::TICKET_LIFE.as_secs(),
                        }),
                    ),
                    Err(err) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::Refused, &err),
                }
            }
            Web::SignIn { token } => {
                if !self.control.claimed() {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        ErrorCode::Refused,
                        "this device is unclaimed: it needs no signing in",
                    );
                }
                let owner = self.control.verify(token.trim()).and_then(|id| {
                    let token_sha = self.control.token_sha(&id)?;
                    Some(Owner {
                        token_id: id,
                        token_sha,
                    })
                });
                // naked: sign_in waits only through web_session, blocking() under within()
                self.sign_in(owner, peer, timeout, "a token that is not the device's")
                    .await
            }
            Web::Redeem { ticket } => {
                let owner = self.sessions.redeem(ticket.trim()).filter(|owner| {
                    self.control.token_sha(&owner.token_id).as_deref()
                        == Some(owner.token_sha.as_str())
                });
                // naked: sign_in waits only through web_session, blocking() under within()
                self.sign_in(
                    owner,
                    peer,
                    timeout,
                    "a ticket that is used up, expired or unknown",
                )
                .await
            }
        }
    }

    /// A new session for `owner`, its cookie set in the answer; or, without
    /// one, a strike against `peer`.
    async fn sign_in(
        &self,
        owner: Option<Owner>,
        peer: Option<SocketAddr>,
        timeout: Duration,
        what: &str,
    ) -> Response<Body> {
        if let Some(peer) = peer {
            if self.limiter.refused(peer.ip()) {
                return error(
                    StatusCode::TOO_MANY_REQUESTS,
                    ErrorCode::RateLimited,
                    "too many failed attempts from this address; try again in a minute",
                );
            }
        }
        let Some(owner) = owner else {
            if let Some(peer) = peer {
                self.limiter.strike(peer.ip());
                self.log
                    .info(format!("api: {peer} tried to sign in with {what}"));
            }
            return error(
                StatusCode::UNAUTHORIZED,
                ErrorCode::InvalidToken,
                "invalid token",
            );
        };
        let id = match self.sessions.create(owner.clone(), timeout) {
            Ok(id) => id,
            Err(err) => return error(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, &err),
        };
        let caller = Caller::Token {
            id: owner.token_id,
            peer: peer.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0))),
        };
        let session = self.web_session(&caller, Via::Session).await; // naked: web_session reads through blocking() under within()
        let mut response = json(StatusCode::OK, &serde_json::json!(session));
        set_cookie(&mut response, &self.sessions.set_cookie(&id, timeout));
        response
    }

    async fn web_session(&self, caller: &Caller, via: Via) -> WebSession {
        let token = match caller {
            Caller::Token { id, .. } => self.control.token_info(id),
            _ => None,
        };
        WebSession {
            claimed: self.control.claimed(),
            fresh: self.control.fresh().await, // naked: fresh reads the settings through blocking() under within()
            via,
            token,
            timeout: self.control.session_timeout().as_secs(),
        }
    }

    /// Work that must wait for the answer to be out: restarting the agent or
    /// the compositor takes this process down, a network change the client's
    /// connection. It runs once hyper has taken the whole answer (`out`), or
    /// the connection is gone; the spawn puts it behind the write hyper
    /// makes in the same poll.
    fn after(&self, after: After, out: tokio::sync::oneshot::Receiver<()>) {
        let control = Arc::clone(&self.control);
        tokio::spawn(async move {
            let _ = within("an answer going out", AFTER_LIMIT, out).await;
            control.run_after(after).await; // naked: run_after is under within() inside Bus
        });
    }

    /// Who is asking and how they got in, and whether they brought the
    /// cookie of a session that has ended - which is forgotten whether the
    /// request is then let in or not.
    fn authenticate(
        &self,
        origin: Origin,
        request: &Request<Incoming>,
        public: bool,
    ) -> (Result<(Caller, Via), ApiError>, bool) {
        let peer = match origin {
            Origin::Local => return (Ok((Caller::Local, Via::Local)), false),
            Origin::Remote(peer) => peer,
        };
        let headers = request.headers();
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim);
        // A token wins over a cookie: ctl and the GUI never send one, and a
        // browser never sends a token.
        let mut stale = false;
        if token.is_none() {
            if let Some(id) = self.cookie(headers) {
                let owner = self.sessions.check(
                    id,
                    self.control.session_timeout(),
                    is_active(headers),
                    |token_id| self.control.token_sha(token_id),
                );
                match owner {
                    Some(owner) => {
                        let caller = Caller::Token {
                            id: owner.token_id,
                            peer,
                        };
                        return (Ok((caller, Via::Session)), false);
                    }
                    // Not a strike: the cookie was the device's own, and it
                    // is the browser's next request after a restart.
                    None => {
                        stale = true;
                        self.log
                            .debug(format!("api: {peer} brought an ended session's cookie"));
                    }
                }
            }
        }
        let admitted =
            admit(&self.control, &self.limiter, &self.log, peer, token, public).map(|caller| {
                let via = match caller {
                    Caller::Token { .. } => Via::Token,
                    _ => Via::Anonymous,
                };
                (caller, via)
            });
        (admitted, stale)
    }
}

/// The request is someone's doing, not a page refreshing itself.
fn is_active(headers: &hyper::HeaderMap) -> bool {
    headers.contains_key(api::HEADER_ACTIVITY)
}

/// Browser requests only from the device's own origin. Another site's page
/// cannot read the answers - there is no CORS - but it could make the
/// browser send a write, with a cookie, or to an unclaimed device with none.
/// A request with neither `Origin` nor `Sec-Fetch-Site` is not a browser's
/// (ctl, the GUI, curl) and passes.
fn guard(headers: &hyper::HeaderMap, method: Method, raw_body: bool) -> Result<(), ApiError> {
    let refuse = |error: &str| ApiError {
        error: error.to_string(),
        code: ErrorCode::CrossOrigin,
    };
    let text = |name| {
        headers
            .get(name)
            .and_then(|value: &HeaderValue| value.to_str().ok())
    };
    // `none` is someone typing the address; `same-site` is another port of
    // the same host, which is another origin.
    if matches!(text(SEC_FETCH_SITE), Some("cross-site" | "same-site")) {
        return Err(refuse("a request from another site"));
    }
    if method == Method::Get {
        return Ok(());
    }
    let Some(origin) = text(ORIGIN) else {
        return Ok(());
    };
    let host = text(HOST).unwrap_or("");
    if host.is_empty() || origin != format!("https://{host}") {
        return Err(refuse("a request from another origin"));
    }
    // A form can post text/plain across sites without asking; a JSON or
    // an octet-stream body cannot, so insisting on them closes that door
    // even for a browser that sends no Origin.
    let expected = if raw_body {
        "application/octet-stream"
    } else {
        "application/json"
    };
    match text(CONTENT_TYPE) {
        Some(given) if given.split(';').next().map(str::trim) != Some(expected) => {
            Err(refuse(&format!("the body must be {expected}")))
        }
        _ => Ok(()),
    }
}

fn set_cookie(response: &mut Response<Body>, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(SET_COOKIE, value);
    }
}

/// Who `peer` is, with `token`, for an endpoint that is `public` or not.
fn admit(
    control: &Control,
    limiter: &Limiter,
    log: &Log,
    peer: SocketAddr,
    token: Option<&str>,
    public: bool,
) -> Result<Caller, ApiError> {
    let refuse = |code: ErrorCode, error: &str| ApiError {
        error: error.to_string(),
        code,
    };
    match token {
        // A valid token always gets in. Tokens are 256 bits, so the limiter
        // is not what stops guessing - it keeps a scan out of the journal and
        // off the CPU - and it must not lock out someone who shares the
        // scanner's NAT.
        Some(token) => match control.verify(token) {
            Some(id) => Ok(Caller::Token { id, peer }),
            // A stale token - a client that still holds one from before an
            // unclaim - is no reason to refuse what needs none.
            None if public || !control.claimed() => Ok(Caller::Anonymous { peer }),
            None if limiter.refused(peer.ip()) => Err(refuse(
                ErrorCode::RateLimited,
                "too many failed attempts from this address; try again in a minute",
            )),
            None => {
                limiter.strike(peer.ip());
                log.info(format!("api: {peer} presented an invalid token"));
                Err(refuse(ErrorCode::InvalidToken, "invalid token"))
            }
        },
        // Until the first claim, anyone who reaches the device manages it:
        // they could claim it and do the same. What makes a credential still
        // refuses on an unclaimed device (`Control::require_claimed`).
        None if public || !control.claimed() => Ok(Caller::Anonymous { peer }),
        None => Err(refuse(
            ErrorCode::TokenRequired,
            "a token is required; `tessaro-ctl access login` with one",
        )),
    }
}

fn with_headers(mut response: Response<Body>, content_type: &str) -> Response<Body> {
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(CONTENT_TYPE, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// An answer's body, which says when hyper has taken the last of it: the
/// moment `After` work may take the answer's connection down.
pub struct Body {
    inner: Full<Bytes>,
    sent: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Body {
    fn new(bytes: Bytes) -> Self {
        Self {
            inner: Full::new(bytes),
            sent: None,
        }
    }
}

impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Infallible>>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_frame(cx);
        if let std::task::Poll::Ready(None) = polled {
            if let Some(sent) = this.sent.take() {
                let _ = sent.send(());
            }
        }
        polled
    }

    /// Not the end until hyper has asked past the last frame, which is what
    /// fires `sent`.
    fn is_end_stream(&self) -> bool {
        self.sent.is_none() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

fn bytes(status: StatusCode, content_type: &str, body: Bytes) -> Response<Body> {
    let mut response = Response::new(Body::new(body));
    *response.status_mut() = status;
    with_headers(response, content_type)
}

/// A static file: a hashed asset is cached for good, everything else is
/// asked for again. A page may not be framed by another site, and nothing
/// is sniffed into another type.
fn static_file(file: statics::File) -> Response<Body> {
    let html = file.content_type.starts_with("text/html");
    let mut response = bytes(StatusCode::OK, file.content_type, Bytes::from(file.body));
    let headers = response.headers_mut();
    if file.immutable {
        headers.insert(
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    if html {
        headers.insert(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        );
        headers.insert(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        );
    }
    response
}

fn json(status: StatusCode, value: &Value) -> Response<Body> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    bytes(status, "application/json", Bytes::from(body))
}

fn error(status: StatusCode, code: ErrorCode, message: &str) -> Response<Body> {
    let body = ApiError {
        error: message.to_string(),
        code,
    };
    json(status, &serde_json::json!(body))
}

fn refused(refusal: ApiError) -> Response<Body> {
    let status = StatusCode::from_u16(refusal.code.status()).unwrap_or(StatusCode::FORBIDDEN);
    json(status, &serde_json::json!(refusal))
}

fn not_found() -> Response<Body> {
    error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "no such path")
}

fn redirect(to: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::new(Bytes::new()));
    *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
    response
        .headers_mut()
        .insert(LOCATION, HeaderValue::from_static(to));
    response
}

#[derive(Default)]
struct Limiter(Mutex<HashMap<IpAddr, Strikes>>);

struct Strikes {
    count: u32,
    since: Instant,
    refused_until: Option<Instant>,
}

impl Limiter {
    fn refused(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let map = lock(&self.0);
        map.get(&ip)
            .and_then(|strikes| strikes.refused_until)
            .is_some_and(|until| now < until)
    }

    fn strike(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut map = lock(&self.0);
        // Forget addresses that have been quiet, so the map cannot grow
        // without bound under a scan.
        map.retain(|_, strikes| {
            now.duration_since(strikes.since) < WINDOW
                || strikes.refused_until.is_some_and(|until| now < until)
        });

        let strikes = map.entry(ip).or_insert(Strikes {
            count: 0,
            since: now,
            refused_until: None,
        });
        if now.duration_since(strikes.since) >= WINDOW {
            strikes.count = 0;
            strikes.since = now;
        }
        strikes.count += 1;
        if strikes.count >= STRIKES {
            strikes.refused_until = Some(now + PENALTY);
            strikes.count = 0;
            strikes.since = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Command;

    #[tokio::test(start_paused = true)]
    async fn an_address_that_keeps_failing_is_refused_for_a_while() {
        let limiter = Limiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();

        for _ in 0..STRIKES - 1 {
            limiter.strike(ip);
        }
        assert!(!limiter.refused(ip));
        limiter.strike(ip);
        assert!(limiter.refused(ip));
        assert!(!limiter.refused(other));

        tokio::time::advance(PENALTY + Duration::from_secs(1)).await;
        assert!(!limiter.refused(ip));
    }

    #[tokio::test(start_paused = true)]
    async fn strikes_spread_out_over_time_are_forgiven() {
        let limiter = Limiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        for _ in 0..STRIKES * 3 {
            limiter.strike(ip);
            tokio::time::advance(WINDOW).await;
        }
        assert!(!limiter.refused(ip));
    }

    #[tokio::test]
    async fn unclaimed_answers_everything_and_claimed_only_the_public() {
        let fx = crate::control::fixture();
        let limiter = Limiter::default();
        let log = Log::buffered(true);
        let peer: SocketAddr = "192.0.2.10:50000".parse().unwrap();
        let admitted = |token: Option<&str>, public: bool| {
            admit(&fx.control, &limiter, &log, peer, token, public).map_err(|refusal| refusal.code)
        };

        assert_eq!(admitted(None, false), Ok(Caller::Anonymous { peer }));
        assert_eq!(
            admitted(Some("stale"), false),
            Ok(Caller::Anonymous { peer })
        );

        let claim = Command::Claim {
            name: "laptop".into(),
        };
        let reply = fx.control.handle(&Caller::Anonymous { peer }, claim).await;
        assert!(reply.result.is_ok(), "{:?}", reply.result);
        let token = reply.result.unwrap()["token"].as_str().unwrap().to_string();

        assert_eq!(admitted(None, false), Err(ErrorCode::TokenRequired));
        assert_eq!(admitted(None, true), Ok(Caller::Anonymous { peer }));
        assert_eq!(admitted(Some("stale"), false), Err(ErrorCode::InvalidToken));
        assert_eq!(
            admitted(Some("stale"), true),
            Ok(Caller::Anonymous { peer })
        );
        assert!(matches!(
            admitted(Some(&token), false),
            Ok(Caller::Token { .. })
        ));
    }

    /// One request over the socket, the way a client sends it, and the
    /// status and body of the answer.
    async fn ask(socket: &std::path::Path, request: String) -> (u16, String) {
        let socket = socket.to_path_buf();
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            let status = answer[9..12].parse().unwrap();
            let body = answer.split_once("\r\n\r\n").unwrap().1.to_string();
            (status, body)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_socket_answers_the_api_and_refuses_what_does_not_parse() {
        let fx = crate::control::fixture();
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let (_stop, shutdown) = watch::channel(false);
        let server = Server::new(
            Arc::clone(&fx.control),
            &fx.paths,
            Arc::new(Log::buffered(true)),
        );
        spawn_unix(server, socket.clone(), shutdown).unwrap();

        let get =
            |path: &str| format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        let (status, body) = ask(&socket, get("/api/v1/device/id")).await;
        assert_eq!(status, 200, "{body}");
        let node: protocol::NodeInfo = serde_json::from_str(&body).unwrap();
        assert!(!node.claimed);

        let (status, body) = ask(&socket, get("/api/v1/config?key=browser.url")).await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("browser.url"));

        let (status, body) = ask(&socket, get("/api/v1/nothing")).await;
        assert_eq!(status, 404);
        assert!(body.contains("not-found"));

        let bad = "POST /api/v1/config/set HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
                   Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let (status, body) = ask(&socket, bad.to_string()).await;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("bad-request"));

        let (status, body) = ask(&socket, get("/api/v1/openapi.json")).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"openapi\": \"3.1.0\""));
    }

    fn headers(pairs: &[(&'static str, &str)]) -> hyper::HeaderMap {
        let mut map = hyper::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_the_device_s_own_origin_may_write_from_a_browser() {
        let code = |pairs: &[(&'static str, &str)], method, raw| {
            guard(&headers(pairs), method, raw).map_err(|refusal| refusal.code)
        };
        let own = [
            ("host", "192.0.2.1:7400"),
            ("origin", "https://192.0.2.1:7400"),
            ("content-type", "application/json"),
        ];
        // Not a browser: ctl, the GUI, curl.
        assert_eq!(code(&[("host", "x")], Method::Post, false), Ok(()));
        assert_eq!(code(&own, Method::Post, false), Ok(()));
        assert_eq!(code(&own[..2], Method::Post, false), Ok(()), "no body");
        assert_eq!(
            code(
                &[
                    ("host", "192.0.2.1:7400"),
                    ("origin", "https://192.0.2.1:7400"),
                    ("content-type", "application/octet-stream"),
                ],
                Method::Put,
                true
            ),
            Ok(())
        );

        let refused = Err(ErrorCode::CrossOrigin);
        let elsewhere = [
            ("host", "192.0.2.1:7400"),
            ("origin", "https://evil.test"),
            ("content-type", "application/json"),
        ];
        assert_eq!(code(&elsewhere, Method::Post, false), refused);
        let plain_http = [
            ("host", "192.0.2.1:7400"),
            ("origin", "http://192.0.2.1:7400"),
        ];
        assert_eq!(code(&plain_http, Method::Delete, false), refused);
        let form = [
            ("host", "192.0.2.1:7400"),
            ("origin", "https://192.0.2.1:7400"),
            ("content-type", "text/plain"),
        ];
        assert_eq!(code(&form, Method::Post, false), refused);
        assert_eq!(
            code(&own, Method::Put, true),
            refused,
            "JSON for a raw body"
        );

        // Another site's page may not even read through the browser's cookie.
        for site in ["cross-site", "same-site"] {
            let from = [("host", "192.0.2.1:7400"), ("sec-fetch-site", site)];
            assert_eq!(code(&from, Method::Get, false), refused, "{site}");
        }
        for site in ["same-origin", "none"] {
            let from = [("host", "192.0.2.1:7400"), ("sec-fetch-site", site)];
            assert_eq!(code(&from, Method::Get, false), Ok(()), "{site}");
        }
        // A GET is answered to its own origin only anyway.
        assert_eq!(code(&elsewhere, Method::Get, false), Ok(()));
    }

    #[tokio::test]
    async fn the_session_cookie_is_read_among_others() {
        let fx = crate::control::fixture();
        let server = Server::new(
            Arc::clone(&fx.control),
            &fx.paths,
            Arc::new(Log::buffered(true)),
        );
        let name = server.sessions.cookie_name().to_string();
        let mut map = hyper::HeaderMap::new();
        map.append(COOKIE, HeaderValue::from_static("a=b"));
        map.append(
            COOKIE,
            HeaderValue::from_str(&format!("c=d; {name}=abc")).unwrap(),
        );
        assert_eq!(server.cookie(&map), Some("abc"));
        assert_eq!(server.cookie(&hyper::HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn a_body_says_it_is_out_only_after_its_last_byte() {
        let (sent, mut out) = tokio::sync::oneshot::channel();
        let mut body = Body::new(Bytes::from_static(b"{}"));
        body.sent = Some(sent);
        assert!(!hyper::body::Body::is_end_stream(&body));

        let frame = body.frame().await.unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), Bytes::from_static(b"{}"));
        assert!(out.try_recv().is_err(), "said so before the end");
        assert!(body.frame().await.is_none());
        assert!(out.try_recv().is_ok());
    }

    /// The document this agent serves is the one checked in, for code
    /// generators. `UPDATE_OPENAPI=1 cargo test` writes it.
    #[test]
    fn the_checked_in_openapi_document_is_current() {
        let document = protocol::openapi::document(env!("CARGO_PKG_VERSION"));
        let text = format!("{}\n", serde_json::to_string_pretty(&document).unwrap());
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../protocol/openapi.json");
        if std::env::var_os("UPDATE_OPENAPI").is_some() {
            std::fs::write(file, &text).unwrap();
            return;
        }
        let checked_in = std::fs::read_to_string(file).unwrap_or_default();
        assert!(
            checked_in == text,
            "agent/protocol/openapi.json is out of date; run \
             `UPDATE_OPENAPI=1 cargo test -p tessaro-agent openapi` and commit it"
        );
    }
}
