# tessaro-gui

`tessaro-gui` is the desktop client for technicians. Everything `tessaro-ctl`
does to a device, it does from windows and tables: the devices on the network
and the ones this machine knows, and per device the settings, the tools of
every command group, the live journal, a screenshot and a live VNC view. It
is built with [iced](https://iced.rs) on the workstation (`mise run gui:run`)
and is never part of the image.

## Where it lives

**`gui/` is a workspace of its own, not a member of `agent/`.** The
`tessaro-kiosk` recipe builds every member of the agent workspace and
installs every binary it finds, and every crate in `agent/Cargo.lock` has to
be in `tessaro-kiosk-crates.inc`. iced would drag its whole dependency tree
into both. So `gui/` has its own `Cargo.lock` and builds into
`build/gui-target`, and it reaches the shared code by path:

* `agent/protocol` - the wire types and the settings registry (`keys.rs`),
  so the GUI validates a value with the same `keys::validate` the agent runs
  at `set`.
* `agent/client` (`tessaro-client`) - what both clients do the same way:
  finding a device and the pinned session (`connect.rs`), `nodes.json`
  (`nodes.rs`), sending the SSH key and pinning the host key (`ssh.rs`),
  moving files and images in acknowledged chunks (`transfer.rs`), and
  reading a journal entry (`journal.rs`).

**`tessaro-client` never prints or asks.** Where the user has to decide -
pinning a certificate seen for the first time - the caller passes
`Trust::Pin` a closure. `tessaro-ctl`'s asks at the keyboard. The GUI's
accepts only the fingerprint the user already accepted in a dialog. What is
worth telling the user comes back as `Session::notes`, and a transfer's
progress goes to a callback.

## One window, inner windows

**The app is one window with inner windows on a desk, WinBox style**
(`mdi.rs`). The node list and every opened device are inner windows: they
drag by the title bar, resize from the bottom-right corner, maximize with the
title-bar button or a double-click, and close with ×. The node list cannot be
closed; Devices in the app header brings it back to the top. The last window
clicked is on top.

* Built from iced's `stack` (one layer per window, in z-order), `pin` (its
  position) and `opaque`, so a window hides what is under it from the mouse
  as well as the eye.
* While a window is dragged, the app listens for the cursor itself
  (`event::listen_with`, only then). The title bar only reports where on it
  the press was.

**The keyboard talks to the window on top** (`main.rs`, `keys`):

* Esc closes its dialog, else closes a device window. It never closes the
  node list or the app.
* Enter presses the dialog's default button, else opens or edits the
  selected row.
* Up and Down move the selection in the page's main table.
* Cmd + / Cmd - / Cmd 0 (Ctrl elsewhere) zoom every window, in tenths from
  0.6 to 2.0, and the zoom is kept in `gui.json` next to `nodes.json`.

A key a widget took - Esc leaving a text field, Enter submitting one - is
left to it; the zoom always works.

**One dark look** (`theme.rs`). There is no light variant, and it does not
follow the system. Every colour and style is a named constant or function
there, so windows cannot drift apart.

## The node list

**One list: the known nodes from `nodes.json`, overlaid with what mDNS sees,
keyed by node id** (`nodes_view::merge`).

* A known device that answers takes its live address and claimed state.
* One that presents another certificate than its pin is marked `MISMATCH`
  and cannot be opened: it was reinstalled, its `/data` was wiped, or someone
  is in the middle. Forget it to pin it again.
* An unknown device gets a row of its own.
* **An unclaimed device opens without a claim, a login or a pin**
  (`NodesView::openable`): it answers everything without a token (see **The
  claim model** in [settings.md](settings.md)). A stranger is reached at the
  address it was seen at (`worker::connect`), and only while it stays
  unclaimed. Actions that make a credential show the device's refusal;
  SSH and VNC go in by the empty root password instead of a key.

Discovery runs for as long as the app does (`discovery.rs`). Rescan starts a
new browse. A device mDNS cannot see (another subnet, a VM) is added by
address.

**Login and claim peek first, then pin exactly what was shown.** The dialog
opens a session with `Trust::Peek`, which sends nothing secret, and shows the
device's name, id and certificate fingerprint. Going on opens a second
session whose pin closure accepts only that fingerprint. If another
certificate answers by then, nothing is sent. A login proves the token with
`TokenList` before storing it. A claim shows the root and hotspot passwords
once, with copy buttons, and closes only through Done.

## Device windows

**Each opened device has a worker thread that owns its blocking `Session`**
(`worker.rs`). The window's subscription starts it; closing the window drops
the subscription, and with it the thread and the connection.

* It polls `Status` every 2s, and every second while a guarded change waits
  for confirmation, so the countdown moves.
* It fetches the settings again only when `Status.revision` moved.
* A lost connection is retried with a growing pause. The node is opened by
  name (last address first, then mDNS), held to its pin, and `nodes.json` is
  read again each time, so a device that moved is found and remembered.
* Pages ask through one generic call, `Request::Call`: any `Command`,
  answered as `Event::Answer` with a tag naming the page that asked.

**The nav lists Overview, then the settings sections, then the tools.**

* The settings sections are the key prefixes the device reports, in its
  order (`device::sections`), so a group a newer image adds appears here by
  itself.
