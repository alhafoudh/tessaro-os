# Changelog

What changed in each Tessaro OS release. The images, clients and Try
Tessaro of every version are on
[GitHub Releases](https://github.com/alhafoudh/tessaro-os/releases).

## 0.1.4

### Features

- Device status shows the reading of every temperature sensor on the device, in `tessaro-ctl device status`, `tessaro-gui`, Webconfig, the page bridge and the demo app. The kernel includes the coretemp, k10temp, NVMe and drivetemp drivers so these sensors can be read.

### Under the hood

- The genericx86-64 image runs oe-core's 6.6.142 kernel with USB audio, and QEMU boots that same image.

## 0.1.3

### Features

- Playlists show URLs, images and videos in turn on a player page, picked by a timetable, and `tessaro-gui` has a Player section with separate Playlists and Timetables pages.
- Devices can carry tags, with a reserved unclaimed tag, shown as badges and filterable in every client. In `tessaro-gui` the node list's tag filter sits under the toolbar.
- `tessaro-ctl` and `tessaro-gui` can run one command on several devices at once, picked by a list of names or by tag.
- HDMI-CEC puts the TV in standby and wakes it with screen power, sends CEC events to the page and scripts, and shows the display's identity in `tessaro-ctl screen show`. CEC actions and the bus's message log are available in `tessaro-ctl`, `tessaro-gui`, Webconfig, the page bridge and the demo app. A CEC message the kernel refuses is reported as not sent, with the reason.
- Presence detection runs BlazeFace on a hidden camera mirror on the device, with events for scripts and the page. Behind `camera.presence.demographics`, it also estimates each face's age and gender with HSE FaceRes, across the bridge, scripts, `tessaro-ctl`, `tessaro-gui`, Webconfig and the demo app.
- Barcode scanners can be read as a keyboard, over serial or as HID POS, with events for the page, frames and scripts, and are managed from `tessaro-ctl`, `tessaro-gui`, Webconfig and the demo app.
- The screen is mirrored over VNC only while a tunnel asks for it, through `tessaro-ctl screen vnc` or the VNC tunnel in `tessaro-gui`, and a lease stops the mirror when nothing uses it. The mirror stops as soon as `tessaro-ctl screen vnc` gets Ctrl-C.
- A demo app shows every kiosk feature through the page bridge. It ships in the image at `/demo/` and opens from the welcome page, and it replaces the self-test page. The page bridge allows actions by default.
- The demo app runs faster on integrated GPUs, without moving backdrop glows or backdrop blurs.
- The welcome page shows the device name in its details box, with the boxes split 3:2.
- `tessaro-gui` dialogs focus the first field to type in when they open, and Tab moves between their fields.

### Fixes

- The agent stops in seconds instead of waiting for leftover blocking threads.
- The camera mirror no longer starts before `camera.env` at boot and loses presence detection's Vision mirror, and it restarts itself when `camera.env` changes.
- `tessaro-ctl device tags add` no longer takes its tags as `--tag`, and its `--json` output streams one object per line.

### Under the hood

- Presence detection adds the tract and jpeg-decoder crates to the image.
- The qemu tasks take `--count N` to boot several devices on one network the host can see.

## 0.1.2

### Features

- The VNC mirror is interactive, so you can control the kiosk through it. Set `screen.vnc` to `view-only` to only watch.
- Any common VNC viewer can log in to the mirror.
- The `screen.rotation` setting turns the screen to match how it is mounted, and the boot splash turns with it. A change to the resolution and the rotation is confirmed or reverted as one.
- The console login banner shows the image version with its sha.

### Fixes

- The first click through the VNC mirror lands where you clicked.
- The VNC mirror no longer crashes when a viewer disconnects quickly.

## 0.1.1

### Features

- `--node` accepts the start of a known device's name or id, and matches it against known and scanned devices together.
- `screen.input.*` settings make libinput ignore mice, keyboards or touchscreens.
- `browser.block` and `browser.allow` settings fill Chromium's URL filter lists.
- The page bridge shows `device.name`, the hotspot SSID, clock and status fields, print jobs and the input volume.
- The page bridge no longer shows secrets or the lockdown settings.

### Under the hood

- genericarm64 boots under QEMU at Full HD, with vmnet.

## 0.1.0

### Features

- Tessaro runs a Chromium kiosk that stays on the kiosk's own origin, is locked down by an enterprise policy, keeps its profile across restarts and is ready for touch, 4K screens and the device APIs.
- `tessaro-agent` supervises the browser over a persistent DevTools session, puts a deadline on every external call and is restarted by systemd's watchdog if it stalls.
- An offline page appears when the kiosk page cannot be reached.
- The default kiosk page is a self-test page served from the device, with emoji and DejaVu fonts. Its WebSerial section picks a USB port by default and has a port picker.
- A welcome page shows a QR code that opens a setup portal on the device's hotspot, with a switchable sign-in sheet and an online indicator. The page bridge reports the same through `tessaro.network.online()`.
- Weston's on-screen keyboard appears when no hardware keyboard is attached, controlled by `KIOSK_OSK`.
- The boot splash, the wallpaper and the welcome, maintenance, offline and debug pages carry the Tessaro artwork and colours, with reworded offline and maintenance texts.
- `tessaro-agent` is the device's only management surface: `tessaro-ctl` talks to it over a local socket or TLS, and a claim model owns the root password.
- Devices are managed over one typed HTTPS API, with an OpenAPI document and Swagger UI.
- `tessaro-ctl` and `tessaro-gui` can manage an unclaimed device without claiming or pinning it.
- `tessaro-ctl access password set` takes the password as an argument or from stdin.
- Kiosk URLs are templates: `data.*` custom values, the read-only `node.id` and `net.*` keys and `net.public_ip` can be used as placeholders, and the agent keeps them current.
- A debug screen shows `debug.template`, a templated text with `{key}` placeholders, full screen instead of the kiosk page.
- Settings are applied to the running agent instead of restarting it, the agent stays up through Weston restarts, and the browser keeps running when a setting does restart the agent.
- `browser.zoom` is applied as Chromium's own page zoom.
- Browser policies let you merge named Chromium policy documents into the device's policy, ordered by priority with move up and down, from `tessaro-ctl`, `tessaro-gui` and Webconfig.
- Networking runs on NetworkManager, with `nmtui` for configuring the network in the field and a view of the network and its interfaces from `tessaro-ctl`.
- The device falls back from the WiFi client to its hotspot until the next boot when the client does not connect after boot.
- WiFi networks can be scanned while the hotspot is up.
- `tessaro-ctl` measures the device's internet connection against speed.cloudflare.com.
- Extra CA certificates can be managed from the agent, `tessaro-ctl` and `tessaro-gui`, where they have their own Certificates page.
- Every image ships an SSH server, and `tessaro-ctl` can authorize an SSH key on the device over the pinned connection, pinning the host key, and list or revoke keys.
- The kiosk screen is mirrored to a loopback VNC port with its own credential.
- A file store in `/data/files` is managed by `tessaro-ctl files` and served at `/files/`.
- Sound runs on PipeWire, with output, input and volume managed through `tessaro-ctl audio`. The audio test recording is kept in the file store as `audio-recording.wav`.
- `tessaro-ctl storage` shows the disk and grows `/data` online over the rest of it.
- Device status shows the hardware identity, CPU use and memory, in `tessaro-ctl`, `tessaro-gui` and the page bridge.
- Printing goes through CUPS: `window.print()` prints silently as a PDF through the agent, the page bridge has printer calls, and printers are managed from `tessaro-ctl`, `tessaro-gui` and Webconfig.
- USB cameras are shared through virtual cameras named Mirror N (`camera.mirrors` per camera), with camera list, format and size in `tessaro-ctl`, `tessaro-gui` and Webconfig. The kiosk uses the mirrors, not the real cameras.
- Camera snapshots can be taken on demand, with live previews in `tessaro-ctl`, `tessaro-gui` and Webconfig. In `tessaro-gui` and Webconfig the preview opens in a side panel on the camera you double-click.
- Scripts can be run on demand, by schedules and from the page, managed from `tessaro-ctl`, `tessaro-gui` and Webconfig.
- Chromium decodes video in hardware on the Pi and x86, and the Pi-only `device.gpu_mem` setting sets the GPU memory.
- The Raspberry Pi 3 renders on the GPU instead of the CPU.
- The Pi runs full KMS, and the agent restarts Weston when plugging in a screen or keyboard changes its configuration.
- `tessaro-gui` is a desktop client for technicians, with the Manrope font, a macOS app icon, resizable and sortable tables that remember their layout, a selectable and copyable Messages log, toolbars that wrap on narrow pages and a green countdown Confirm button for guarded screen changes.
- `tessaro-gui` remembers devices added by address or opened unclaimed and shows unseen devices as not seen.
- Webconfig is a browser management interface on the device, with Quick Setup, browser sessions and tickets, a live Screen panel and a phone layout. `tessaro-ctl` and `tessaro-gui` can open it.
- The `tessaro-ctl` help, the `tessaro-gui` navigation and the Webconfig menu are grouped into the same named sections.
- `tessaro-ctl` bash completion is installed in the image.
- An example page lets you try the `window.tessaro` page bridge.
- Images are built for genericx86-64, raspberrypi3-64, raspberrypi4-64 (Pi 4B, 400 and CM4) and genericarm64, which boots under QEMU, also on a Mac.
- Try Tessaro is a macOS app that runs a Tessaro device in a bundled QEMU VM on the Mac's GPU.

### Fixes

- HDMI links stay on 8-bit RGB, so the Pi no longer falls back to YCbCr 4:2:2 and bands dark gradients.
- A Pi booted without a screen lights up when HDMI is plugged in, since Weston starts with no screen connected and ignores writeback connectors.
- Chromium gets a session bus and no longer logs errors at every start.
- Browser restarts no longer leave Chromium metrics files on `/data`.
- Updates fit on a `/data` that has not been grown, and uploading a `.wic.zst` image no longer fails its partition table check before enough of it is decompressed.
- iPhones open the setup portal on the hotspot, and iOS no longer zooms into the portal's fields.
- A proxy that answers 401 is treated as a refused proxy login, like 407.
- The managed network profiles show as (managed) instead of (not saved).
- `tessaro-gui` no longer flickers its VNC panel, and the actions where it had drifted from `tessaro-ctl` behave the same as in `tessaro-ctl`.
- OK works in Webconfig dialogs opened from Configure.

### Under the hood

- Tessaro OS is built with kas on top of Moonforge, with Chromium as the browser, NetworkManager for networking and no container runtime.
- The default hostname is `tessaro`.
- The root partition is 4096M, the ESP 256M and the Pi boot partition 512M, the x86 swap partition is gone so `/data` is last, and images ship a 1 GB `/data` that the device can grow.
- Images are named `tessaro-os`, versioned as the release version plus the short git sha (with `-dirty` for uncommitted changes), and ship as `.wic.zst` with a `.wic.bmap`. Devices decode zstd as well as bz2.
- The device and client stores are SQLite databases, and the image includes the `sqlite3` shell.
- Releases carry `tessaro-ctl`, `tessaro-gui` and the Try Tessaro DMG beside the images, with the bundled QEMU's source.
- Tessaro is licensed under Apache-2.0, ships an SBOM with a license list and license policy, and the images' GPL, LGPL and AGPL sources are archived.
- `tessaro-gui` uses the MIT-licensed `iced_table` instead of the GPL-3.0 `iced_table2`.
