# Playlists

**A playlist is screens shown in turn - web pages, images and videos, mixed -
by a player page the device ships, instead of browser.url alone; the
timetable picks which playlist plays when.** The page plays: it preloads,
transitions, times each item, trims videos and watches for input, because
frame-exact timing and smooth transitions need the browser's own clock and
compositor. The agent manages: it picks the playlist, writes it for the page,
keeps copies of remote media, relays input from the page's frames and loads
the page again when it stops reporting. `tessaro-ctl playlist` keeps the
playlists, their items and the timetable in `tessaro.db`; the logic is
`agent/tessaro-agent/src/playlists.rs` (the store, the pick, what the page
gets), `control/playlists.rs` (the commands and the watchers), `media.rs`
(the cache) and the page in
`meta-tessaro-distro/recipes-browser/tessaro-selftest/files/player/`.

## On screen: browser.url or the player

**The player is on screen while `playlist.default` is set or the timetable
has an entry, enabled or not; otherwise browser.url is shown directly, as on
a device that has never had a playlist.** The configuration decides, never
the clock: a timetable with a lunch entry and no default keeps the player up
all day and plays browser.url in it, as a playlist of one, outside the entry.
Deciding by the clock would leave and take the player at the entry's edges, a
navigation each, which is a white flash on a public screen. So the only
flashes are setting the first default or timetable entry and removing the
last. A playlist that nothing uses changes nothing on screen.

* **The swap is `state::Effective`, like maintenance mode's.** In player mode
  `KIOSK_URL` is `KIOSK_PLAYER_URL` (`http://127.0.0.1/player.html`), so
  `generated.env`, the agent's navigation and origin enforcement follow it
  without knowing the mode exists, and a reboot comes up on the player.
  Maintenance mode and the debug screen still win over it (see **Maintenance
  mode** and **Debug screen** in [settings.md](settings.md)).
* **Whether the timetable has an entry reaches `Effective` through
  `state::Live`**, read from the store by `render::live` with the device's
  addresses (`playlists::Shared`). A change to the playlists that moves it,
  or the origins below, renders the configuration again like a setting does.
* **`KIOSK_PROBE_URL` reads as empty in player mode**, so the probe checks
  the player, which is always there. A source that is down is the player's to
  go past, not a reason to take the whole screen to the offline page.
* **The periodic refresh never reloads the player.** Its frames are loaded
  again each time their item comes round.

## What plays

**A timetable entry covering now plays its playlist; of several, the highest
priority wins, then the one listed first; with none, `playlist.default`
plays, and without that, browser.url.** An entry is days of the week, a
`from` and a `to` in the device's `time.timezone`; a `to` earlier than `from`
runs past midnight into the next day, and the same time for both is the whole
day (`protocol::playlist::covers`). An entry or a default naming a playlist
that is gone is passed over.

* **`watch_playlist` picks once a minute, on the minute, and at once after a
  change** to the playlists, the timetable or the configuration. What it
  picked goes to `/run/tessaro-kiosk/playlist.json` (`playlists::player_doc`,
  served by nginx at `/playlist.json`), written only when it changed, and the
  page is told with `window.__tessaroPlayer.reload()`.
* **The page gets every option filled in**: a URL item's `{key}`
  placeholders expanded as browser.url's are, the cached copy's address for a
  remote image or video, the playlist's transition where the item has none.
  The page decides nothing the agent already knows.
* **A different playlist switches at once; new items of the same playlist
  apply at the next item.** A playlist of one item switches at once too,
  since it never reaches a next item.

## The player page

**Two layers, one shown and one preparing the next item; an item comes on
screen only once it is ready to show, by a CSS transition on the
compositor.** So nothing half-loaded is ever seen. The page is plain modules
with no build step: `player-core.js` is the pure sequencing, tested with
`mise run player:test`, and `player.js` the DOM.

* **Ready means decoded.** An image is shown after `img.decode()`. A video is
  seeked to its trim start and paused on its first decoded frame
  (`requestVideoFrameCallback`). A page in a frame counts as ready at its
  `load` plus the item's `ready_delay_ms`, for a page that is still drawing
  then.
* **The next item is prepared as soon as one is shown**, and an item that is
  due while the next is not ready yet stays on screen until it is: never a
  blank screen.
* **A video plays to its trim end, checked per frame, or to its end**, muted
  unless the item has `sound`. The next item comes after it, whatever the
  playlist's other timings. A single video loops from its trim start without
  a transition.
* **Every prepare and play has a watchdog.** An item that does not load, an
  error, a video whose time stops moving: the page goes past it and says so.
  When a whole round of items failed, the page shows the device's offline
  page as a layer of its own (`/offline.html`, the staged
  `/run/tessaro-kiosk/index.html`) and keeps trying behind it; the agent
  never navigates away for it.
* **The old layer is torn down after each transition**: a frame emptied, a
  video's source removed, so the decoder is free for the next one.

## Interactive items

**An interactive URL or image item takes touch and keys; it moves on once its
duration has passed and nobody has touched it for its idle time.** The
duration is then the least it stays. A non-interactive frame gets
`pointer-events: none`, so a touch never lands in a page that is only shown.

* **Input inside a frame from another origin is invisible to the page**, so
  every frame says so itself: `control/frame-input.js`, registered by the
  agent in every document before the document's own scripts, calls the
  player's binding at most once a second, and the agent calls
  `window.__tessaroPlayer.input()`. The script takes the frame's copy of the
  binding first, so a page in a frame cannot report for the player.
