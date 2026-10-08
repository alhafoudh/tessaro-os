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

**`gui:run` launches the bundle with `open`, not by running its executable**,
so the window comes to the front. A process started from the terminal is
the terminal's child, and macOS does not let it take the front: winit asks
with `activateIgnoringOtherApps`, which the system no longer honours, so
the window stays behind the terminal until the Dock icon is clicked.
`open -n -W` starts that build even when another is running and waits for
it, `--stdout`/`--stderr` send its output to the terminal, and Ctrl-C quits
it. The environment of the shell does not reach the app through `open`.

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
  pinned session, the flows (update, file trees, ping, grow, DevTools, VNC), how
  a request is built and what an answer says in words. What is in it, and
  how it reports without printing, is in [clients.md](clients.md).

**A page shows the client's words, not its own.** A page draws what
`tessaro_client::describe` and the flows hand back: `Fact`s in a facts
table (`shared_facts`), `Line`s in the device window's Messages, colored by
their tones (`theme::text_line`, `theme::toned`, and the highlighter in
`messages.rs`).

## One window, inner windows

**The app is one window: the node list fills its desk, and every opened
device, its settings windows, its CEC console and the bulk windows float over it as inner
windows, WinBox style** (`mdi.rs`). The node list is the main screen, not a window, so it
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
  `SETTINGS`, `CONSOLE` in `main.rs`), in the `gui_prefs` table of the client's
  `tessaro.db` next to the nodes (see **The node list**), after every
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
  settings window's table. In the node list Shift-Up and Shift-Down mark
  the rows they pass, and Cmd-A (outside a text field) marks every row
  shown (**The node list**).
* Cmd + / Cmd - / Cmd 0 (Ctrl elsewhere) zoom every window, in tenths from
  0.6 to 2.0, and the zoom is kept in `gui_prefs`.

A key a widget took - Esc leaving a text field, Enter submitting one - is
left to it; the zoom always works. The exception is Esc in a dialog's
field, which still closes the dialog.

**A dialog opens with the cursor in its first field to type in**
(`dialog::Fields`), as Webconfig's do, so typing needs no click first. The
focus is asked for once, as the dialog opens (`Device::open`, or when a
login or claim dialog's field arrives with the device's answer), so a field
clicked into afterwards keeps it. A dialog with only boxes, choices or
buttons focuses nothing. Each window numbers its own fields, because every
inner window shares one widget tree, where the same id twice would focus
both.

**Tab and Shift-Tab move the cursor between a dialog's fields**, round at
the ends (`Fields::step`). No iced field takes Tab, so it reaches `keys` in
`main.rs` and goes to the window on top. The step walks only that window's
dialog fields: iced's own `focus_next` walks the whole tree, every inner
window and the page under the dialog with it. A box or a choice takes no
focus in iced, so Tab passes over it.

**One dark look, in the welcome page's colours** (`theme.rs` in
`gui/tessaro-style`, the crate the GUI shares with Try Tessaro along with
`icon.rs` and the fonts; `main.rs` imports both as `theme` and `icon`), so the
client and the kiosk's own screen read as one product. The page's CSS
variables in `tessaro-selftest/files/index.html` are the source: its
near-black background is the desk, its blue and purple glows are the
chrome and the selection, and its accents and status colours are the
GUI's. The page's translucent cards and lines become solid colours, since
iced draws no gradients or blending behind them. There is no light
variant and no following the system theme. Every colour and style is a
named constant or function there, so windows cannot drift apart. The UI font is Manrope, bundled in
`gui/tessaro-style/fonts/` (SIL OFL, `OFL.txt` beside it) as static Regular
and Bold files, so the GUI reads the same on every OS; iced alone would take
whatever sans the host has. Monospace text (`Font::MONOSPACE`) is left to
the host.

## The node list

**One list: the known nodes from the `nodes` table of
`~/.config/tessaro/tessaro.db`, overlaid with what mDNS sees, keyed by node
id** (`nodes_view::merge`). Every change writes that one node's row, never
the whole list, so the app's long-lived copy cannot overwrite what
`tessaro-ctl` stored meanwhile.

* A known device that answers takes its live address and claimed state.
* One that presents another certificate than its pin is marked `MISMATCH`
  and cannot be opened: it was reinstalled, its `/data` was wiped, or someone
  is in the middle. Forget it to pin it again.
