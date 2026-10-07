# Cameras

**Every USB camera is opened by its own mirror and nothing else, and every
other reader has a virtual camera of its own with the same picture.** A V4L2
camera streams to one reader at a time: whoever opens it first, the browser
or a tracker, holds it. A v4l2loopback device does the same. So
`tessaro-camera@<device>.service` opens the camera the moment it is plugged
in and writes every frame into `camera.mirrors` loopback devices, named
`<camera> Mirror 1` and up: one for the kiosk page's `getUserMedia()`,
another for a service on the device, and so on.

```
/dev/video0 (the camera, root 0600) -> tessaro-camera@video0 (its only reader)
                                         -> /dev/video2  "HD Webcam Mirror 1" -> Chromium
                                         -> /dev/video3  "HD Webcam Mirror 2" -> a tracker
```

The mirror is `agent/camera/` (`tessaro-camera`); the image side is
`meta-tessaro-distro/recipes-multimedia/tessaro-camera/` and
`recipes-kernel/v4l2loopback/`. `tessaro-ctl camera list` shows every camera,
what it captures and its mirrors.

## Why v4l2loopback and not PipeWire

**PipeWire shares a camera properly, but Chromium can only reach PipeWire
cameras through xdg-desktop-portal, and our Chromium has no PipeWire at
all.** meta-chromium sets `use_sysroot=false`, which leaves WebRTC's
`rtc_use_pipewire` off (`third_party/webrtc/webrtc.gni`), so
`WebRtcPipeWireCamera` is not compiled in. Turning it on is a full Chromium
rebuild on every machine. And even then Chromium never opens PipeWire's
socket itself: `pipewire_session.cc` connects only through the Camera portal
(`camera_portal.cc`, `org.freedesktop.portal.Camera` on the session bus),
and xdg-desktop-portal needs the `polkit` distro feature and pulls in
flatpak, geoclue, fuse3, bubblewrap and rtkit. A loopback device is an
ordinary V4L2 camera to Chromium, so its own V4L2 capture reads it, with no
rebuild.

## Hiding the real cameras

**`71-tessaro-camera.rules` makes every USB video node root's, 0600, so the
browser cannot open a camera itself.** The kiosk runs as `weston`, which is
in `video`, and it has to stay there: the Pi's hardware video decoder is
`/dev/video10` and up. Those are platform devices, and the rule matches only
nodes with a USB parent (`SUBSYSTEMS=="usb"`), UVC's metadata nodes included.
A node that captures (`ID_V4L_CAPABILITIES` is `:capture:`, from systemd's
`60-persistent-v4l.rules`) also gets `SYSTEMD_WANTS=tessaro-camera@%k.service`,
which starts its mirror; `BindsTo=dev-%i.device` stops it when the camera is
unplugged.

* **No new group for the mirrors.** The image has no `systemd-sysusers`, and
  `/etc/group` lives on the `/etc` overlay: a device whose overlay holds a
  copy would never see a group a later image adds. The mirror runs as root
  instead, with the unit's `DeviceAllow=` and sandboxing narrowing what it
  may touch.
* **The mode alone does not hide it: the rule takes the `uaccess` tag off.**
  systemd's `70-uaccess.rules` tags every video4linux node `uaccess`, and
  `73-seat-late.rules` gives the seat session's user an ACL on it. weston
  runs in that session, so with the tag the kiosk could open the camera
  despite `0600`, Chromium would list it beside the virtual one, and a
  page asking for any camera would get the real one first and fail with
  `NotReadableError`. busybox's `ls` does not mark the ACL; the node shows
  `0660`.
* **The loopback devices are virtual**, so the rule leaves them to udev's
  default `video` group, 0660, which the kiosk reads.

## The mirror

**One process per camera: capture with mmap buffers, `write()` each frame
into every mirror as captured.** Nothing is decoded or converted, so a
mirror costs a copy per frame even on a Pi 3. At start the mirror asks the
camera what it has, picks a mode, adds its loopback devices through
v4l2loopback's control node `/dev/v4l2loopback`, and removes them again when
it stops. It reports to `/run/tessaro-camera/<device>.json`, a
`protocol::CameraInfo`, written whole through a rename, which the agent
reads for `camera list`.

