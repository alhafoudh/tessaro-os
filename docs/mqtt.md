# MQTT

**With `mqtt.url` set, the device keeps a connection to an MQTT broker,
publishes what it is and what it does there, and takes every command the
HTTP API takes from there too; a client reaches it through the broker
instead of over IP.** A device behind NAT, on a cellular link or in someone
else's network can then be managed at all, a fleet can be told something
with one message, and anything that speaks MQTT - a dashboard, an
automation, a cloud - can follow and drive it without a client of ours. No
broker is set by default: nothing dials out until `mqtt.url` is.

The device's side is `agent/tessaro-agent/src/mqtt/` (the bytes under the
session in `link.rs`, keeping it in `run.rs`, a command and its answer in
`commands.rs`) and `control/mqtt.rs` (its status, test and secrets, the
page's subtree); the session itself, which does no I/O, is
`agent/mqtt/` (`tessaro-mqtt`, over mqtt-protocol-core), shared with the
clients. Where everything lives and how a request travels are
`protocol::mqtt::{topic, envelope}`. The events are `crate::events`. The
topics and every command are described in `agent/protocol/asyncapi.json`
(AsyncAPI 3.0), generated from the same types as `openapi.json`.

## Trust

**The broker is trusted like the local socket: whoever may publish to a
device's command topics has as much of the device as `mqtt.commands`
gives.** There is no token per message. A broker is where the access rules
for a fleet are kept anyway, and a token would have to sit in every
dashboard and automation that talks to it. So the broker's own rules - who
may publish to `<prefix>/<id>/cmd/#` and to `<prefix>/tag/#` - are the
device's lock.

* **`mqtt.commands` is how much the broker gets**: `full`, the default,
  every command; `read`, only the endpoints that read (`GET`); `off`,
  none - the device subscribes to no command topic and only publishes.
* **A plain link that takes every command is said in `mqtt status`**, as a
  warning: anyone who can publish to the broker, or see the link, has the
  device. Nothing refuses it; a lab broker on a trusted network is fine.
* **A command over MQTT is `Caller::Mqtt`** in the journal and in what it
  changes: `settings revision 3: browser.url changed by MQTT
  (tessaro/client/.../reply)`, the request's response topic naming who
  asked.

## The link

**One connection, client id the device id, MQTT v5, kept by one task that
reconnects with a growing pause when it drops** (2s doubling to 5 minutes;
a connection that lasted a minute starts it over). A change to the
`mqtt.*` keys reconnects at once, a new password or certificate too;
nothing else restarts.

* **TCP, then TLS for `mqtts://` and `wss://`, then a WebSocket for `ws://`
  and `wss://`**, each step with a deadline and named when it fails
  (`link.rs`). The device opens every byte itself: the WebSocket handshake
  runs over its own TLS stream, so the TLS is native-tls with the device's
  trust - the system's authorities and `network certs`' - and not a second
  TLS stack. `tessaro-ctl mqtt test` says which step stopped and prints the
  broker certificate's SHA-256.
* **`mqtt.tls.fingerprint` pins the broker's certificate** instead of
  checking its chain, for a broker with a self-signed one.
* **A client certificate** for a broker that wants one (mutual TLS) is
  `tessaro-ctl mqtt cert set --cert F --key F`, PEM.
* **Through `network.proxy.url`**: with a proxy set, the connection is a
  `CONNECT` tunnel through the device's local proxy, like every other
  connection the agent makes to the internet ([networking.md](networking.md),
  **Proxy**). A proxy that allows only 443 needs `wss://` on 443.
* **The password and the client key are secrets** in the `secrets` table,
  never settings: a password may hold any character, and `config get`
  never shows it. `{key}` placeholders in it are filled in, like
  `mqtt.username`'s.
* **The keep-alive** is `mqtt.keepalive` (60s): a ping unanswered for as
  long ends the connection, so a half-open socket does not look connected.

## Topics

**Everything of a device is under `<prefix>/<id>/`, where `<prefix>` is
`mqtt.prefix` filled in (default `tessaro`) and `<id>` its device id**, so
one wildcard, `<prefix>/+/status`, lists every device under a prefix. Only
the prefix is the operator's: the shape below it is fixed. A placeholder's
value in the prefix stays on its level - `/`, `+` and `#` become `_`, a
leading `$` too (`topic::escape`) - so a device name cannot move a device's
topics or reach another's.

| Topic | Retained | QoS | What |
| --- | --- | --- | --- |
| `status` | yes | 1 | online, name, tags, version, fingerprint, claimed, `mqtt.commands`; the will says `{"online": false}` |
| `state/device` | yes | 1 | the status, as `device status --json` |
| `state/config` | yes | 1 | the settings that are set, the proxy's password masked, and the revision |
| `state/mqtt` | yes | 1 | the link, as `mqtt status --json` |
| `event/<source>/<name>` | no | 1 | every event, `{seq, source, name, detail}` |
| `jobs/<job>` | no | 1 | a job's steps as they happen, then its end |
| `screen/frame` | no | 0 | a JPEG, every `mqtt.screenshot.interval` seconds when the picture changed |
| `logs` | no | 0 | the journal, an entry a message, from `mqtt.logs`' priority up |
| `page/out/...` | no | 1 | what the page publishes |
| `page/in/...` | no | 1 | what the page hears, as `tessaro:mqtt` |
| `cmd/<group>/<Name>` | no | 1 | a command (**Commands** below) |

The state parts are published when they change, at connect, and every
`mqtt.state.interval` seconds (60) for what moves on its own - the CPU,
the temperatures. `<prefix>/tag/<tag>/cmd/<group>/<Name>` reaches every
device with that tag.

## Commands

**A command topic names an endpoint by its group and type name -
`cmd/device/Status`, `cmd/config/Set`, `cmd/screen/Screenshot` - and the
message carries what the HTTP request would: the device runs it exactly as
the HTTP API does** (`api::Dispatch`, which both transports call), so every
endpoint of `openapi.json` is a command and a new endpoint is one with
nothing to add here.

* **A JSON endpoint's request is `{"params": {...}, "body": ...}`**, both
  optional, `params` the path's and the query's fields as one flat object.
* **A raw endpoint's request** - a piece of an upload, an update, a document
  to print - is the bytes themselves, with the params as JSON in the
  `params` user property.
* **The answer goes to the request's response topic**, with its
  correlation data, the user property `status` (`ok`, or the refusal's
  code, `not-found`, `refused`...) and `device`, the id of who answered: a
  tag reaches several. An `ok` answer is the endpoint's JSON, or a raw
  one's bytes with its headers (`x-tessaro-size`...) as user properties; a
  refusal is `{"error", "code"}`, as over HTTP. A request without a
  response topic is run and not answered.
* **A job answers `{"job"}`, and its steps come on `jobs/<job>`** as they
  happen; polling `cmd/jobs/Poll` works too.
* **At most 8 commands run at once**; more wait their turn.
* **What has to wait for the answer to be out** - restarting the agent, a
  reboot, a network change - waits until the answer is written to the
  broker, as over HTTP it waits until hyper has taken it.
* `mosquitto_rr -t <prefix>/<id>/cmd/device/Ping -e reply/me -m '{}'` is a
  command from a shell.

## A change that cuts the link

**A change to `mqtt.*` or `network.*` keys that arrives over MQTT is put
back by itself unless the link is up again 60 seconds after it**
(`commands.rs`): a wrong URL, password or network setting sent over the
broker would otherwise leave nobody able to reach the device to undo it.
The keys go back to what they were - set again, or unset - and only if
nothing changed them since (`if_revision`). The answer to the change goes
out before the link follows it. A change made over the API or locally is
not watched: someone is there to see it.

## Leaving the broker

**The device empties its retained topics when it leaves them for good**:
when `mqtt.url` is emptied, when the URL or the prefix changes, and before a
factory reset, which forgets the broker (best effort, a few seconds; a
device that is not connected then leaves them). An agent that stops or
restarts says `{"online": false}` instead, and a link that drops leaves the
broker to publish the will. A tag removed is a subscription dropped; tags
retain nothing.

**An unclaim leaves the link as it is**: the broker is configuration, not
an owner. A factory reset forgets it with the rest of the settings and
secrets; the device id survives a factory reset, so the device comes back
under the same topics once it is pointed at a broker again.

## Events

**Everything the device does or sees goes to one stream, `crate::events`,
next to what each producer did with it before** - the page's event, the
scripts that run on it, the journal - and the link publishes every one:
`cec`, `presence`, `scanner` (what the page would get), `config/changed`
(the keys and the revision, never the values), `script/started`, `browser`
(`navigated`, `restarting`, `restarted`, `new-page`, `answering`,
`not-answering`, `site-down`, `site-up`, `offline-page`, `devtools-held`,
`devtools-released`), `display/changed`, `printer` (`problem`, `ok`),
`player` (`started`, `skipped`, `nothing-playable`) and `update` (its
phases, `pending`, `cancelled`).

* **`seq` counts up from 1 since the agent started**: the device's clock
  may be wrong, so `seq` is the order, and a gap is something lost.
* **While the link is down, events wait**, up to 1000, the oldest going
  first; `mqtt status` says how many.

## Screenshots and the journal

**`mqtt.screenshot.interval` publishes what is on screen every that many
seconds, only when the picture changed**, at `mqtt.screenshot.width`
pixels across (640; 0 for the screen's own) and `mqtt.screenshot.quality`.
Chromium scales a capture only from its surface, which can hang on this
stack, so the device captures the whole page and scales it itself, from
the JPEG's DCT and an average (`jpeg.rs`); `screen screenshot --width` does
the same over any transport. A screen switched off publishes nothing.

**`mqtt.logs` publishes the journal from a priority up**, an entry a
message, looked at every 5 seconds. Off by default: on a metered link it
costs.

## The page

**The kiosk page has a subtree of its own, and only that**: it publishes
below `<prefix>/<id>/page/out/` with `tessaro.mqtt.publish(topic, payload,
{retain})` (actions mode, at most 60 messages a minute), and hears what
arrives below `page/in/` as `tessaro:mqtt` events (config mode and up), its
topic the part below `page/in/`. A page never sees the broker, the user,
the commands or another device's topics: a compromised page must not be a
way to the fleet. `mqtt.page=0` takes the subtree away. The calls are in
[bridge.md](bridge.md).

## Scripts

**A script runs on every message to its topic filters**
(`script create|set --mqtt-topics 'acme/doors/+/open'`; not `--mqtt`,
which names the broker a command goes through), `+` and `#` allowed, any
topic on the broker, while `mqtt.scripts` is on. The run gets
`TESSARO_TRIGGER=mqtt`, the topic in `TESSARO_MQTT_TOPIC` and the payload,
whatever it is, in the file `TESSARO_MQTT_PAYLOAD` names
([scripts.md](scripts.md)). The device subscribes with no-local, so what it
publishes itself never runs a script.

## Through the broker from a client

**`tessaro-ctl --mqtt URL|@NAME` and tessaro-gui's broker rows reach a
device through the broker**: the client subscribes to its own reply topic,
`<prefix>/client/<random>/reply`, and sends each request as a message to the
device's command topic (`agent/client/src/mqtt.rs`, under `Session`, so
every command, job, followed journal, upload and download works unchanged).
`--node` names the device there by name or id or the start of either,
picked from the statuses the broker keeps; `--tag` and `-n a,b` run on each
device as over IP ([clients.md](clients.md)).

* **Nothing is pinned and no token is sent**: the broker is trusted, and
  the device is the one at its id's topics.
* **What needs the device itself is refused** with the reason: `ssh
  connect`, `browser devtools`, `screen vnc`, `access webconfig`, `access
  claim` and `login`, and `device ping`, which times the connection.
* **Brokers are kept by name** in the client's store (`nodes brokers add
  NAME URL`), with a user, a password, a prefix, a CA and a client
  certificate; `--mqtt URL` takes one that is not kept, `user:password@`
  allowed, and `TESSARO_MQTT_PREFIX` for another prefix.
* Webconfig has no broker transport: a browser reaches a device at its own
  address ([webconfig.md](webconfig.md)).

## The development broker

**`mise run mqtt:run` runs mosquitto in docker with a plain, a TLS, a
WebSocket and a TLS WebSocket listener**, the TLS ones on a dev CA it makes
once in `build/mqtt/` (`test/mqtt/broker.rb`). The plain ones take anyone;
the TLS ones the user `tessaro`, password `tessaro`. `mise run
mqtt:connect` points a device at it - the qemu VM by default, which reaches
this host at `10.0.2.2` - and keeps it as `@dev` for `tessaro-ctl --mqtt
@dev`; `mise run mqtt:watch` prints every message. The e2e suite's `mqtt`
lane runs the same broker on its worker's ports ([e2e.md](e2e.md)), and
`mise run agent:integration` keeps the agent's link to a broker of its
compose file.

## What does not work

* **MQTT 3.1.1 brokers**: the requests need v5's response topic,
  correlation data and user properties.
* **QoS 2**: everything is at most QoS 1; a command a broker delivers twice
  runs twice.
* **Messages for a device that is away**: a request waits on the broker
  for a minute (its message expiry), then is dropped; the device starts a
  clean session at every connect.
