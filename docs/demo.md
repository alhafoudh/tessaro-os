# The demo

**The demo shows every feature of the device on the device's own screen,
through the same page bridge any kiosk page has, so what it shows is
exactly what a customer's page can do.** It is a React app in `demo/`: a
home page of feature tiles, each with a badge saying whether that feature
is ready on this device, and a section per feature with a live
demonstration. It never talks to the device except through
`window.tessaro` ([bridge.md](bridge.md)) and the browser's own APIs.

## Where it runs

**The demo is served from the device's file store, at
`http://127.0.0.1/files/demo/`**, which `mise run demo:sync` builds into
`/data/files/demo` and opens (`tessaro-ctl files sync`,
[files.md](files.md)). That is the welcome page's origin, which matters
twice:

* **The bridge answers it.** The bridge answers the origin of
  `browser.url` (**Who may call** in [bridge.md](bridge.md)), and the
  factory `browser.url` is the welcome page at `http://127.0.0.1/`. A demo
  on any other origin would get no `window.tessaro` while the welcome page
  is the kiosk's page.
* **The device grants apply.** Serial, HID and the camera mirrors are
  granted to the device origins by policy, and `http://127.0.0.1` is one of
  them (**Device APIs** in [kiosk-browser.md](kiosk-browser.md)).

**The agent's refresh timer takes the browser back to `browser.url`**
every `agent.refresh_interval` seconds, whatever page it is on; a page
elsewhere on the same origin is not drift, but the timer still navigates.
While working on the demo, `agent.refresh_interval=0` keeps it on screen.

**Most sections need `browser.bridge.mode=actions`.** With `config` the
demo reads but cannot act, and with `off` it has no bridge at all; it
says so on the home page and in each section, with the command that
changes it.

## Sections and what they detect

**Every section works out whether its feature is there before showing it,
and says why not and how to switch it on.** `src/features/detect.ts` has
one `detect()` per section, run on the way in and again when the settings
change (`tessaro:config`), when a camera comes or goes, and every 20s. A
section is one of:

| Status | Badge | When |
| --- | --- | --- |
| `ready` | Ready | the feature works here |
| `limited` | Partly | some of it works: the browser's part without the bridge's, software WebGL, no touchscreen |
| `off` | Off | the operator has it switched off (`camera.presence.enable`, `screen.cec.enable`, `printer.enable`, no script marked `--bridge`) |
| `needs-bridge` | Bridge off / Read-only | the bridge mode is below what the section needs |
| `no-hardware` | Nothing plugged in | no camera, no printer set up, no HDMI-CEC adapter |
| `offline` | Offline | the section streams from the internet and the device cannot reach it |

**Detection only reads.** A section that is off shows the operator's
command (`tessaro-ctl camera presence on`, `config set printer.enable=1`)
and where the same switch is in Webconfig; the page cannot switch any of
them itself, by design ([bridge.md](bridge.md)).

**The section always renders under its banner**, so the parts that need
nothing from the device - the inputs, the synthesizer, WebGL, a camera
preview - work with the bridge off.

The registry of sections is `src/features/registry.ts`; the sections are
`src/sections/`. The self-test page's checks (fonts and emoji, every input
type, multi-touch, scrolling, codecs, WebSerial and WebHID with the
serial port picked by USB id) live in the sections they belong to.

## What it changes on the device

**The demo writes one setting, `data.demo_note`, and only when asked.**
The Saved data section sets and removes it with `tessaro.data.set()` and
`unset()`; no template uses it, so the page does not move. The volume and
the microphone level are shown, not changed.

The actions it offers are the bridge's: the screen switched off for 10
seconds and on again (and on again if the section is left early), the
on-screen keyboard raised and lowered, a ping, a speed test, a receipt
printed, a script run, the browser reloaded, sent home, its cache cleared
or restarted, maintenance for a few seconds, and a reboot behind a
confirmation. A refusal for coming too soon after the last page restart
(`DISRUPT_GAP`, the speed test's gap, the print and script bursts) is shown
as a countdown on the button (`src/bridge/refusal.ts`).

## Maintenance for a few seconds

**Maintenance puts another page on screen, so the demo makes that page its
own.** It calls `browser.maintenance(true, url)` with its own
`#/maintenance-return` as the URL. The bridge answers the maintenance URL's
origin, which is the demo's, so that route can count down
`MAINTENANCE_SECONDS` and call `browser.maintenance(false)`
(`src/features/maintenance.ts`). When the agent refuses that for coming
within `DISRUPT_GAP` of switching it on, the route asks again every second
and shows the seconds left. Maintenance is a setting, so a device that
restarts during it comes back on the same route, which switches it off.

**Switching maintenance off loads `browser.url`**, not the demo. The demo
leaves `tessaro.demo.return` in `localStorage`, shared with every page on
the origin, naming the section to come back to and when.

## Navigation

**Every page works with a finger, a keyboard and a TV remote.** The bottom
bar's buttons go home and to the previous and next section. The arrows
move the focus to the nearest control in that direction
(`src/shell/focus.ts`), Enter presses it, and Escape or Backspace go back
home. `screen.cec.keys` sends the remote's arrows, OK and exit as those
keys (`agent/protocol/src/cec.rs`), so the demo needs nothing of its own
for the remote. A section focuses its first button on the way in, never a text
field: a focused field raises the on-screen keyboard.

## The app

**Routes are in the hash and every asset path is relative** (Vite's
`base: "./"`, react-router's `HashRouter`), so one build works wherever
nginx serves it. nginx serves files and has no fallback to `index.html`, so
a route in the path would be a 404.

* **Its tools are Webconfig's**, on the same Node 22.11 and Vite 6
  (**The frontend** in [webconfig.md](webconfig.md)), with Tailwind and the
  welcome page's palette, backdrop and mark (`src/theme.css`,
  `src/shell/Mark.tsx`). There are no API types: the demo has no API, and
  the bridge's calls are typed by hand in `src/bridge/types.ts` after
  `control/bridge.js` and the page shapes in `control/bridge.rs`.
* **`src/bridge/mock.ts` is a pretend device** that answers like the
  agent, refusals included. `demo:run` installs it, and so does `?mock`
  (`?mock=config` for the read-only bridge, `?mock=off` for none) on any
  build; a real bridge always wins.
* **`bitbake-lock.json` follows `package-lock.json`**, made by
  `scripts/bitbake-lock.mjs`, Webconfig's script under the demo's name, and
  checked by `demo:test`.

## Media

* **The recorded tones are in the demo** (`public/media/`), so the audio
  check works with no network.
* **The film is Big Buck Bunny** (Blender Foundation, CC BY 3.0), streamed
  from archive.org, with a 1080p clip from test-videos.co.uk as the second
  choice (`FILMS` in `src/sections/Video.tsx`).
* **The photo wall is Unsplash's** (`images.unsplash.com`, the Unsplash
  License), loaded at the size it is shown.

The streamed media need the internet; their sections say so when the
device is offline, and nothing streamed is part of the build.

## Adding a section

**A new feature of the device gets its section in the same change**
(**Planning a feature** in `CLAUDE.md`):

1. Its `detect()` in `src/features/detect.ts`, with a case in
   `detect.test.ts` for every status it can give.
2. Its page in `src/sections/`, taking `{ status }`, built from
   `src/shell/ui.tsx` (`Panel`, `ActionButton`, `Stat`, `Hint`).
3. Its entry in `SECTIONS` in `src/features/registry.ts`, with an icon from
   `src/shell/icons.tsx`.
4. The call in `src/bridge/types.ts` and an answer in `mock.ts`, if the
   section uses a bridge call the demo has not used before.
