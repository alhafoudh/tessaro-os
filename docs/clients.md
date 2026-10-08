# The two clients

`tessaro-ctl` and `tessaro-gui` manage the same devices through the same API,
so they do the same things. How they share that, and what each keeps for
itself, is here. How the GUI draws is in [gui.md](gui.md); the API itself in
[api.md](api.md).

## What lives where

**Everything both clients do is in `agent/client` (`tessaro-client`); a
client binary only reads input, draws, and decides.** Two copies of a flow
drift apart unnoticed - an upload that follows a symlink loop, a WiFi join
that cannot rejoin a known network, a form that rounds a timeout off - so
there is one copy, and both clients call it.

* `agent/protocol` - what is on the wire: the endpoint types (`api.rs`) and
  the settings registry (`keys.rs`).
* `agent/client` - talking to a device and everything a command does
  around its requests:
  * the connection: discovery and the pinned session (`connect.rs`),
    the known nodes in the client's `tessaro.db` (`nodes.rs`, `store.rs`),
    claim and login (`access.rs`), a device's tags with the reserved
    `unclaimed`, the tag filter and a tag's badge colour (`tags.rs`),
    the devices a run on several stands for and running it (`bulk.rs`);
  * flows that take several requests: an image update (`update.rs`), whole
    trees to and from the file store and `files sync` (`files.rs`), device
    ping (`ping.rs`), growing `/data` (`storage.rs`), the DevTools forward
    (`devtools.rs`), the VNC mirror started, forwarded and kept going
    (`vnc.rs`), SSH keys (`ssh.rs`, `tunnel.rs`), chunked transfers
    (`transfer.rs`);
  * what a request is built from: network changes, the WiFi password, the
    proxy URL (`network.rs`), the clock and NTP, the root password
    (`actions.rs`), timeouts (`schedule.rs`), a script from its typed
    fields (`script.rs`);
  * what an answer says, in words: `describe/` per subject, and the job
    steps of `ping.rs`, `speedtest.rs`, `storage.rs` and `script.rs`.
* `agent/tessaro-ctl` - clap, `--json`, prompts at the keyboard, painting
  the text for a terminal (`style.rs`, `progress.rs`) and where it goes
  (`out.rs`).
* `gui/tessaro-gui` - iced: pages, tables, forms and dialogs, jobs on their
  own connections, painting the text with the theme (`theme.rs` in
  `gui/tessaro-style`).
* `gui/try-tessaro` - the Mac app that runs a device in a VM
  ([try-tessaro.md](try-tessaro.md)). Not a third client of the commands:
  its sample activities call `agent/client` like the other two, and say
  the `tessaro-ctl` command that does the same.

**A device is named the same way everywhere, by `connect::resolve`.** A
bare name no known device has may be the start of the name or id of one
device, and stands for it, the way `docker` takes the start of a container
id. The devices it is matched against are the known ones and those an mDNS
scan finds, together, with a device that is both counted once by its id
(`connect::pick`), so an unclaimed device just switched on is reached by
the start of its name too. The start of several is an error that lists
them, and an exact name always wins (`lobby` beside `lobby-2`). Such a
name always costs a scan; an exact known name goes to its last address
first. `NAME.local` is always a name to look for on the network.

## Nothing in the client prints or asks

**A shared function hands back text and takes decisions as arguments; it
never writes to a terminal or opens a dialog.** Both clients link it, and a
`println!` in it would land nowhere in the GUI.

* **A decision the user makes is passed in.** Pinning a certificate seen for
  the first time is a closure in `Trust::Pin`: the ctl's asks at the
  keyboard, the GUI's accepts the fingerprint already accepted in a dialog.