* **A frame in a process of its own is a child session.** In player mode the
  page's DevTools session auto-attaches every iframe (`Target.setAutoAttach`
  with `flatten`), paused until it has the binding and the scripts
  (`waitForDebuggerOnStart`), then lets it run (`cdp/session.rs`). The child
  session gets `Page.enable` first: without it Chromium accepts its scripts
  and never runs them. Its contexts are kept apart from the page's: context
  ids are per target.
* **Reports come from the player page alone** - the main frame's own world on
  the player's origin. A heartbeat every 5 s, the item on screen, an item gone
  past and why; a player on screen that has not reported for 20 s is loaded
  again (`check_player`), unless someone is in DevTools.

## Frames that refuse to be framed

**A page that forbids framing (`X-Frame-Options`, CSP `frame-ancestors`) is
framed anyway: a code-less extension drops those headers for frames the
player opens, and only for those.** `frame-unlock/` in the `tessaro-kiosk`
recipe is a Manifest V3 extension with one `declarativeNetRequest` rule:
for `sub_frame` responses initiated by `127.0.0.1`, remove
`X-Frame-Options`, `Content-Security-Policy` and
`Content-Security-Policy-Report-Only`. Chromium's network stack applies it,
so no request waits on the agent, which `Fetch` interception would make it do.

* **The whole CSP goes, not only `frame-ancestors`**: the rule can only set or
  remove a header. It goes only inside the player; the same site opened
  directly keeps every header.
* **It is loaded with `--load-extension`** from `KIOSK_EXTENSION_ARGS` in the
  env file, which can be emptied, and `DisableLoadExtensionCommandLineSwitch`
  is in the default `KIOSK_DISABLE_FEATURES`, since branded builds switch the
  flag off.
* **It is loaded from a copy in `/run/tessaro-kiosk/frame-unlock` that
  `weston` owns**, made by tmpfiles every boot. Chromium writes an unpacked
  extension's indexed rules into the extension's own directory; from the
  read-only copy in `/usr/share/tessaro-kiosk` the load fails ("Internal error
  while parsing rules") and Chromium then never opens its DevTools port, so
  the agent cannot drive it at all. Measured on Chromium 147.
* **A page that checks `window.top !== window` and refuses to draw cannot be
  helped this way.** Script that tries to navigate the top window away is
  blocked by Chromium without a user gesture.

## Frames are first-party

**An interactive item keeps its login and its storage from one play to the
next.** A page in a frame of `127.0.0.1` is third-party, so the base policy
(`tessaro-kiosk-policy.json.in`) carries `BlockThirdPartyCookies: false` and
`ThirdPartyStoragePartitioningBlockedForOrigins: ["http://127.0.0.1"]`. Both
are static, so the policy never moves with the mode, which would restart the
browser.

* **The device grants follow the interactive items**: the origins of the
  interactive URL items of every playlist join browser.url's in the device
  origins (`render::device_origins`), and each such frame gets an `allow=`
  list (serial, hid, usb, bluetooth, camera, microphone and the like).
  Every playlist's, not the one playing, so the timetable switching
  playlists never rewrites the policy; saving a playlist that changes the set
  restarts the browser, like any change to the grants.
* **A URL item with `bridge` gets `window.tessaro`** in its frame, answered
  only from a frame directly in the player, on the origin of such an item or
  browser.url's (see [bridge.md](bridge.md)).

## The media cache

**An image or video from anywhere but the device itself is copied to
`/data/tessaro/media-cache/` and played from there**, so it plays with the
network down and never waits on a slow server between two items. Sources
under `http://127.0.0.1/` - the file store at `/files/`, the local pages'
`/media/` - are played where they are (`protocol::playlist::is_local`).

* **Only playlists the player can play are copied**: the default and every
  one in the timetable.
* **A copy is named by the SHA-256 of its URL**, with the URL's extension so
  nginx serves the right type (`cache_name`), fetched to a `.part` and renamed
  in, so nginx never serves half a file.
* **Every copy is asked again every 15 minutes** with `If-None-Match` and
  `If-Modified-Since`; a failure keeps the copy it has, and is tried again
  after 2 minutes. Until a source has a copy, the page plays it from its
  source.
* **Fetches go through the local proxy when there is one and trust the extra
  certificate authorities** (`http::trust`), like the agent's other requests;
  each phase and each frame of the body has its own deadline, so a large
  video on a slow link is fine and a stalled one is not.
* **Copies nothing wants are removed, and `/data` keeps the 256 MiB the file
  store keeps** (`files::RESERVE`): a copy that would eat into it is removed
  again and counts as failed.
* **The `media_cache` table says what each copy is**: its file, its ETag and
  Last-Modified, when it was fetched and the last error.

## The commands

**`tessaro-ctl playlist` keeps the playlists, `playlist items` their items and
`playlist timetable` the timetable; `--help` lists every option.** Items are
numbered from 1. `playlist create|set --file` take the JSON `playlist show
--json` prints. `playlist status` says whether the player is on screen, what
plays and why, the item on screen, what was gone past and how far the cache
is; `device status` carries the same.

* **A playlist that is `playlist.default` or in the timetable cannot be
  removed**, so the device never points at nothing. A rename takes
  `playlist.default` along.
* **`playlist.default` must name a playlist the device has** when it is set.
* **Like the settings, playlists stay through an unclaim and go with a
  factory reset**, the media cache with them.
