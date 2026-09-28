# tessaro-gui

`tessaro-gui` is the desktop client for technicians. Everything `tessaro-ctl`
does to a device, it does from windows and tables: the devices on the network
and the ones this machine knows, and per device the settings, the tools of
every command group, the live journal, a screenshot and a live VNC view. It
is built with [iced](https://iced.rs) on the workstation (`mise run gui:run`)
and is never part of the image.

**On macOS, `mise run gui:build` also creates
`build/gui-target/release/Tessaro.app`.** Open that bundle in Finder or copy
it to Applications. `mise run gui:run` builds and runs a debug bundle with
the same Dock icon while keeping terminal output. Both use
`gui/package-macos.sh`, with the version from `gui/Cargo.toml`, and an ad-hoc
signature for local use; distribution signing and notarization are separate.
`cargo run` alone runs the bare executable, without the bundle's icon.

The app icon's editable source, transparent 1024px PNG and multi-resolution
ICNS are in `gui/tessaro-gui/icons/`. After editing `tessaro.svg`, export it
as a transparent 1024 × 1024 `tessaro.png`, then run
`sh gui/tessaro-gui/icons/generate-macos.sh` on macOS to regenerate
`Tessaro.icns`. Normal builds use the checked-in ICNS. The welcome page
(`tessaro-selftest/files/index.html`) inlines the same mark's paths, so a
change to the mark goes into both.

## Where it lives

**`gui/` is a workspace of its own, not a member of `agent/`.** The
`tessaro-kiosk` recipe builds every member of the agent workspace and
installs every binary it finds, and every crate in `agent/Cargo.lock` has to
be in `tessaro-kiosk-crates.inc`. iced would drag its whole dependency tree
into both. So `gui/` has its own `Cargo.lock` and builds into
`build/gui-target`, and it reaches the shared code by path:

* `agent/protocol` - the API's endpoint types ([api.md](api.md)) and the
  settings registry (`keys.rs`),
  so the GUI validates a value with the same `keys::validate` the agent runs
  at `set`.
* `agent/client` (`tessaro-client`) - everything both clients do: the
  pinned session, the flows (update, file trees, ping, grow, DevTools), how
  a request is built and what an answer says in words. What is in it, and
  how it reports without printing, is in [clients.md](clients.md).

**A page shows the client's words, not its own.** A page draws what
`tessaro_client::describe` and the flows hand back: `Fact`s in a facts
table (`shared_facts`), `Line`s in the output pane and the log, colored by
their tones (`theme::text_line`, `theme::toned`).

## One window, inner windows

**The app is one window: the node list fills its desk, and every opened
device and its settings windows float over it as inner windows, WinBox
style** (`mdi.rs`). The node list is the main screen, not a window, so it
cannot be closed or lost behind anything but the windows opened from it.
The inner windows drag by the title bar, resize from the bottom-right
corner, maximize with the title-bar button or a double-click, and close
with ×. The last window clicked is on top. Title bars are dark; the window
with the keyboard has the lighter one and a purple border.

* Built from iced's `stack` (the node list, then one layer per window, in
  z-order), `pin` (its position) and `opaque`, so a window hides what is
  under it from the mouse as well as the eye.
* **A window is always kept whole on the desk** - moved, resized, opened or
  when the app window shrinks. `pin` gives its content only what is left of
  the desk right of and below its position, so a window past the edge would
  be squeezed while its stored size kept growing, and the corner would stop
  answering.
* While a window is dragged, the app listens for the cursor itself
  (`event::listen_with`, only then). The title bar only reports where on it
  the press was.
* **Where a window was is remembered per kind, not per device** (`DEVICE`,
  `SETTINGS` in `main.rs`), in `gui.json` next to `nodes.json`, after every
  move, resize and maximize. The next window of that kind opens there, cascaded
  off any of its kind already at that spot. A maximized window is
  remembered as maximized, next to the size it restores to, not as the
  desk's size. Whether device windows show their Messages log is kept
  there too, as the last window toggled it.
* **The app window reopens where it was left** (`Prefs::window` in
  `main.rs`): position, size and whether it was maximized, again with the
  size it restores to. It is kept in the screen's points, unzoomed, because
  iced scales a new window's size by the zoom and reports sizes and
  positions divided by it. The OS reports every step of a drag or resize,
  so the latest geometry is only held (`Moving`) and written once the
  window has been still for `SETTLE`; only then is the window asked
  whether it is maximized (`window::is_maximized`), and a maximized size
  never overwrites the restored one. The check runs on a ticker thread
  that lives only while something is unwritten: iced's pool executor has
  no timer.