* **One reader per mirror, which is why there are several.** v4l2loopback
  gives a device's capture buffers to one opener at a time
  (`vidioc_reqbufs`: "only exclusive ownership for each stream"); a second
  reader gets `EBUSY` from `VIDIOC_REQBUFS`, and `read()` is refused the
  same way. `camera.mirrors` (1 to 8, 2 by default) is how many may watch a
  camera at once.
* **A mirror's name is `<camera> Mirror N`**, its card label and what a
  page's `enumerateDevices()` shows. `<camera>` is the product name: the
  part of uvcvideo's `Product: Product` card before the colon, cut short so
  the whole fits V4L2's 31 characters. Every mirror is in `video`, so the
  page sees them all; a page that wants one camera picks by label.
* **With camera.presence.enable on, one more: `<camera> Vision`**, root
  0600, which presence detection reads and the page never sees. It is
  reported apart (`CameraInfo.vision`), so `camera.mirrors` and `camera
  list`'s mirrors are still the page's; see **The hidden mirror** in
  [presence.md](presence.md).

* **Exclusive caps, per device.** A loopback device announces capture only
  while something writes to it. Chromium lists only devices that capture, so
  without it a page would see a camera with nothing on it. It is
  `announce_all_caps=0` in the config `V4L2LOOPBACK_CTL_ADD` takes; the
  module's `exclusive_caps` option only covers devices made at load, and
  `modprobe.d` asks for none (`devices=0`).
* **The control node needs no capability**, only opening it: neither ADD nor
  REMOVE checks one in `v4l2loopback_control_ioctl`, and the node is root
  0600. So the unit keeps an empty `CapabilityBoundingSet=`.
* **A virtual camera still open cannot be removed** (the driver answers
  `EBUSY`). The mirror leaves the number of each such mirror in
  `<device>.leftover`, one a line, and the next mirror of that camera
  removes them once their readers let go.
* **A camera that stops sending frames for 10s** makes the mirror exit with
  an error, and systemd restarts it.
* **The camera streams for as long as it is plugged in**, and its light
  stays on. Streaming only while someone reads is a later step.
* **A camera it cannot mirror**, with neither MJPEG nor YUYV, gets a report
  with the reason and no mirrors, and the mirror waits until it is
  stopped rather than failing, so the unit does not restart in a loop.

## Picking a format

**MJPEG where the camera has it, at the largest size up to 1920x1080 that
keeps 25 fps; else YUYV by the same rule.** MJPEG is what reaches those sizes
over USB 2, and passing it through keeps the Pi's CPU free; YUYV at 25 fps is
usually 640x480. A page that asks for less gets Chromium's own downscale; it
cannot get more than the mirror captures.

* **`camera.format`** (`auto`, `mjpeg`, `yuyv`) and **`camera.size`**
  (`auto`, `WIDTHxHEIGHT`) override that, for every camera. A camera without
  the format or the size captures `auto` and says why (`fallback` in its
  report, a warning in `camera list`).
* **They and `camera.mirrors` are applied by restarting the mirrors.** The agent renders them into
  `/run/tessaro-camera/camera.env`, only when the content changes
  (`render::render_camera`), and try-restarts every running
  `tessaro-camera@*.service` (`converge` in `control/settings.rs`). A page
  showing a camera loses its picture for a moment and has to ask for it
  again; the browser and the agent stay. `tessaro-ctl camera format`,
  `camera size` and `camera mirrors` are the shorthands.

## Snapshots

**A snapshot is the mirror's newest frame, taken without a mirror slot, and
only while someone asks.** Every mirror has one reader, so a preview in
`tessaro-ctl camera snapshot`, the GUI or Webconfig must not take one. The
mirror has every frame in hand anyway: while `/run/tessaro-camera/<device>.want`
was touched in the last 5s, it writes the newest good frame to `<device>.jpg`
at most every 500ms (temp file and rename). `GET
/api/v1/camera/{device}/snapshot` touches the want file and answers that
JPEG once it is at most 2s old, waiting up to 3s for the first after a
pause (`camera_snapshot` in `control/mod.rs`). With nobody asking, the
mirror writes nothing and removes its last frame, so no stale picture is
left.

