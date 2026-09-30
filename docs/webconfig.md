# Webconfig

**Webconfig is the device's own management interface in a browser**: Quick
Setup and everything `tessaro-ctl` and `tessaro-gui` manage, served by the
agent at `/` on the API's port, `https://<device>:7400/`. It looks and works
like the GUI's device window - its menu, pages, toolbars, tables, dialogs,
Messages and status bar - so a user of one finds their way in the other.
What only a native client can do stays there: the VNC viewer, the SSH
terminal, DevTools over SSH and finding devices on the network. The SSH
keys and every setting behind those remain on Webconfig's pages.

## Sessions

**Tessaro has tokens, not user accounts, and a browser session is a
token's.** A browser can neither pin the device's certificate nor keep a
token safe from the page's own scripts, so it trades one for a session: a
random id in a cookie the page's scripts cannot read, standing in for that
token (`api/sessions.rs`). An unclaimed device needs no session at all, as
it needs no token ([settings.md](settings.md), the claim model).

* **A browser gets a session three ways**, each answered with the cookie:
  signing in with a token (`POST access/session`), redeeming a one-time
  ticket (`POST access/ticket/redeem`), or claiming the device from the
  browser - the claim's answer carries the cookie itself, because on the
  hotspot the phone is dropped right after it, and a second request to sign
  in would never arrive.
* **The cookie is `__Host-tessaro-<node id>`, `Secure`, `HttpOnly`,
  `SameSite=Strict`.** The node id is in the name because cookies ignore
  ports: two devices behind one address (qemu's forwards) would otherwise
  overwrite each other's.
* **A session is only as good as its token.** It keeps the token's id and
  stored hash, and every request checks the device still has that exact
  token (`Control::token_sha`): revoking it, unclaiming or a factory reset
  ends the session with nothing told, and a new token that happens to get
  the same id does not revive it. A session never changes whether the
  device is claimed.
* **It lasts `access.session_timeout` without use**, a week by default. The
  key restarts nothing - an agent restart would end every session - and
  the API reads it from the state it last read (`Control::session_timeout`).
* **Only what someone does is use.** A request counts only with
  `X-Tessaro-Activity: 1` (`api::HEADER_ACTIVITY`), which Webconfig sends
  with every write and with a read made within 2 s of an input or a page
  change (`webconfig/src/api/activity.ts`). The pages poll the status all
  the time; a page left open in a tab would otherwise keep its session
  forever. A header rather than a separate touch endpoint because there is
  then nothing extra to call, and a custom header cannot be sent by another
  site without a CORS preflight, which the agent never answers.
* **Sessions are in memory, as hashes, and end with the agent** - except
  across a restart the agent makes itself to apply a change. Most settings
  restart no agent at all (the running agent applies them, and a Weston
  restart leaves the agent running), so sessions simply carry on. The rest -
  a `Consumer::AgentRestart` key such as `access.listen`, the proxy switched
  on or off, the extra certificate authorities, a factory reset - restart
  the agent, and logging everyone out for them would make Webconfig
  unusable. So `restart_to_apply` of the agent unit (`After::Restart` or
  `After::Restarts`) first writes the sessions and how long each has been idle to
  `/run/tessaro-kiosk/sessions.db` (the `sessions` table, on tmpfs), and
  the next process reads and empties it at start in one transaction
  (`Control::load_sessions`). A restart
  someone asks for (`device restart`, `After::RestartAsked`), a crash and a
  reboot end every session.
* **A cookie of an ended session is no credential and no offence**: the
  request is treated as if it had none, with no limiter strike - every
  browser brings one after the agent restarted - and the answer clears it.
* **Signing out** (`DELETE access/session`) ends this browser's session and
  clears its cookie.

## Opening it from ctl and the GUI

**`tessaro-ctl access webconfig` and the GUI's Open Webconfig (Access page)
open the address the client itself reached the device at**
(`Session::address`), so it is the right IP and port whichever way the
device was found. On a claimed device the client asks for a ticket
(`POST access/ticket`, Bearer callers only, good once and for a minute) and
opens `https://ADDRESS:PORT/#ticket=...`; the page redeems it and drops it
from the address (`webconfig/src/session/SessionContext.tsx`). The ticket
rides in the fragment, which a browser never sends to any server, and the
token itself never goes into a URL. Both clients share this in
`agent/client/src/webconfig.rs`. `--print` prints the address instead, for
a machine without a browser. A link-local IPv6 address is refused: a
browser cannot take its interface.

## Where it opens

**A fresh device opens on Quick Setup, any other on Overview**
(`WebSession.fresh`): fresh is unclaimed and no setting stored.
Quick Setup's first save sets something, so from then on the device opens
on Overview; Quick Setup stays first in the menu. What Quick Setup does is
in [quick-setup.md](quick-setup.md).

## The pages

**The menu is the GUI's** (`Page::TOOLS` in `gui/tessaro-gui/src/device.rs`,
`webconfig/src/pages/registry.tsx`): Quick Setup first, then the GUI's pages
in the GUI's order, then the setting groups no page claims (`own_sections`),
Data always among them. A page's Configure opens the settings it owns in a
dialog over the page, with the GUI's scopes (`Page::scope`: the WiFi keys
are on WiFi, not Network), and only where the scope has settings.