* **On macOS the app header is the app window's title bar** (`main.rs`):
  the native title is hidden and the title bar transparent over a
  full-size content view, so only the traffic lights are left, over the
  header's left end. The header's title starts `TRAFFIC_LIGHTS` points in,
  divided by the zoom because the zoom scales the header but not the
  traffic lights. The content view takes the title bar's clicks, so the
  header drags the window itself (`window::drag`) and maximizes it on a
  double-click. Other platforms keep their native title bar.
* **The title-bar icons are drawn, not typed** (`icon.rs`): a `□` from a
  fallback font lands on fractional device pixels at most zooms, so some
  of its edges come out half as thin. The canvas snaps each edge to a whole
  device pixel from the screen's scale times the zoom (`Desk::set_pixel`).

**The keyboard talks to the window on top** (`main.rs`, `keys`), or to the
node list when no window is open or the list was clicked last:

* Esc closes its dialog, else closes a device or settings window. It never
  closes the node list or the app.
* Enter presses the dialog's default button, else opens or edits the
  selected row.
* Up and Down move the selection in the page's main table, or in a
  settings window's table.
* Cmd + / Cmd - / Cmd 0 (Ctrl elsewhere) zoom every window, in tenths from
  0.6 to 2.0, and the zoom is kept in `gui.json` next to `nodes.json`.

A key a widget took - Esc leaving a text field, Enter submitting one - is
left to it; the zoom always works.

**One dark look, in the welcome page's colours** (`theme.rs`), so the
client and the kiosk's own screen read as one product. The page's CSS
variables in `tessaro-selftest/files/index.html` are the source: its
near-black background is the desk, its blue and purple glows are the
chrome and the selection, and its accents and status colours are the
GUI's. The page's translucent cards and lines become solid colours, since
iced draws no gradients or blending behind them. There is no light
variant and no following the system theme. Every colour and style is a
named constant or function there, so windows cannot drift apart. The UI font is Manrope, bundled in
`gui/tessaro-gui/fonts/` (SIL OFL, `OFL.txt` beside it) as static Regular
and Bold files, so the GUI reads the same on every OS; iced alone would take
whatever sans the host has. Monospace text (`Font::MONOSPACE`) is left to
the host.

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
  unclaimed. Once it answers, the worker writes it to `nodes.json`, pinned
  to that session's certificate with no token, so it stays in the list and
  later sessions are held to that pin. Actions that make a credential show the device's refusal;
  SSH and VNC go in by the empty root password instead of a key. Unclaim is
  disabled instead: the device does not refuse it, and the node would be
  forgotten here for nothing. The claim state comes from each status poll.

Discovery runs for as long as the app does (`discovery.rs`). Rescan starts a
new browse. A device mDNS cannot see (another subnet, a VM) is added by
address. Both are actions on the node list's own toolbar, since they act on
that list; the app header only has the app's name.

**A device added by address is written to `nodes.json` at once, claimed or
not** (`NodesView::keep`), so it is listed after a restart although mDNS never
announces it. A new one is pinned to the certificate the add just saw, with no
token; a known one only takes the new address, and one that presents another
certificate than its pin is not touched. After a restart its claimed state is
unknown until it answers, so opening it goes through the login dialog's peek,
and a device that turns out unclaimed opens straight from there.

**Login and claim peek first, then pin exactly what was shown.** The dialog
opens a session with `Trust::Peek`, which sends nothing secret, and shows the
device's name, id and certificate fingerprint. Going on opens a second
session whose pin closure accepts only that fingerprint. If another
certificate answers by then, nothing is sent. A login proves the token with
`TokenList` before storing it. A claim shows the root and hotspot passwords
once, with copy buttons, and closes only through Done.

**A claim from the device window's Access page goes over the session the
window already has** (`worker::Request::Claim`), so it pins the certificate
that session was opened on, which the form and the Overview show. The worker
keeps the new token for its later calls and writes the node to nodes.json,
and the node list reloads.

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
* Pages ask through one generic call, `Request::Call`: any endpoint, built
  typed by `worker::call`, `fetch` or `send` and run on the worker's session,
  answered as `Event::Answer` with a tag naming the page that asked.

