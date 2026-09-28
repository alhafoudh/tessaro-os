# Display

## Display scaling

**Chromium cannot scale itself on this stack, so Weston does it.** The obvious
knob, `--force-device-scale-factor`, is inert here: Chromium only honours it
when the compositor advertises `wp_fractional_scale_manager_v1`
(`WaylandWindowManager::DetermineUiScale`), and Weston 13.0.1 does not
implement that protocol server side. Set it and nothing happens.

What is left is `weston.ini`'s `[output] scale=`, which on the DRM backend is
the *only* path: integers only, no fractional scaling, matched against an exact
connector name, with no `--scale` command-line option and no wildcard matching
(`drm_config_find_controlling_output_section`). Since a shipped image cannot
know whether it will be plugged into a 1080p or a 4K panel, or what the
connector will be called, the config is generated per boot:

* `/usr/libexec/tessaro-weston-config` runs as `ExecStartPre=` of
  `weston.service` (drop-in `10-tessaro-scale.conf`, from the `weston-init`
  bbappend), reads every connected connector out of `/sys/class/drm`, and
  writes `/etc/xdg/weston/weston.ini` plus an `[output]` section per connector
  to `/run/weston/weston.ini`. The drop-in then points `weston --config=` at it.
  It also writes `require-outputs=none` into `[core]` and a
  connected-connectors comment (see **Display hotplug**), `[input-method]`
  (see **On-screen keyboard**) and `[screen-share]` (see
  [remote-access.md](remote-access.md)).
* **Its log is `journalctl -t tessaro-weston-config`, not `-u weston`.** It runs
  as `ExecStartPre=`, and those lines do not come back under the unit even
  though the compositor's own do. Every decision it makes - connector, scale
  and why, keyboard and why - is one line there.
* Scale is `screen.scale` (`KIOSK_SCALE`) if set, otherwise 2 at 3400px
  wide or more and 1 below - measured on the mode being set, if one is. `none` writes
  no `scale=`.
* **Resolution is `screen.resolution` (`KIOSK_RESOLUTION`)**: `preferred`, or
  a `WIDTHxHEIGHT` written as `mode=` into each connector's `[output]`. It is
  the one setting that can leave nobody able to see the screen, so it is
  guarded on every layer. The agent only accepts a mode some connected connector lists
  in `/sys/class/drm/*/modes` (`tessaro-ctl screen modes` prints them); the
  generator writes it only for connectors that list it and leaves the rest on
  their preferred mode; and the change is on **probation** - it reverts on its
  own unless `tessaro-ctl screen confirm` arrives within 60s. The pending change is in
  `state.json`, so it survives the agent restarting with Weston, and the boot
  oneshot reverts a change still pending at boot: a reboot is not a confirm.
  The timer is monotonic, never the wall clock.
* **Every generated `[output]` sets `max-bpc=8`, so the link stays on 8-bit
  RGB.** Weston's default is 16 (`weston-drm.man`), which lets the driver pick
  deep colour. Where the link cannot carry deep RGB, vc4 falls back to 12-bit
  YCbCr 4:2:2 in limited range (`HDMI_CSC_*` in
  `/sys/kernel/debug/dri/0/hdmi0_regs` holds the RGB-to-BT.709 matrix, and
  `max_requested_bpc=12` in `.../state`). The monitor's conversion back to RGB
  then loses levels and dark gradients band visibly, even though Chromium's
  frame, captured as PNG, is smooth and dithered. Nothing above Weston
  renders more than 8 bits per channel, so deep colour gains nothing. This is
  why an `[output]` is written for every connector even under `KIOSK_SCALE=none`
  with the preferred mode: it then carries only `name=` and `max-bpc=`.
* **The technician-facing file is still `/etc/xdg/weston/weston.ini`**, which is
  on the `/etc` overlay and persists. It is the base the generator copies, and
  any connector already named there (any `name=` line) is left alone - so
  a hand-written scale always wins. `/run/weston/weston.ini` is generated and
  must never be edited.
* The empty `ExecStart=` in the drop-in is required to clear oe-core's line
  before replacing it, and `--modules=systemd-notify.so` has to be carried over
  verbatim - `weston.service` is `Type=notify` and hangs without it.
* The `screen.*` keys are the ones read by the compositor, so
  `tessaro-ctl config set` restarts Weston for them, and with it the browser and the
  agent.

## Display hotplug