* **A decision the caller makes is handed back.** `update::send` answers
  `Sent::Wiped` when `/data` goes, and the caller forgets the node in its
  known nodes; `update::Plan::warning` says what is lost, and each
  client confirms it its own way (the ctl's `-y` or typing the name, the
  GUI's form that wants the name typed).
* **Something worth telling the user comes back.** `Session::notes` after
  opening a session, `text::Line`s from a flow or a description.
* **Progress goes to a `report::Report`.** A flow calls `progress` while a
  step runs and `line` when one is over, and checks `stopped` between steps.
  The ctl's is its progress line on stderr (`progress.rs`), or stdout for a
  ping's replies; the GUI's sends the job's events to its page, and its
  `stopped` is the job's Cancel (`jobs.rs`).

## Text: tones, lines and facts

**Shared text is spans with a tone, `text::Line`, not strings.** A `Tone`
says what a piece of text is - a label, a healthy value, a warning, a
command to run - never its color. The ctl maps each tone onto its palette
(`style::of`, `style::line`), so its output keeps the colors it had; the GUI
colors each span with its theme (`theme::text_line`), or a whole table cell
by the line's loudest tone (`theme::toned`). A plain string would lose one
or the other.

* A `Line` reads as its plain text (`Display`): that is what the ctl prints
  with colors off, what the GUI copies, and what the tests compare.
* A span can be padded to a column, and the ctl pads inside the color
  codes, since `{:<N}` around painted text counts the escape bytes.
* A `text::Fact` is a label and a value: a row of `device status`, a row of
  a GUI facts table. The GUI capitalizes the label.
* Thresholds live with the tones (`text::usage_level`, `unit_state`,
  `link_state`, `net::signal_tone`), so a disk is as full in both clients.

The labels and words are the ctl's: it is scripted against and documented,
so its text is the one to keep. A GUI page may still lay the same facts out
as it likes - a table instead of rows, a dialog instead of a line.

## Adding a command

1. The request: an endpoint type in `agent/protocol/src/api.rs` (see
   [api.md](api.md)).
2. The logic both need, in `agent/client`: building the request from what
   the user typed, a flow of several requests reporting to a `Report`, and
   what the answer says as `Line`s or `Fact`s (`describe/`). Tests go
   there, next to it.
3. The ctl command: its clap arguments, a call into the client, `--json`,
   and printing the lines with `style::line`. A new group also goes into
   its section in `sections::GROUPS` (`sections.rs`), which the root
   `--help` lists the groups by; a test in `main.rs` fails on a group left
   out.
4. Its place in the GUI in the same change (the table in
   [gui.md](gui.md)): a form or button that calls the same client function,
   showing the same lines.
5. Its place in Webconfig in the same change, on its group's page
   (`webconfig/src/pages/`), unless it is native-only. Webconfig cannot link
   `agent/client`, so its words are a port pinned by golden fixtures
   (**The pages** in [webconfig.md](webconfig.md)): a new describe function
   gets a fixture there too.

**Opening Webconfig is shared too** (`webconfig.rs`): the address this
session reached the device at, with a one-time ticket in its fragment on a
claimed device, and the host's way of opening a URL; `tessaro-ctl access
webconfig` and the GUI's Access page both call it.

A check that only one client needs - a GUI form's field being empty - stays
in that client. A second copy of anything in step 2 is the thing to avoid.

## Running on several devices

**One command on several devices is the client's work, not the device's:
`bulk.rs` picks the devices and runs on each, and nothing on a device or in
the API knows about it.** A tag is only what finds the devices.

* **Picking them** (`bulk::select`): every name of a comma-separated
  `--node`, each resolved as `connect::resolve` does, and every device with
  all of the `--tag`s, known or found on the network. At most one scan is
  made, and only when a tag is given or a name is not a known device's; its
  results are shared by every name and tag. A device the scan found is
  reached where it announced itself, held to its pin when known, so no
  device scans again when its session opens. `unclaimed` only comes from the
  scan, as in `nodes list --tag`: the known nodes' cached tags never carry
  it. A name that stands for nothing, or no device at all, fails before
  anything runs.
* **Running** (`bulk::each`): a thread and a session per device, at most
  `--parallel` (`bulk::PARALLEL` by default) at once, the outcomes in the
  order the devices were picked. `describe::bulk` has the words: the
  heading over a device's output, the device list a confirmation shows, the
  summary of who failed. They are native-only, so they have no golden
  fixture.
* **The ctl** runs the parsed command on each device through the same code
  as for one (`run_on` in `main.rs`), with its output kept apart per thread
  (`out.rs`: every module prints through its `println!` and friends, which
  write to the thread's buffer during a run on several), and prints each
  device's output whole, one after the other, then the summary. With
  `--json` it prints one array, a `{name, id, address, ok, result, error}`
  per device, `result` being what the command printed as JSON. Any device
  failing fails the run.
* **What the ctl refuses on several devices** (`bulk_refused`): anything
  that holds the terminal for one device (`ssh connect`, `browser
  devtools`, `screen vnc`, `device logs -f`, `screen cec messages -f`, `browser policies
  edit`, a watched `camera snapshot`), opens something on this machine (`access webconfig`), writes
  one local file every device would overwrite (`files download`, a named
  screenshot or snapshot), or reads stdin, which there is one of. A prompt
  that is reached anyway fails that device (`prompt::keyboard`).
* **Asking first**: a command with `-y` lists the devices, and runs on them
  only with `-y`; without it the run stops before any device is touched.
* **The GUI** marks rows in the node list and runs one action on them in a
  bulk window (**The node list** and **The bulk window** in
  [gui.md](gui.md)), each device's run a job like a device window's.

## What differs on purpose

**Some things are each client's own, because the medium differs.** Keep
them apart; do not pull them into the client.

* How to confirm: the ctl asks y/N or wants the device's name typed, and
  takes `-y`; the GUI's forms want the name typed. On several devices the
  ctl wants `-y`, and the GUI's bulk window wants the number of devices
  typed where one device's name would be.
* The GUI asks before a restart or a reboot; the ctl, a typed command, does
  not.
* The GUI browses for devices for good and streams what it finds
  (`discovery.rs`); the ctl's `connect::browse` is one pass.
* A network change's `--verify` and `--no-apply` are the ctl's only; the
  GUI sends the defaults.
* `--json` is the ctl's: the device's answer as it came.