**The nav has one entry per subject; a page shows its tools, and its
settings open in a window of their own** from Configure, the first button
on its toolbar. So one subject is never in two places, and the settings
table keeps its full width.

* The pages are Overview and one per `tessaro-ctl` command group
  (`device/pages.rs`). Configure opens the keys of the page's prefix
  (`Page::scope`): Overview the `device.*` keys, WiFi the `network.wifi.*`
  keys, which Network leaves out. A page whose prefix the device has no
  keys for has no Configure.
* A prefix no page shows gets an entry after the pages, in the device's
  order (`device::own_sections`), and clicking it opens its settings window,
  so a group a newer image adds appears by itself. Data is always listed,
  empty or not, so the first custom value can be added there; unsetting
  one is its Delete.
* A settings window belongs to its device window (`main.rs`, `configs`):
  one per group, Configure again raises it, and it closes with its device.
  It keeps its own selection and filter, and the edit dialog opens inside
  it; what a change did still goes to the device window's Messages.
* **Each action is on one page only: the page of its command group.**
  Overview is the `device` group and nothing else, so it stays a summary; a
  `browser` command goes on Browser, not on Overview.

**Every page is drawn the same way** (`section.rs`, `grid.rs`): a toolbar
with the actions on the page first and those on the selected row after them,
then its tables. Anything that is a list is a table.

Tables use `iced_table2`: drag a header divider to resize a column, and
scroll horizontally when the columns exceed the window width. Dividers are
always visible in the header; body rows have a continuous background. Click a
column title to cycle ascending, descending, then original order; an arrow
marks the active sort. Text sorts case-insensitively with numbers in natural
order, and formatted sizes use their underlying byte counts. Selection,
double-click actions and keyboard navigation follow the displayed rows.

Column widths and sorting are saved in `gui.json`, separately for the node
list and each device's tables (including settings and the journal), and
restored when the app or window opens again. A resize is saved when the drag
ends. The journal follows new entries in its original order; choosing a sort
turns off auto-follow until the original order is restored.