**The generated config is kept true to what is plugged in, by the agent.**
`tessaro-weston-config` only sees the screens and keyboards attached when
Weston starts. So `watch_display` in `control/watchers.rs` reads `/sys/class/drm/*/`
`status`/`modes` and `/sys/class/input/input*` every 2s. Once a change has
held still for 5s, it runs the generator again, as root, into
`/run/tessaro-kiosk/weston-candidate.ini`, and compares that with
`/run/weston/weston.ini` (`agent/tessaro-agent/src/hotplug.rs`). Weston is
restarted, taking the browser and the agent with it, only if one of these
holds:

* a connector is connected now that was not when Weston started. The
  generator records that set in a `# tessaro-weston-config: connected ...`
  comment line, and the agent matches that exact prefix. This is what brings a
  device booted with no screen onto a screen plugged in later, whatever the
  scale and resolution settings are;
* an `[output]` section would now be written differently: a different panel
  on the same connector;
* the `[input-method]` section changed: `screen.osk=auto` saw a keyboard come
  or go.

A connector going away is never a reason. Weston copes with a head
disappearing, and the running config keeps its section for when the screen
comes back. So a monitor switched off and on restarts nothing. Every restart,
and every refusal to restart for the same hardware, is one `display: ...` line
in `journalctl -u tessaro-agent`; "the config still fits" and an
operator-stopped Weston are logged only with `agent.debug=1`.

* **`screen.resolution` and `screen.scale` stay in charge.** The restart
  only re-runs the same generator with the same settings, so a late screen
  gets the configured mode (if it offers it) and the right scale. The check is
  paused while a change is on probation: that change restarted Weston itself,
  and a monitor re-syncing to the new mode must not be taken for a new one.
* **It cannot loop.** The hardware snapshot a restart was made for is written
  to `/run/tessaro-kiosk/display-reconciled`. If Weston comes back from that
  restart and the config still differs for the same hardware, the agent logs
  it and leaves it alone. It also leaves an operator-stopped `weston.service`
  alone.