* An unknown device gets a row of its own.
* **An unclaimed device opens without a claim, a login or a pin**
  (`NodesView::openable`): it answers everything without a token (see **The
  claim model** in [settings.md](settings.md)). A stranger is reached at the
  address it was seen at (`worker::connect`), and only while it stays
  unclaimed. Once it answers, the worker writes it to the known nodes, pinned
  to that session's certificate with no token, so it stays in the list and
  later sessions are held to that pin. Actions that make a credential show the device's refusal;
  SSH and VNC go in by the empty root password instead of a key. Unclaim is
  disabled instead: the device does not refuse it, and the node would be
  forgotten here for nothing. The claim state comes from each status poll.

**A device's tags are badges in the Tags column, and pressing one filters the
list by it** (`tag_cell`, `Message::TagFilter`). Each tag keeps its colour
(`theme::badge_colour`, see **Tags** in [settings.md](settings.md)), and
`unclaimed` is the warning colour. The tags picked show as badges between the
toolbar and the table, each pressed again to drop it, with Clear for all; a
row has to carry every one of them, and the Find box matches tags too. A badge is a button
inside the cell, so the press is the badge's and does not select the row
(`grid::widget`). A known device that is not seen shows the tags the store
last kept for it.

**Several rows are marked for a run on all of them: Cmd-click adds or takes
out one, Shift-click marks every row from the selected one, Cmd-A every row
the filters show** (`NodesView::click`, `Marks`). With the tag filter that
is every device of a tag. A plain click or Up and Down select one row again
and drop the marks; a filter drops the marks it hides. The modifiers come
from the keyboard's `ModifiersChanged`, since a row's press carries none.
Open, Login, Claim and Forget act on the selected row as before; **Run on
marked** opens a bulk window for the marked devices that can be opened
(`openable`), and says how many it left out (**The bulk window**). The grid
paints every marked row as selected (`grid::grid_marked`).

Discovery runs for as long as the app does (`discovery.rs`). Rescan starts a
new browse. A device mDNS cannot see (another subnet, a VM) is added by
address. Both are actions on the node list's own toolbar, since they act on
that list; the app header only has the app's name.

**A device added by address is written to the known nodes at once, claimed or
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
keeps the new token for its later calls and writes the node's row,
and the node list reloads.

## Device windows

**Each opened device has a worker thread that owns its blocking `Session`**
(`worker.rs`). The window's subscription starts it; closing the window drops
the subscription, and with it the thread and the connection.

* It polls `Status` every 2s, and every second while a guarded change waits
  for confirmation, so the countdown moves.
* It fetches the settings again only when `Status.revision` moved.
* A lost connection is retried with a growing pause. The node is opened by
  name (last address first, then mDNS), held to its pin, and the known nodes
  are read again each time, so a device that moved is found and remembered.
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
* **The pages after Overview are listed under named sections**, so a long
  nav stays readable: each entry of `Page::TOOLS` names its section, a
  section's pages stand together, and a muted title that is not a button
  stands above them. The titles are `agent/client/src/sections.rs`'s, which
  also orders the groups in `tessaro-ctl --help`, so the ctl and both
  menus sort a subject the same way.
* A prefix no page shows gets an entry after the pages, under Settings, in
  the device's order (`device::own_sections`), and clicking it opens its
  settings window,
  so a group a newer image adds appears by itself. Data is always listed,
  empty or not, so the first custom value can be added there; unsetting
  one is its Delete.
* A settings window belongs to its device window (`main.rs`, `configs`):
  one per group, Configure again raises it, and it closes with its device.
  It keeps its own selection and filter, and the edit dialog opens inside
  it; what a change did still goes to the device window's Messages.