**Right-clicking a table cell selects its row and offers Copy of the cell's
text as shown** (`copy_menu.rs`, wrapped around every cell in `grid.rs`).
The text is read back from the cell's widgets through `operate`, so no page
passes it in, and the clipboard is written by the widget itself. A masked or
shortened value is copied as displayed; the full one keeps its own Copy
button.

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
| Overview | `device status` and `id`, systemd units, `device ping`, `device factory-reset` |
| Screen | `screen modes` with "use this mode", `screen screenshot` with a 3s live refresh and Save, `screen power`, `screen keyboard` |
| Browser | what the browser shows, `browser navigate`, `reload`, `clear-cache`, `maintenance`, `debug`, `zoom`, `devtools` (a job holding the tunnel until Cancel), `inject`, `bridge`, `eval` (results in the page's output) |
| Network | `network show` and interfaces, `network last`, `network ping`, `network speedtest` (with "Bypass the proxy"), `network proxy set`, `off` and `test`, `network profiles list` and `show` |
| WiFi | `network wifi status`, `scan`, `join`, `hotspot-password` |
| Certificates | `network certs list`, `add` (a file picker) and `revoke` |
| Storage | `storage show`, partitions and filesystems, `storage grow` (check first) |
| Audio | `audio show`, outputs and inputs, choosing one, volume, mute, `audio test` for the tone and the recording |
| Time | `time show` and its servers, `time timezone` (a choice of `time zones`), `time ntp on|off` with servers, `time sync`, `time set` (this computer's clock or a typed time) |
| Schedules | `schedule list`, `create` and `set` in one dialog (multi-line calendar and commands, checked with `schedule check` as you type), `enable`/`disable`, `run`, `remove`, `logs` (the Log page, filtered to the schedule's runs) |
| Access | `access claim` (while unclaimed), `access token create`, `list`, `revoke`, `access password set`, `access unclaim` (while claimed), `access webconfig` (Open Webconfig) |
| SSH | `ssh keys list` and `revoke`, `ssh connect` (authorize the key, open a terminal) |
| Files | `files list` as a browser, `upload` (files or a folder), `download`, `mkdir`, `move`, `rm` |
| Update | `update status`, `update send` with progress, `update cancel` |
| Log | `device logs --follow`, filtered by unit on the device and by text here |

The window's title bar, after the device's name and address, has Refresh,
Restart browser, weston or agent, and Reboot, the restarts confirmed first,
and the VNC and Messages toggles. There is no toolbar row under it. **Refresh is the window's, not a
page's**: it fetches `Status`, the settings and what the page shown asks
for, so no page or settings window has a Refresh of its own. The status bar
shows the device's `Status`, and a guarded change's countdown on every page.
**The one Confirm is a green button in the Screen page's toolbar**, shown
only while a change waits, with the seconds left in its label: every guarded
key is a `screen.*` one, whether it came from "Use this mode" or from a
setting.

**Dialogs are one generic form** (`pages::Form`), confirmed with Enter. A
destructive one - factory reset, unclaim, growing `/data`, an update that
erases `/data` or rewrites the disk - wants the device's name typed first,
as `tessaro-ctl` does. What the device shows once (a token, a password, the
hotspot password) comes in a dialog with Copy that closes only through Done.
After unclaim or a factory reset, the node is forgotten on this machine.
A multi-line field (a schedule's calendar and commands, one per line) takes
Enter as a new line, so its form is confirmed with the button. The schedule
form stays open until the device takes it: a refused save shows the
device's reason in it, and under the fields it shows how the device's
systemd reads the calendar and when it fires next, asked again once typing
pauses.

## Work on connections of its own

**Long work runs as a job on a second connection, so the worker keeps
polling** (`jobs.rs`). The jobs are the device's own jobs (`network ping`,
the speed test, `storage grow`, polled with `Session::job`), `device ping`,
files going up or down, an image update, and the DevTools tunnel
(localhost:9222, or a free port when 9222 is taken here). Each is the
client's flow (`update::send`, `files::upload`, `ping::device`, ...), with
the job as its `Report`. A job is a subscription keyed by its id: it
reports progress, lines and a result to its page. Cancel drops it: a device
job is cancelled on the device at the next poll, a flow stops at its next
step (`Report::stopped`), and a watcher thread shuts the socket down, which
ends whatever call it was in. An update that reboots the device waits for
it to come back, as `tessaro-ctl update send` does; one that erases `/data`
forgets the node here.

**The live journal is the same, on its own connection** (`logs.rs`), open
while the Log page is shown and Live is on, polling the journal's pages
(`Session::logs`). A new unit filter starts it again. A lost connection is
followed again from the last entry's `__CURSOR`, so nothing shows twice.
The last 5,000 entries are kept, the newest 500 matching the filter
are drawn, and Pause freezes the table while entries keep arriving.

The transfers are `tessaro_client::files` and `update`, the same code as
`tessaro-ctl files` and `tessaro-ctl update send`, so a dropped upload
resumes where the device says it got to, and a symlink is skipped, never
followed. Files and images are chosen with the system's own dialogs
(`rfd`).

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
* **A frame is shown only once iced has it on the GPU** (`image::allocate`,
  `vnc_uploaded` in `device.rs`). iced_wgpu uploads an image of 2 MiB or more
  off-thread and draws nothing for that handle until it is done
  (`MAX_SYNC_SIZE` in `iced_wgpu/src/image/cache.rs`), and a whole screen is
  far past that, so putting each new handle in the view straight away blanks
  the panel on every frame. The shown frame keeps its `Allocation`; while
  one upload runs, only the newest frame waits.
* It is view only because remote input never reaches the browser (the second
  seat, in remote-access.md).
* `screen.vnc=off` shows a note instead of a picture.

## Adding a page

1. Add the page to `Page` and `Page::TOOLS` in `device.rs`, and give it a
   `Page::scope` if settings belong to it (its Configure).
2. Ask for its data in `refresh_page` with a tag, and keep the answer in
   `take_answer`.
3. Draw it with `page`, `table` and `shared_facts` in `device/pages.rs`,
   with its actions as `section::Action`s. Its words come from
   `tessaro_client::describe`, written there if they are new
   (**Adding a command** in [clients.md](clients.md)).
4. Put long work in a `jobs::Kind` that runs the client's flow, and give a
   device job's events a line in `stream_line`, from the client.

It then looks and behaves like the other pages: selection, double-click and
Enter, Up and Down, disabled actions, dialogs.