* **Weston has to be allowed to start with no screen.** Weston 13's `[core]
  require-outputs` defaults to `any`: with no output to enable, it exits with
  status 1, and its log just stops at `Color manager: no-op`. The generator
  adds `require-outputs=none` to `[core]` unless the base sets it; Weston then
  starts with no screen and lights one by itself when HDMI is plugged in.
  Chromium started with no output does not answer DevTools, so the agent
  restarts it once meanwhile. That is harmless.
* **Writeback connectors are not screens.** vc4 under full KMS exposes
  `Writeback-1`, always `connected` with no modes. The generator and
  `tessaro-ctl screen modes` skip it.
* **The generator's output must depend on the settings and the hardware
  only.** A timestamp or anything random in it would make every hotplug
  restart the compositor.
* **On the Pi this only works on full KMS** (`VC4DTBO = "vc4-kms-v3d"` in
  `kas/common/raspberrypi.yml`, for every Pi target). meta-raspberrypi
  defaults `raspberrypi3-64` to fake KMS, where
  the firmware owns HDMI: a screen missing at boot never comes up, and a
  monitor switched off and on shows Weston's old framebuffer scaled into the
  new mode (squashed text). Under full KMS the kernel owns HDMI and sends real
  hotplug uevents. `config.txt`'s `hdmi_*`
  options are ignored there; `video=` on the kernel command line replaces them.

## On-screen keyboard

**It is weston-keyboard, and Chromium has to ask for it.**
`/usr/libexec/weston-keyboard` ships in the `weston` package (the `clients`
PACKAGECONFIG is on by default), and Weston launches it unprompted -
`text_backend_configuration()` defaults `[input-method] path=` to it. It only
draws when the browser asks for a panel.

The browser and the compositor each own half of it, deliberately split:

* **The browser is put in IME mode unconditionally**, by
  `--enable-wayland-ime --wayland-text-input-version=1` in
  `tessaro-kiosk.service`. Chromium 147 speaks text-input v1 and v3, and
  `kWaylandTextInputV3` is `FEATURE_ENABLED_BY_DEFAULT`, so left alone it binds
  v3, which Weston 13 does not offer. The version switch is only read when
  `--enable-wayland-ime` is also present, and v3 would not help anyway:
  Chromium's v3 client does not support input panel show/hide yet. **These switches are
  the exception to the counterable-from-`KIOSK_CHROMIUM_ARGS_EXTRA` rule** -
  `--disable-wayland-ime` cannot undo them, because `IsImeEnabled()` tests for
  `--enable-wayland-ime` first.
* **Whether a keyboard exists is a compositor decision**, made by
  `tessaro-weston-config` writing `[input-method] path=` (empty) into the
  generated `weston.ini`, or leaving the section out so Weston's default
  applies. With no input method client bound,
  `input_method_context_create()` returns early, no panel surface is ever
  created and `show_input_panel` reaches nothing.

Keeping the flags unconditional is the point: if the IME path itself switched
with the panel, the device would take text input differently - composition,
dead keys - depending on what was plugged in. This way only the panel changes.

`KIOSK_OSK` is `auto` (default), `always` or `never`. `auto` means "no hardware
keyboard attached", and how that is decided matters:

* **udev's `ID_INPUT_KEYBOARD`, and the bus.** systemd's `input_id` builtin
  (`60-input-id.rules`) is the only thing here that tells a full keyboard from
  a device that merely has keys - a power button, a lid switch and a mouse's
  consumer-control endpoint all carry `EV_KEY`. But that tag alone is a trap:
  nearly every x86 board exposes an "AT Translated Set 2 keyboard" through the
  i8042 or the EC with nothing plugged in. Hence USB (`0003`) and Bluetooth
  (`0005`) only, from `/sys/class/input/input*/id/bustype`.
* **Under QEMU the answer is "keyboard attached", and that is right.** runqemu
  boots x86 with `-machine q35,i8042=off -usb -device usb-kbd`, so the guest
  has a real USB keyboard (`QEMU QEMU USB Keyboard`) and `auto` hides the
  panel. Exercising the keyboard under `mise run qemu:vnc` therefore needs
  `tessaro-ctl config set screen.osk=always`; note `qemu:run`/`qemu:vnc` pass
  `-snapshot`, so that does not survive a reboot of the VM.
* **Keyboard-shaped peripherals will fool it.** A barcode scanner, an RFID
  reader or a KVM dongle enumerates as a USB HID keyboard. `screen.osk=always`
  is the answer, which is why that value exists.
* **It fails towards showing the keyboard.** No `udevadm`, an unpopulated udev
  database, anything unexpected: the verdict is "no keyboard" and the panel is
  offered. A superfluous keyboard on screen is a nuisance; a touch-only device
  with no way to type is a brick.
* **It follows hotplug, at the cost of a Weston restart.** The agent re-runs
  the decision when an input device comes or goes, and restarts Weston, taking
  the browser with it, if the verdict changed - see **Display hotplug**. So on
  a device with `auto`, plugging in a keyboard, or a scanner that looks like
  one, costs a page reload. `screen.osk=always`/`never` never restart for it.
* An `[input-method]` section written by hand in `/etc/xdg/weston/weston.ini`
  wins over all of it, the same courtesy `[output]` sections get.

weston-keyboard is a demo client: cairo-drawn, fixed-size keys scaled by the
output scale, QWERTY with shift and symbols, a numeric layout from the field's
content purpose, and a real touch handler. It takes no keyboard grab, so a USB
keyboard keeps working with the panel up.

**The page cannot see it.** Chromium's v1 client keeps only a visible/not
visible bool out of `input_panel_state` and never learns the panel geometry,
the surface is not resized, and `visualViewport` does not change - so a field
near the bottom of the page can sit behind the keyboard with nothing the site
can do about it. A touch-first site should keep its inputs out of the bottom of
the viewport, or bring its own keyboard in the page, where it can reserve the
space. That in-page route is also the only one that can react to a keyboard
being plugged in without restarting anything.

**`tessaro-ctl screen keyboard show|hide` works through the page, because the
panel follows the focused field.** There is no way to raise weston-keyboard
from outside: only the client Weston launched may bind `input_method`
(`text-backend.c`), and the panel appears when Chromium sends
`show_input_panel` for a focused field. So `show` focuses `--selector`, or the
field that has the focus, with `Runtime.evaluate{userGesture}` and calls
`navigator.virtualKeyboard.show()`; `hide` blurs it (`control/page.rs`). Both
refuse when the generated `weston.ini` has an empty `[input-method] path=`:
with `screen.osk=never`, or `auto` with a hardware keyboard plugged in, there
is no keyboard to show until Weston restarts. The page bridge offers the same
as `tessaro.keyboard.show()` and `hide()` (docs/bridge.md).

## Screen power

**`tessaro-ctl screen power off|on` switches the display itself off, not a
black page: the CRTC is disabled and the signal stops.** Weston 13 can do
that (`weston_output_power_off`, `libweston/compositor.c`) but offers it
through no protocol, D-Bus call or signal, so a module of ours does:
`tessaro-power.so`, from `weston-tessaro-power`
(`meta-tessaro-distro/recipes-graphics/wayland/`), loaded by the `--modules=`
in `weston-tessaro-scale.conf.in`. It is built out of tree against Weston's
installed plugin headers (`weston.pc`), so changing it never rebuilds Weston.

* **The module listens on `/run/weston/power.sock`**, in Weston's
  `RuntimeDirectory=`, mode 0600, so only Weston's user and root can reach
  it. One line per connection, `on`, `off` or `status`, answered with the
  state afterwards; the agent's side is `power.rs`.
* **Touch does not wake it.** A forced power-off keeps an output off through
  input, unlike `weston_compositor_sleep()`, whose idle state any touch ends.
  A kiosk that is off for the night stays off when someone taps the glass.
* **A new output is switched off too.** A TV that drops hot-plug detection in
  standby comes back as a new output, which starts powered on; the module
  powers every output created while it is off.
* **The agent puts it back after Weston restarts.** The module's state dies
  with Weston, and a hotplug or a `screen.*` setting restarts it. The agent
  keeps `/run/tessaro-kiosk/screen-off` while the screen is meant to be off,
  and `watch_screen_power` checks every 10s and switches it off again. The
  file is in `/run`, so a reboot always brings the screen back on.
* **A screenshot is refused while it is off**: a powered-off output paints no
  frame, and `Page.captureScreenshot` would wait for one. The VNC mirror
  freezes on the last frame meanwhile.
* **`device status` shows a `screen off` row**, and the page bridge offers the
  same as `tessaro.screen.off()` and `on()`.

## Boot splash and wallpaper

**Both use the welcome page's palette, so the screen goes from boot to page
without a jump in colour**: `#0a0d14` for the field, `#5cc8ff` to `#9d8cff`
for the mark and the bar. Each is a PNG rendered from an SVG beside it by a
script there, and the PNG is what the build uses, so an edited SVG needs the
script run and both committed.