* **A CEC console belongs to its device window too** (`main.rs`,
  `consoles`; `device/console.rs`): one per device, its button raises it,
  and it closes with its device. It sends any message as `tessaro-ctl
  screen cec send` (hex data, an address, an optional reply opcode; Enter
  sends) and draws the answer under its fields, or in Messages once it is
  closed. Below, the bus's message log is followed as `screen cec messages
  -f` on a connection of its own (`cec_log.rs`), asked every
  `cec::MESSAGES_POLL` from the last message shown, and only while the
  console is open: closing it drops the subscription, which ends the
  thread. It keeps the newest 500 lines, with Pause and Clear as on the Log
  page.
* **Each action is on one page only: the page of its command group.**
  Overview is the `device` group and nothing else, so it stays a summary; a
  `browser` command goes on Browser, not on Overview.

**Every page is drawn the same way** (`section.rs`, `grid.rs`): a toolbar
with the actions on the page first and those on the selected row after them,
then its tables. Anything that is a list is a table.

Tables use `iced_table` (MIT): drag a header divider to resize a column, and
scroll horizontally when the columns exceed the window width. Dividers are
always visible in the header; body rows have a continuous background. The
crate styles a row by its index alone, so the selected row is painted by its
own cells (`theme::table_selected`). Do not go back to the `iced_table2`
fork for its `selected_row`: it is GPL-3.0, which would make the whole GUI
GPL-3.0. Click a
column title to cycle ascending, descending, then original order; an arrow
marks the active sort. Text sorts case-insensitively with numbers in natural
order, and formatted sizes use their underlying byte counts. Selection,
double-click actions and keyboard navigation follow the displayed rows.

Column widths and sorting are saved in `gui_prefs`, separately for the node
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

**Messages is the one text pane of a device window, and its text can be
selected** (`messages.rs`): what changes did, and what commands print -
ping, speed test, grow, printer discovery, `eval` with its `> code` echo,
job results. Drag or Cmd A to select and Cmd C to copy, or its Copy button
for the whole log. Right-click offers Copy of the selection, or Copy all
when nothing is selected (`copy_menu_with` in `copy_menu.rs`, which takes
the text from the log since no widget reports a selection). iced's text widgets only paint, so Messages is a
`text_editor` that drops every edit, and a highlighter paints each span in
its tone. A new line rebuilds the text and moves to the end, so a selection
does not survive a line arriving: select once a streaming job is done, or
use Copy. Starting a job or an `eval` shows Messages if it is hidden,
without changing what new windows start with, which only the toggle sets.

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
| Overview | `device status` and `id`, systemd units, `device ping`, `device tags` (Tags: one comma-separated field, `device.tags`), `device factory-reset` |
| Screen | `screen show` above the modes table (each display's EDID identity, and the HDMI-CEC bus: the adapter, the TV's power and whether it shows the device, the rest of the bus, or how to switch CEC on), from the same `GET /api/v1/screen` answer as the table; `screen modes` with "use this mode", "Rotate" (`screen.rotation`), `screen screenshot` with a 3s live refresh and Save, `screen power`, `screen keyboard`, `screen vnc` ("VNC tunnel", a job holding the tunnel until Cancel) with the mirror's state from `device status`; the TV over HDMI-CEC, while CEC is on and an adapter is there: `screen cec wake`, `standby`, `source` ("This input"), `key` volume-down, volume-up and mute, `scan`, each answer in Messages, and the CEC console |
| Browser | what the browser shows, `browser navigate`, `reload`, `clear-cache`, `maintenance`, `debug`, `zoom`, `devtools` (a job holding the tunnel until Cancel), `inject`, `bridge`, `eval` (results in Messages) |
| Policies | `browser policies list` in priority order with its `#` column, `set` and `edit` in one wide editor (from the template, a file, or the stored text, checked as you type, saved against the revision it opened), `move` as Move up and Move down (the moved row stays selected), `show` (the effective policy), `remove` |
| Playlists | `playlist status` as facts above the lists, `playlist list` with "Make default" and "Clear default" (`playlist.default`), `create` and `set` in one dialog (name, transition and its length), `remove`; the selected playlist's items (`playlist show`) beside `items add` and `items set` in one dialog (an edit opens filled in, and what the chosen kind cannot carry is left out), `items move` as Move up and Move down (the moved item stays selected), `items remove` |
| Timetables | `playlist timetable list`, `timetable add` and `set` in one dialog (a choice of playlist), Enable/Disable, `timetable remove` |
| Network | `network show` and interfaces, `network last`, `network ping`, `network speedtest` (with "Bypass the proxy"), `network proxy set`, `off` and `test`, `network profiles list` and `show` |
| WiFi | `network wifi status`, `scan`, `join`, `hotspot-password` |
| Certificates | `network certs list`, `add` (a file picker) and `revoke` |
| Storage | `storage show`, partitions and filesystems, `storage grow` (check first) |
| Audio | `audio show`, outputs and inputs, choosing one, volume, mute, `audio test` for the tone and the recording |
| Camera | `camera list` as the saved format, size and mirrors, a row per camera (its node, the nodes of its mirrors, what it captures, a fallback or error), and the modes of the one selected, double-click to show it in the camera panel, `camera format` (a choice), `camera size` (opening a mode fills it in), `camera mirrors` (a choice); presence detection as `camera presence` says it, Presence (`camera presence on|off` with the camera, the near distance and estimating age and gender) and Calibrate (`camera calibrate`) |
| Time | `time show` and its servers, `time timezone` (a choice of `time zones`), `time ntp on|off` with servers, `time sync`, `time set` (this computer's clock or a typed time) |
| Scripts | `script list`, `create` and `set` in one dialog (a multi-line body, "Run on CEC events" for `--cec`, and "Run on scans of" for `--scanner`), `run` (a job; its output in a window as it comes), `remove`, `logs` (the Log page, filtered to the script's runs) |
| Schedules | `schedule list`, `create` and `set` in one dialog (a multi-line calendar, checked with `schedule check` as you type, and the script it runs), `enable`/`disable`, `remove`, `logs` (the Log page, filtered to the runs it started) |
| Printer | `printer list` with printer.enable above it, `printer discover` (a job; each printer found is a row to Add from), `create` (a dialog that stays open until the device takes it: a driverless printer must answer), `show`, `test`, `default`, `print` (a file picker), `remove`, `jobs` and `cancel` |
| Scanner | `scanner list` with scanner.enable above it, `scanner discover` and `identify` (jobs; each device found is a row to Add from, and the one identified is chosen), `create` and `set` in one dialog (the device, layout, terminator, gap, baud, prefix and suffix), `show`, `enable`/`disable`, `test` (a job; its scans go to the log as they come), `remove`, and the `scanner logs` table |
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
key is a `screen.*` one, whether it came from "Use this mode", "Rotate" or
from a setting, and one confirm keeps every change that waits.

**Dialogs are one generic form** (`pages::Form`), confirmed with Enter. A
destructive one - factory reset, unclaim, growing `/data`, an update that
erases `/data` or rewrites the disk - wants the device's name typed first,
as `tessaro-ctl` does. What the device shows once (a token, a password, the
hotspot password) comes in a dialog with Copy that closes only through Done.
After unclaim or a factory reset, the node is forgotten on this machine.
A multi-line field (a schedule's calendar, one expression per line, or a
script's body) takes
Enter as a new line, so its form is confirmed with the button. The schedule
form stays open until the device takes it: a refused save shows the
device's reason in it, and under the fields it shows how the device's
systemd reads the calendar and when it fires next, asked again once typing
pauses.

## Work on connections of its own

**Long work runs as a job on a second connection, so the worker keeps
polling** (`jobs.rs`). The jobs are the device's own jobs (`network ping`,
the speed test, `storage grow`, polled with `Session::job`), `device ping`,
files going up or down, an image update, the DevTools tunnel
(localhost:9222, or a free port when 9222 is taken here) and the VNC tunnel
(localhost:5900 the same way). Each is the
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

## The bulk window

**One action on every marked device, each device's run a job on a
connection of its own** (`bulk_view.rs`, an inner window of kind `bulk`).
It is `tessaro-ctl --tag` / `-n a,b` for the desktop: how the devices are
picked and run is **Running on several devices** in [clients.md](clients.md).

* The actions: reload the page, restart the browser, the display or the
  agent, reboot, set a setting (checked with `device::check` before
  anything runs, sent without a revision, since each device has its own),
  run a script, upload files into a directory of the file store, and update
  the image with the Update page's options. Each is a `jobs::Kind`; reload,
  the restarts, reboot and set are kinds of their own for this window, the
  others are the device windows'.
* The restarts, the reboot and the update are confirmed first in a dialog
  that lists the devices, as a device window confirms them. An update that
  erases `/data` or rewrites the disk wants the number of devices typed
  where a device window wants the name.
* At most `bulk::PARALLEL` jobs run at once, the next starting as one ends
  (`BulkView::fill`). The app's subscriptions take each running device's job
  from `BulkView::active_jobs`, keyed by the window and the device's place;
  the window's job ids start far above a device window's, which count from 1
  for the same nodes.
* A row per device says waiting, running with its last progress or line,
  done or failed with the message, and the ctl's summary goes under them
  once all have ended. Stop, or closing the window, drops what runs, as a
  device window's Cancel does, and leaves the waiting ones unrun. An update
  that erases `/data` forgets the node here, as a device window does.

## The camera panel

**The camera panel shows one camera's snapshot beside any page, like the
VNC panel** (`camera_panel_view` in `device.rs`). A device may have several
cameras, so there is no title bar toggle: double-clicking a camera on the
Camera page (or Enter on it) opens the panel on that one, and it stays on
it, whatever is selected later, until another is double-clicked or Close
closes it. It takes a snapshot as it opens, and has Take, a Live toggle
(`camera snapshot` every second, only while the panel is open) and Save.
With VNC open too, the two share the right side, one above the other. How a
snapshot is taken is **Snapshots** in [camera.md](camera.md).

**While presence detection watches the camera shown, its faces are boxed
over the snapshot**, green while near, with their id and distance, and
their estimated age and gender once settled with camera.presence.demographics
on (`faces.rs`, a canvas stacked on the image). The worker asks for `camera
presence` after each snapshot; the boxes are shares of the frame, placed in
the rectangle the contained image fills.

## VNC

**The VNC panel shows the device's screen live beside any page, and takes
the mouse and keyboard to control it** (`vnc.rs`, the VNC toggle), unless
the device is on `screen.vnc=view-only`.

* The device mirrors its screen only while a tunnel asks, and its server
  listens on its loopback only (see **The mirror on demand** in
  [remote-access.md](remote-access.md)). So the panel goes in through
  `tessaro_client::vnc::open`, as `tessaro-ctl screen vnc` does: it starts the
  mirror, sends the SSH key and pins the host key over the control connection,
  then runs the system's `ssh -N -L` from a free local port to
  `127.0.0.1:5900`, and stops the mirror when it closes. Its own connection
  is the viewer that keeps the mirror going; a reconnect starts it again. An
  unclaimed device gets no key and ssh gets in by its empty password (**SSH
  keys** in remote-access.md), so the panel works before a claim too.
* The server is neatvnc, which takes a login and no anonymous viewers: VeNCrypt
  with a plain login inside TLS, or the logins for viewers without TLS (the
  classic VNC password, Apple's, RSA-AES; see remote-access.md). The panel
  uses VeNCrypt. No Rust VNC crate speaks it, so `vnc.rs` is a small RFB
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
* **Keys go to the device only while the pointer is over the picture**
  (`vnc_keys` in `main.rs`, `vnc_has_keys` in `device.rs`), so typing
  elsewhere in the app stays the app's. Even then the zoom keys stay the
  app's, and a key a text field took is left to it. Leaving the picture
  releases every key and button held on the device, and a release always
  sends the keysym its press did (`held`), whatever Shift did meanwhile.
* **The pointer is mapped through the letterbox** (`letterbox`): the
  picture is drawn with `ContentFit::Contain`, so a position is scaled back
  to the device's pixels and the bars around it are outside the screen.
  Moving over the picture does not raise its window; a click does.
* **Input waits at most 20ms on a still screen.** The connection is one TLS
  stream read by one thread, so only the wait for the next server message
  has a read timeout, and input queued by the UI (`Control`) is written
  between messages. A timeout inside a message would lose its place.
* Characters are sent as the keysym of what was typed, Shift applied
  (`keysym.rs`); the device's VNC backend turns it back into a keycode with
  its own keymap.
* `screen.vnc=view-only` shows the picture without taking input, and the
  device would drop it anyway (remote-access.md). `screen.vnc=off` shows a
  note instead of a picture: the device refuses the tunnel.
* **For a viewer of the user's own, the Screen page's VNC tunnel job holds
  the forward** (`open_vnc` in `jobs.rs`, `tessaro-ctl screen vnc`): it says
  where to point the viewer and with which login, starts the mirror again
  every 20s and says when viewers come and go, until Cancel, which stops the
  mirror. One runs at a time.

## Adding a page

1. Add the page to `Page` and `Page::TOOLS` in `device.rs`, beside the
   pages of its section (`tessaro_client::sections`), and give it a
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

## The README's screenshots

**The README's pictures of the app are drawn headless, never taken from a
screen**: the ignored test in `screenshot.rs` boots `App`, opens a device
window and plays it the worker's events from the agent's fixtures, then
renders with tiny-skia (**README screenshots** in
[DEVELOPMENT.md](../DEVELOPMENT.md)). No worker runs, so a call the shown
page makes needs an entry in `call_answers`, by its tag. Monospaced text
comes from the host's fonts, the only part of the picture that differs
between machines.