* **"Asked for" is an mtime under 5s old in either direction**, so a clock
  set back cannot leave a want file from the future keeping snapshots on.

* **Nothing streams; a preview polls.** Live previews ask once a second (the
  GUI's and Webconfig's Live, `camera snapshot --watch`), which keeps the
  want file fresh; each viewer costs a JPEG a second over the API.
* **An MJPEG frame is served as captured, with its Huffman tables added.**
  UVC cameras leave out the DHT segment and rely on the standard tables of
  JPEG Annex K; a frame without one gets them inserted before its SOS, or
  browsers and image decoders refuse it (`snapshot.rs`).
* **A YUYV frame is encoded**, with the `jpeg-encoder` crate, only for a
  snapshot: the mirrors still get it raw.
* **The device name is checked** before it goes into a path: a node name
  (`video0`) whose mirror reported it (`want_snapshot` in `camera.rs`).

## The grant

**`VideoCaptureAllowedUrls` grants the camera to the device origins**, like
the microphone's `AudioCaptureAllowedUrls` ([audio.md](audio.md)), and moves
with them (`ORIGIN_POLICIES` in `protocol/src/policy.rs`). The page still
needs a secure context: https, or `http://127.0.0.1`. The self-test page has
a camera preview for checking a device by hand.

## Testing in qemu

**The qemu VM's camera is a USB camera on the host, served over USB/IP.**
`test/usbcam/usbcam.rb` is a UVC camera in Ruby: a USB/IP server whose
device streams ffmpeg's frames of a clip, looped, or of its test pattern.
The guest attaches it with `usbip attach -r 10.0.2.2` through `vhci-hcd`,
and has a USB camera like any other: uvcvideo binds it, the udev rule hides
it and starts its mirror, and the page reads the virtual camera.
`mise run usbcam:run -- [CLIP] --attach` does it for the `qemu:run` or
`qemu:vnc` VM; the e2e camera lane does it for its own ([e2e.md](e2e.md)).
On a Mac, `--live` streams the Mac's built-in camera instead of a clip,
which is how Parallels gives a VM a camera too: an emulated USB camera fed
from AVFoundation.

* **QEMU emulates no camera.** A UVC device for it has been proposed more
  than once and never merged.
* **A `dummy_hcd` gadget cannot stand in for one.** `dummy_hcd.c` fails every
  isochronous transfer, and the UVC gadget (`f_uvc.c`) streams only over an
  isochronous endpoint; its bulk path is a `TODO`.
* **The emulator streams over a bulk endpoint**, which uvcvideo supports,
  so there are no alternate settings to negotiate. It offers MJPEG at
  1280x720 and 640x480 and YUYV at 320x240, all at 30 fps: MJPEG is what
  `auto` picks, and YUYV stays small because slirp carries every byte
  through the host. ffmpeg starts for the mode the guest commits to, and
  the newest frame is what is sent, so a slow reader drops frames rather
  than falling behind.
* **The guest side is in qemux86-64 and genericarm64**: on qemux86-64
  `tessaro-qemu-usbip.cfg` for `usbip-core` and `vhci-hcd`, and
  `usbip-tools` and uvcvideo installed through `MACHINE_EXTRA_RRECOMMENDS`
  in `kas/machine/qemux86-64.yml`; on genericarm64 the modules come from
  `tessaro-genericarm64.cfg` and `usbip-tools` from
  `kas/machine/genericarm64.yml`.
* **genericarm64 reaches the camera only on slirp**, `qemu:run:arm64
  --no-vmnet`: the guest finds the host at `10.0.2.2` and `--attach` goes
  over the `127.0.0.1:2222` forward. On vmnet neither exists, and the server
  listens on `127.0.0.1` alone.
* **`--live` lets AVFoundation drop frames** rather than pacing the stream
  like a clip: it keeps only the newest frame, so a slow reader skips frames
  instead of falling behind. The terminal needs macOS's camera permission.

## What does not work

* **Pi CSI cameras** (Camera Module 2 and 3): they are libcamera only, and
  meta-raspberrypi builds libcamera for vc4 alone, not the Pi 5's pisp.