* The tools are one page per `tessaro-ctl` command group (`device/pages.rs`).

**Every page is drawn the same way** (`section.rs`, `grid.rs`): a toolbar
with the actions on the page first and those on the selected row after them,
then its tables. Anything that is a list is a table.

**Settings are edited in a dialog, WinBox style: OK applies and closes, Apply
applies and stays, Default unsets.**

* The input follows the key's kind in the registry: a checkbox for a flag, a
  list for a choice, otherwise text.
* The value is checked live with `keys::validate`.
* Every change is sent with the revision the table was read at
  (`if_revision`), so a change against stale values is refused and the
  settings are fetched again.
* What a change did goes to Messages: the revision, restarted units, network
  checks and the audio and time outcomes.

**What the tool pages cover, by command group:**

| Page | Covers |
| --- | --- |
| Overview | `device status` and `id`, systemd units, `device ping`, `device factory-reset`, `browser navigate`, `browser maintenance`, `browser debug`, `browser zoom` |
| Screen | `screen modes` with "use this mode", `screen confirm`, `screen screenshot` with a 3s live refresh and Save |
| Network | `network show` and interfaces, `network last`, `network ping`, `network speedtest`, `network profiles list` and `show` |
| WiFi | `network wifi status`, `scan`, `join`, `hotspot-password` |
| Storage | `storage show`, partitions and filesystems, `storage grow` (check first) |
| Audio | `audio show`, outputs and inputs, choosing one, volume, mute, `audio test` for the tone and the recording |
| Time | `time show` and its servers, `time timezone` (a choice of `time zones`), `time ntp on|off` with servers, `time sync`, `time set` (this computer's clock or a typed time) |
| Access | `access token create`, `list`, `revoke`, `access password set`, `access unclaim` |
| SSH | `ssh keys list` and `revoke`, `ssh connect` (authorize the key, open a terminal) |
| Files | `files list` as a browser, `upload` (files or a folder), `download`, `mkdir`, `move`, `rm` |
| Update | `update status`, `update send` with progress, `update cancel` |
| Log | `device logs --follow`, filtered by unit on the device and by text here |

The window's own toolbar has Restart browser, weston or agent, and Reboot,
each confirmed first. The status bar shows the device's `Status`, and a
guarded change's countdown with Confirm.

**Dialogs are one generic form** (`pages::Form`), confirmed with Enter. A
destructive one - factory reset, unclaim, growing `/data`, an update that
erases `/data` or rewrites the disk - wants the device's name typed first,
as `tessaro-ctl` does. What the device shows once (a token, a password, the
hotspot password) comes in a dialog with Copy that closes only through Done.
After unclaim or a factory reset, the node is forgotten on this machine.

## Work on connections of its own

**Long work runs as a job on a second connection, so the worker keeps
polling** (`jobs.rs`). The jobs are the streams (`network ping`, the speed
test, `storage grow`), `device ping`, files going up or down, and an image
update. A job is a subscription keyed by its id: it reports progress, lines
and a result to its page. Cancel drops it, and a watcher thread shuts its
socket down, which ends whatever call it was in.

**The live journal is the same, on its own connection** (`logs.rs`), open
while the Log page is shown and Live is on. A new unit filter is a new
stream. The last 5,000 entries are kept, the newest 500 matching the filter
are drawn, and Pause freezes the table while entries keep arriving.

The transfers are `tessaro_client::transfer`, the same code as `tessaro-ctl
files` and `tessaro-ctl update send`, so a dropped upload resumes where the
device says it got to. Files and images are chosen with the system's own
dialogs (`rfd`).

## VNC

**The VNC panel shows the device's screen live, view only, beside any page**
(`vnc.rs`, the VNC toggle).

* The device's server listens on its loopback only (see
  [remote-access.md](remote-access.md)). So the panel sends the SSH key and
  pins the host key over the control connection (`tessaro_client::ssh`, as
  `ssh connect` does), then runs the system's `ssh -N -L` from a free local
  port to `127.0.0.1:5900`. An unclaimed device gets no key and ssh gets in
  by its empty password (**SSH keys** in remote-access.md), so the panel
  works before a claim too.
* The server is neatvnc, which takes VeNCrypt with a plain login inside TLS
  and nothing else. No Rust VNC crate speaks that, so `vnc.rs` is a small RFB
  3.8 client: VeNCrypt X509Plain (or TLSPlain), the image's `tessaro` login,
  then Raw and CopyRect updates into a framebuffer. The picture is handed to
  the UI at most every 150ms.
* It is view only because remote input never reaches the browser (the second
  seat, in remote-access.md).
* `screen.vnc=off` shows a note instead of a picture.

## Adding a page

1. Add the page to `Page` and `Page::TOOLS` in `device.rs`.
2. Ask for its data in `refresh_page` with a tag, and keep the answer in
   `take_answer`.
3. Draw it with `page`, `table` and `facts` in `device/pages.rs`, with its
   actions as `section::Action`s.
4. Put long work in a `jobs::Kind`, and give its events a line in
   `stream_line`.

It then looks and behaves like the other pages: selection, double-click and
Enter, Up and Down, disabled actions, dialogs.