* **The Screen panel stands where the GUI's VNC panel does**, beside every
  page (`shell/ScreenPanel.tsx`), opened from the title bar and remembered
  in the browser's `localStorage`. A browser has no VNC client, so it is a
  screenshot every 3 s (the GUI's `LIVE_SHOT`), taken only while the panel
  is open, the tab visible and the device answering, never two at once.
  VNC itself stays the native clients' ([remote-access.md](remote-access.md)).
* **Row actions act on the selected row**; double-click or Enter opens it,
  as in the GUI. A page never asks for the name of something its table
  already shows.
* **Dialogs keep what is typed** while data refreshes underneath, focus
  their first useful field, and show the device's refusal beside the
  fields. A change on probation shows the green Confirm (Ns) button on the
  Screen page and counts down in the status bar.
* **The status is polled every 2 s, every second while a change is on
  probation, and the settings fetched again only when its revision moves**,
  as the GUI's worker does (`webconfig/src/device/DeviceContext.tsx`). What
  a change did goes to Messages.
* **Long commands are jobs**, polled every 400 ms (`useJob` in
  `webconfig/src/api/jobs.ts`); leaving the page cancels one still running.
  A script's run on Scripts is one too: leaving stops following it, and the
  run goes on ([scripts.md](scripts.md)).
  File transfers and image updates go in `UPDATE_CHUNK` pieces and resume
  from what the device says it has (`webconfig/src/flows/`).
* **The Printer page prints a file as one request**, its bytes the body
  (`printDocument` in `webconfig/src/flows/raw.ts`), so at most
  `PRINT_DATA_MAX`; a larger document goes into the store on Files and is
  printed by its path (`tessaro-ctl printer print NAME --stored PATH`).
  Discover is a job whose finds fill a table of their own, each added with
  the URI it was found by ([printing.md](printing.md)).

**The words are the Rust's.** A result the clients describe in words - a
status, what a change did, a job's steps - is a port of `agent/client`'s
`describe` and friends (`webconfig/src/describe/`). Golden fixtures keep
the two equal: each file in `agent/client/tests/describe/` names a
function, holds an input and the spans it renders to; `cargo test -p
tessaro-client --test describe` checks the Rust against them
(`UPDATE_DESCRIBE=1` rewrites them) and `webconfig:test` checks the port
against the same files. A change to the Rust's words fails the frontend's
tests until the port follows. Rust writes `{:.1}` with ties to even, not up
as `toFixed` does: floats go through `fixed()` in `describe/common.ts`.

## The look

**The GUI's theme, copied**: the colours and sizes of
`gui/tessaro-gui/src/theme.rs` are Tailwind `@theme` tokens in
`webconfig/src/styles.css`; change them in both together. Manrope is
vendored (`webconfig/src/fonts/`, OFL) and bundled, so nothing is fetched
from the internet - a device on a hotspot has none.

**A phone gets everything 1.25x and a layout of its own**, since the GUI's
13px text and tight controls are too small to read and tap there. Below
Tailwind's `md` breakpoint (`max-md:` in the classes):

* The root font size is 125% (`styles.css`), which makes the body text
  16px. More leaves a 390px-wide phone too few columns for a label and its
  value. That scales only what is in rem, so sizes are written in rem,
  never px (13px is `0.8125rem`). The table's column widths and heights
  are the exception: pages write the GUI's px and `Table.tsx` turns them
  into rem.
* Inputs are 16px whatever the scale: below that iOS zooms the page in on
  focus and leaves it zoomed.
* Buttons, menu entries, table rows and Quick Setup's sections are taller,
  to be tapped.
* The title bar is one row: the menu button, the name and Refresh. Its
  other tools are the drawer's last entries (`Shell.tsx`).
* The status bar is one line: the link, the name, and what is wrong or on
  (`StatusBar.tsx`).
* A fact table puts each label over its value (`Facts` in `controls.tsx`),
  toolbars wrap without their separators, and a wide table scrolls
  sideways.

## Serving

**The agent serves the built files from `/usr/share/tessaro-webconfig`**
(`KIOSK_WEBCONFIG_ROOT`, `api/statics.rs`). A path without a `.` in its
last part that is no file is answered with `index.html`, since the routes
are the app's own (`/network`, `/settings/data`); anything under `/assets/`
never is, so a missing script is a 404, not a page parsed as JavaScript.

* **Vite names every built asset after its content, so `/assets/` is cached
  for good** (`immutable`); everything else, the page and the API included,
  is `no-store`, so a device updated in place serves its new Webconfig at
  the next load.
* **The page has a strict Content-Security-Policy**: its own scripts,
  styles and fonts only, images from `blob:` (screenshots, downloads) and
  `data:`; no inline script, no framing, `Referrer-Policy: no-referrer`.
* **The API refuses browser requests from anywhere but the device's own
  origin** (**Trust and auth** in [api.md](api.md)).

## The frontend

**React, TypeScript, Vite, Tailwind and TanStack Query, in `webconfig/`**
beside `agent/` and `gui/`. The API's types and a typed client are
generated from `agent/protocol/openapi.json` (`openapi-typescript` into
`src/api/schema.d.ts`, which is not checked in, with `openapi-fetch` and
`openapi-react-query`), so a path or body the device does not have is a
type error, and a change to an endpoint reaches Webconfig through the same
document the Rust side regenerates.

* **Node is pinned to bitbake's**, 22.11.0 in `mise.toml`, the version
  meta-browser's `nodejs-native` has; Vite stays on 6, which runs on it.
* **`mise run webconfig:run`** is Vite's dev server on
  `http://localhost:5173`, proxying `/api` to a device,
  `TESSARO_WEBCONFIG_TARGET` (default the qemu forward,
  `https://127.0.0.1:17400`). The proxy makes the requests come from the
  device's origin and hands the browser a plain cookie under another name,
  mapped back on the way in (`webconfig/dev-proxy.ts`): a `__Host-` cookie
  is kept only for https.
* **`webconfig:build`** writes `build/webconfig`, which `agent:integration`
  serves; `webconfig:test` and `webconfig:lint` are its checks.

## Built in bitbake

**`tessaro-webconfig` builds the app offline in the image build**
(`meta-tessaro-distro/recipes-browser/tessaro-webconfig`), with the Node
meta-browser already builds for Chromium, so the image build needs no Node
on the host. Its own recipe, so a change to the pages never re-hashes the
cargo build in tessaro-kiosk.

* **The dependencies come through bitbake's `npmsw` fetcher**, which
  unpacks every package of a lockfile into a ready `node_modules`, with the
  devDependencies (`dev=1`), since the build tools are what is needed. It
  runs no install scripts and makes no `.bin` links: the recipe runs each
  tool by its path, and esbuild, rollup, lightningcss and Tailwind's oxide
  find their binaries in their per-platform packages without scripts.
* **It fetches `bitbake-lock.json`, not `package-lock.json`.** npmsw fetches
  every entry whatever its `os` and `cpu`, and refuses an entry without an
  integrity, which is what a package bundled in another (`inBundle`) has.
  `webconfig/scripts/bitbake-lock.mjs` keeps what a Linux build host on
  x86_64 or arm64 runs; `mise run webconfig:lock` writes it after any
  change to `package-lock.json`, and `webconfig:test` fails when it is
  stale.
* **The recipe lists the sources by name**, not `webconfig/` whole, so a
  developer's `node_modules` or build output never reaches the fetch or the
  hash, and brings `agent/protocol/openapi.json` to the same relative path,
  so the codegen runs as it does in the checkout.

## Testing

The `webconfig` e2e lane (`test/e2e/spec/webconfig_spec.rb`) asks the API
from the host as a browser does (`Browser` in `support/api.rb`, which sends
`Origin` and keeps cookies): the files and their caching, the origin rule,
a claim that signs the browser in, tickets, signing out, a revoked token,
and a change the agent restarts for against a restart someone asks for.
How the pages look and behave is checked in a browser by hand.