* **The boot splash is psplash, themed at compile time**
  (`meta-tessaro-distro/recipes-core/psplash/`). psplash has no runtime
  theme: the logo comes from `SPLASH_IMAGES`, the colours from
  `psplash-colors.h` and the bar's frame from `base-images/psplash-bar.png`,
  and the bbappend replaces the last two in the source before configure.
  `SPLASH_IMAGES:rpi` is set as well, or `meta-moonforge-raspberrypi`'s
  Moonforge logo wins on the Pi.
* **psplash draws a pixel fully or not at all** (`psplash_fb_draw_image` in
  `psplash-fb.c` tests only whether alpha is non-zero), so the logo and the
  bar's frame carry no alpha: the background colour and the mark's glow are
  painted into the image. The background is one solid colour, the bar itself
  a plain rect in one colour 4px inside the frame, and neither image is
  scaled: the logo is centred at its own size (the `fullscreen`
  `PACKAGECONFIG`), the bar sits at 5/6 of the height (`psplash.c`).
* **The wallpaper is Weston's `background.png`**
  (`meta-tessaro-distro/recipes-graphics/wayland/files/`), a 1920x1920 square
  shown `centered`, so a landscape and a portrait panel both get a crop of it
  at native pixels. In its outer 160px it fades to exactly the
  `background-color` in `weston-init.bbappend`, which fills a larger output,
  so there is no seam. Light noise dithers it because the dark gradients band
  at 8 bits per channel. It must never look blank: a plain wallpaper cannot
  be told apart from an uninitialised framebuffer or an empty browser window.
* **The device's own pages redraw the wallpaper in CSS**: the welcome,
  maintenance and offline pages and the debug screen (`debug.rs`). Its glows
  and dot grid are placed in pixels from the centre of the screen, as
  Weston's `centered` shows the square, so a page appears over the desktop
  without a visible change. The pages stay inline and fetch no image, since
  they are shown when the network is down. The welcome and maintenance pages
  then let the glows drift; the offline page and the debug screen keep them
  still. A change to `background.svg` goes into each of them too.
