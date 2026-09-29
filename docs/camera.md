# Cameras

**Every USB camera is opened by its own mirror and nothing else, and
everything else reads the mirror's virtual camera.** A V4L2 camera streams to
one reader at a time: whoever opens it first, the browser or a tracker, holds
it. So `tessaro-camera@<device>.service` opens the camera the moment it is
plugged in and republishes its frames as a v4l2loopback device, which any
number of readers open at once: the kiosk page through `getUserMedia()`, and
anything else on the device.

```
/dev/video0 (the camera, root 0600) -> tessaro-camera@video0 (its only reader)
                                         -> /dev/video50 (v4l2loopback, video 0660)
                                              -> Chromium, a tracker, anything
```

The mirror is `agent/camera/` (`tessaro-camera`); the image side is
`meta-tessaro-distro/recipes-multimedia/tessaro-camera/` and
`recipes-kernel/v4l2loopback/`. `tessaro-ctl camera list` shows every camera,
what it captures and its virtual device.

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
* **The loopback devices are virtual**, so the rule leaves them to udev's
  default `video` group, 0660, which the kiosk reads.

## The mirror

**One process per camera: capture with mmap buffers, `write()` each frame
into the loopback as captured.** Nothing is decoded or converted, so
mirroring MJPEG costs a copy per frame even on a Pi 3. At start the mirror
asks the camera what it has, picks a mode, adds a loopback device through
v4l2loopback's control node `/dev/v4l2loopback` with the camera's own name
as its card label (what a page's `enumerateDevices()` shows), and removes it
again when it stops. It reports to `/run/tessaro-camera/<device>.json`, a
`protocol::CameraInfo`, written whole through a rename, which the agent
reads for `camera list`.

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
  `EBUSY`). The mirror leaves its number in `<device>.leftover`, and the
  next mirror of that camera removes it once the reader lets go.
* **A camera that stops sending frames for 10s** makes the mirror exit with
  an error, and systemd restarts it.
* **The camera streams for as long as it is plugged in**, and its light
  stays on. Streaming only while someone reads is a later step.
* **A camera it cannot mirror**, with neither MJPEG nor YUYV, gets a report
  with the reason and no virtual device, and the mirror waits until it is
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
* **They are applied by restarting the mirrors.** The agent renders both into
  `/run/tessaro-camera/camera.env`, only when the content changes
  (`render::render_camera`), and try-restarts every running
  `tessaro-camera@*.service` (`converge` in `control/settings.rs`). A page
  showing a camera loses its picture for a moment and has to ask for it
  again; the browser and the agent stay. `tessaro-ctl camera format` and
  `camera size` are the shorthands.

## The grant

**`VideoCaptureAllowedUrls` grants the camera to the device origins**, like
the microphone's `AudioCaptureAllowedUrls` ([audio.md](audio.md)), and moves
with them (`ORIGIN_POLICIES` in `protocol/src/policy.rs`). The page still
needs a secure context: https, or `http://127.0.0.1`. The self-test page has
a camera preview for checking a device by hand.

## What does not work

* **Pi CSI cameras** (Camera Module 2 and 3): they are libcamera only, and
  meta-raspberrypi builds libcamera for vc4 alone, not the Pi 5's pisp.
* **qemu**: it has no USB camera to emulate. The e2e lane (`camera_spec.rb`)
  checks the module, `camera list` and `camera.*`; the mirror is checked on a
  Pi with a webcam.
