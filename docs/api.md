# The API

**One HTTP API is the only way into a device.** `tessaro-ctl`, `tessaro-gui`,
the setup page on a phone and anyone's own program all call the same
endpoints, so none of them can do what the others cannot, and every change
goes through the same validation, transactions and journal lines. The page
bridge (`window.tessaro`, [bridge.md](bridge.md)) is the one other way in,
and it maps its calls onto the same commands.

## Endpoints are types

**Every endpoint is one type in `agent/protocol/src/api.rs`**: its method and
path, the query it takes (`Params`), its JSON body (`Body`) and what it
answers (`Response`). The agent routes on those types (`api::all`, served by
`agent/tessaro-agent/src/api/`), the OpenAPI document is built from them
(`protocol/src/openapi.rs`), and the clients call an endpoint by its type
(`Session::call::<api::device::Status>` in `agent/client/src/connect.rs`).
Sending the wrong body or reading the wrong answer is a compile error in
Rust, and cannot drift from what the device serves. There is no code
generator in between: the Rust clients use the types themselves.

* **Paths are `/api/v1/<group>/...`**, the group being the `tessaro-ctl`
  command group that does the same thing. `{name}` in a path is a field of
  `Params`; the rest of `Params` is the query string, encoded with
  `serde_urlencoded` on both ends.
* **Each endpoint maps its request onto one `Command`** (`Endpoint::action`),
  the agent's own model of what it can be asked, which `Control::handle` runs.
  `Command` never crosses the network.
* **The OpenAPI document is generated, served and checked in.** The agent
  serves it at `/api/v1/openapi.json` and Swagger UI at `/api/docs/`;
  `agent/protocol/openapi.json` is the same document for other languages'
  code generators. A test in tessaro-agent fails when the two differ;
  `UPDATE_OPENAPI=1 cargo test -p tessaro-agent openapi` rewrites it. Every
  schema is what schemars derives from the Rust type, serde attributes
  included, so the document cannot describe a field the code does not have.
* **Swagger UI is its own recipe**, `tessaro-api-docs`: swagger-ui-dist's
  released files with our `swagger-initializer.js`. `utoipa-swagger-ui`
  downloads the same files at build time, which the offline cargo build of
  tessaro-kiosk cannot.
* **The router is hyper's, by hand**, not axum and tower: the routes are
  data (`api::find`), a literal path beating one with a `{name}` in the same
  place, and the whole server is one file.

## Ways in

* **HTTPS on `access.listen`** (default `0.0.0.0:7400`, `off` disables it).
  The device makes an EC P-256 key and a self-signed certificate in
  `/data/tessaro/tls/` on first boot, valid from 1970 to 9999 so a wrong
  clock cannot break it. The key survives an unclaim and a factory reset.
* **Plain HTTP on `/run/tessaro-agent.sock`**, mode 0600 root: no auth, full
  power. Not group accessible on purpose - Chromium runs as `weston`, and a
  compromised browser must not be one `connect()` from the control plane.
  `tessaro-ctl` on the device uses it when no `--node` is given.
* **The same port serves the setup page** at `/`, from
  `/usr/share/tessaro-portal` ([setup-portal.md](setup-portal.md)), and
  Swagger UI at `/api/docs/`. Nothing else is served on 7400.

## Trust and auth

**A client pins the certificate instead of verifying it.** Every device is
self-signed, so there is no chain to check; clients compare its SHA-256 with
the one stored for that node id before a token is sent, and a mismatch is a
hard stop. The fingerprint is in `GET /api/v1/device/id` and in the mDNS
`fp` record. A session holds every later connection to the same
certificate (`Session::redial`). A browser - Swagger UI, the setup page -
shows its own warning for the self-signed certificate; that is accepted.

**The token is a Bearer token.** `Authorization: Bearer tsr_...` on every
request. Who may do what is the claim model in
[settings.md](settings.md): an unclaimed device answers everything without
a token, a claimed one only the endpoints with `Endpoint::PUBLIC` (who it
is, a ping, the claim). A stale token on an unclaimed device, or on a public
endpoint, is ignored rather than refused. Failed tokens are rate-limited per
address (`Limiter` in `api/mod.rs`); a valid token always gets in. Nothing
is restricted by address or subnet.

## Answers

* **Success is 200 with the endpoint's JSON**, or the bytes of a raw
  endpoint (`RAW_RESPONSE`): `screen/screenshot` answers a JPEG,
  `files/content` a piece of a file with the whole file's size and mtime in
  `x-tessaro-size` and `x-tessaro-mtime`.
* **A refusal is `{"error", "code"}`** (`ApiError`), with the status the
  code names (`ErrorCode::status`): 400 for a query or body that does not
  parse, 401 for a missing or invalid token, 404, 413, 422 for everything
  the device itself refused (`error` says why), 429 for a rate-limited
  address, 500. `code` is what a program branches on; `error` is for people.
* **Uploads are raw bodies**, one piece of at most `UPDATE_CHUNK` per
  request, each acknowledged with how much the device has, so a dropped
  link resumes (`files/upload`, `update/image`; `transfer.rs` in the client).
* **Work that takes the agent or the network down with it waits for the
  answer**: a restart, a reboot, a network re-render run as soon as hyper
  has taken the whole response (`After`, the `Body` in `api/mod.rs` that
  says so), and not later: a client that asks right after must already find
  it under way.

## No streams: jobs and pages

**Nothing streams; a client polls.** Long-lived responses would need a
second mechanism beside plain request and answer, in every client and in
the OpenAPI document; polling needs none.

* **Jobs** are the commands that run in steps: `network/ping`,
  `network/speedtest`, `storage/grow`. Starting one answers `{"job"}`;
  `GET /api/v1/jobs/{job}?after=N` answers the steps since `N`, the next `N`,
  and whether it is done and how. `DELETE` stops it. The device keeps a job's
  steps (`api/jobs.rs`) while it runs, within the command's own time limit,
  and for 5 minutes after it ends. `Session::job` polls one to its end, and
  the steps read the same in both clients (`tessaro_client::ping`,
  `speedtest`, `storage`).
* **The journal is paged**: `device/logs` answers journalctl's JSON entries
  and a cursor; asking again with it answers only what came since, which is
  how `tessaro-ctl device logs --follow` follows (`Session::logs`).

## Connections

**One request at a time per connection, kept alive.** The device closes a
connection gracefully after 10 minutes on TCP (an hour on the socket), and a
client opens a new one before a request on a connection that has been idle
for a minute or open for eight (`IDLE`, `LIFE` in `connect.rs`), so that
close is never in the middle of a request. A request that could not be sent
on a connection the device had already closed is sent once more on a new
one; one that was sent and got no answer is `Answer::Lost`, never resent -
it may have been a network change that took the connection with it.

The client's HTTP is its own (`agent/client/src/http.rs`) over the pinned
native-tls stream: no HTTP client crate takes a stream it did not open,
which the pin check needs, without an async runtime or rustls.
