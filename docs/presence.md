# Presence detection

**The device finds the faces in front of the screen on one camera and
says when someone arrives, leaves, comes near or steps back: to the
journal, to the scripts that run on it and to the page.** Nothing runs in
the page: `tessaro-vision` reads a hidden mirror of the camera, runs
MediaPipe's BlazeFace on it, and hands each frame's faces to the agent,
which decides what they mean. `tessaro-ctl camera presence on|off` switches
it (camera.presence.enable), `tessaro-ctl camera presence` shows who is
there and how it runs. With camera.presence.demographics on it also
estimates each face's age and gender (**Age and gender** below).

```
tessaro-camera@video0  (no decoding)
  -> /dev/video2 "HD Webcam Mirror 1"  -> the page
  -> /dev/video4 "HD Webcam Vision"    root 0600, presence detection only
       -> tessaro-vision.service
            newest frame each tick -> scaled JPEG decode -> BlazeFace (tract)
            -> faces tracked between frames
            [-> a face not settled yet: larger decode -> FaceRes (tract)]
            -> a datagram each frame to /run/tessaro-kiosk/vision.sock
tessaro-agent
  confidence, distance, facing, arrive / linger / near / classified
  -> journal, scripts (--presence), page (tessaro:presence, tessaro:faces)
```

The vision service is `agent/vision/` (`tessaro-vision`); the agent's side
is `presence.rs` (what the faces mean, pure) and `control/presence.rs` (the
socket, the events, the commands). The unit is
`meta-tessaro-distro/recipes-multimedia/tessaro-camera/files/tessaro-vision.service`.

## Privacy

**No picture is kept and nothing identifies anyone.** The vision service
decodes a frame, finds the faces in it and drops it; it writes no frame
anywhere, and the snapshots of [camera.md](camera.md) are the mirror's,
taken only while a client asks. What leaves it is a box, a score and
keypoints per face. A face's `id` follows a box from frame to frame and is
new when a face comes back: it is never an identity. There is no face
recognition and no emotion.

**Age and gender are estimated only where the owner switches it on**
(camera.presence.demographics, off by default). Each is an estimate from
the face alone, settled once per `id` and forgotten with it; the face
descriptor FaceRes computes on the way is never read, so it never leaves
the process. A screen in a public place that estimates them can fall under
GDPR and the EU AI Act: whether it may be used where the device stands is
the owner's decision, which is why it is a setting and not a default, and
the key's doc, the ctl option and the dialogs all say so.

## The hidden mirror

**While camera.presence.enable is on, every camera's mirror adds one more
virtual camera, `<camera> Vision`, that only root may open.** The page's
mirrors are untouched, so camera.mirrors still says how many readers the
page and other services get, and the page can neither list the Vision
mirror nor take it before the vision service does: v4l2loopback streams a
device to one reader at a time ([camera.md](camera.md), The mirror).

* **udev makes it root's.** `71-tessaro-camera.rules` matches
  `ATTR{name}=="*Vision"` and sets root 0600 without the `uaccess` tag, as
  for the real camera. v4l2loopback copies the label into the node's name
  before it registers it (`v4l2_loopback_add`), so the rule sees it on the
  add event; a camera's own mirrors end in a number and never match.
* **Every camera gets one**, not only the watched one: a mirror cannot tell
  which camera is first, and one more write a frame costs nothing.
* **Switching it on or off restarts the mirrors once**
  (`KIOSK_CAMERA_VISION` in camera.env, Consumer::Camera); a page showing a
  camera asks for it again.

## The vision service

**One process, started and stopped by the agent with
camera.presence.enable**, never by a boot target: the agent starts it at
start when the setting is on (`presence_listen`). It finds the watched
camera in the mirrors' reports (`/run/tessaro-camera/<device>.json`,
`CameraInfo.vision`): `auto` is the one with the lowest node, a name is
matched without case. A report that moves away, or a mirror silent for
10s, makes it look again; a camera without its Vision mirror is said in
its status.

* **At most `camera.presence.fps` frames a second, the newest.** Every
  frame is dequeued; one is kept when it is due, the rest go straight back
  to the driver, so a slow inference never builds a backlog.
* **Decoding is scaled.** An MJPEG frame is decoded at 1/2, 1/4 or 1/8 of
  its size in the IDCT (jpeg-decoder's `scale`), the smallest that still
  covers the model's input: a 1080p frame costs a fraction of a full
  decode. A frame without Huffman tables gets the standard ones first
  (`tessaro_camera::snapshot::with_huffman`). A YUYV frame is sampled.
* **The frame is letterboxed** into the model's square, aspect kept, as
  MediaPipe's `ImageToTensorCalculator` does, and the faces are mapped back
  onto the frame (`picture::Letterbox`).
* **Faces below a floor of 0.3 are dropped** there; the agent applies
  camera.presence.confidence on top, so changing it restarts nothing.
* **Faces are tracked** (`track.rs`): each is matched to the box it
  overlaps most in the frames before and keeps that box's id, smoothed;
  a box missed for a couple of frames keeps its id.
* **Every frame looked at is a datagram**, faces or not: it is how the
  agent knows the service still looks. A datagram never blocks either end.
* **It runs as root with no capabilities**, no network, one core at most
  (`CPUQuota=100%`, `Nice=10`) and only video4linux devices. Its status,
  `/run/tessaro-vision/status.json`, goes with the service
  (`RuntimeDirectory=`), so a stopped one is never taken for a running one.

## The models

**MediaPipe's BlazeFace, the weights `@vladmandic/human` runs in the
browser, converted from MediaPipe's own `.tflite` to ONNX and run by
tract**, pure Rust: no C++ runtime to build in bitbake, and the crates
vendor like any other.

| camera.presence.model | MediaPipe model | Input | Faces up to about |
|---|---|---|---|
| `face-full` | `face_detection_full_range` (Human's blazeface-back) | 192x192 | 5 m |
| `face-short` | `face_detection_short_range` (blazeface-front) | 128x128 | 2 m |

* **The ONNX files are committed** in `agent/vision/models/` and installed
  to `/usr/share/tessaro-vision` by tessaro-kiosk. `mise run vision:models`
  makes them again: `convert.py` fetches the `.tflite` files from
  MediaPipe's v0.8.9 tag, checks their sha256 and runs tf2onnx in a
  throwaway python container.
* **Around the network** (`blazeface.rs`): the anchors, decoding the boxes
  and keypoints and the weighted non-maximum suppression are MediaPipe's
  own, from the graphs that run these models (`SsdAnchorsCalculator`,
  `TensorsToDetectionsCalculator`, `NonMaxSuppressionCalculator`).
* **`tessaro-vision --bench FRAME.jpg...`** runs a model on pictures and
  prints decode and inference times: what to run on a new board.
  `--model face-short` and `--models DIR` pick another model or directory,
  `--demographics` adds each face's age and gender and how long a look
  took.

## Age and gender

**With camera.presence.demographics on, the vision service looks at each
face a few times with HSE FaceRes and settles its age and gender for as
long as the face stays.** FaceRes is the estimator `@vladmandic/human` runs
by default (Apache-2.0, from HSE-asavchenko/HSE_FaceRec_tf), converted from
Human's TFJS graph to `faceres.onnx` by the same `convert.py` and run by
tract like the detector (`demographics.rs`). The setting is
`Consumer::Agent` and `Consumer::Vision`: the vision service restarts to
load the model, the mirrors do not.

* **A face not settled yet is cut out of a larger decode.** The detector's
  picture is a few hundred pixels across, too small for FaceRes's 224x224
  input, so the same frame is decoded again at the scale that gives the
  smallest unsettled face its input's size, whole if that is more than the
  frame has. Settled faces cost nothing, and a frame with none to settle
  is not decoded twice.
* **The square is cut around BlazeFace's box with a margin and turned so
  the eyes are level** (`picture::crop`, `demographics::square`), as Human
  straightens a face before FaceRes. The margin, 1.8 times the box, is the
  one that told the test photos' men and women apart best.
* **A face narrower than 48 pixels in the camera's frame is never looked
  at**: its square would be mostly made up. It stays unsettled.
* **Each face gets a few looks, then settles** (`track.rs`, `LOOKS`): the
  age is their mean, and the gender `male` when the looks average at least
  0.65 likely male, `female` at most 0.35, and `unknown` in between, so a
  face the network is unsure of does not get a coin toss. A settled face
  is never looked at again; a face that comes back gets a new `id` and
  starts over.
* **The age is the expected value of FaceRes's 100 classes**, not Human's
  reading (the likeliest year moved towards a neighbour): the classes peak
  at years ending in 9, so the likeliest year jumps by ten between two looks
  at the same face where the expected value moves by a year or two.
* **Human's other estimators do not work here.** Its SSR-Net gender
  weights (`gender-ssrnet-imdb`) answer about 0.99998 for every input,
  noise included, in Human's own TFJS as in ONNX; its Oarriaga gender model
  turns the same face from woman to man with the crop's margin.
* **TFJS graphs need `TArgs` added** before tf2onnx takes them: Human's
  converter left the attribute off every `_FusedConv2D`, and the TensorFlow
  tf2onnx imports into refuses the node (`with_targs` in `convert.py`). The
  weights are untouched.
* **`faceres.onnx` is larger than Human's file**: the TFJS weights are
  quantised to 16 bits, tf2onnx writes them as 32-bit floats.

## What the faces mean

**The agent decides, from live settings** (`presence.rs`, applied as
`Consumer::Agent`):

* **A face counts at camera.presence.confidence** or above.
* **Distance comes from the face's width**: a face 0.15 m wide spans the
  share of the frame that `2 * d * tan(fov / 2)` gives at a distance `d`,
  with camera.presence.fov the camera's horizontal field of view.
  `tessaro-ctl camera calibrate --distance 1` measures the fov from the one
  face in view of someone standing that far away, and refuses with no face
  or several.
* **Someone arrives** when a face has been in view for
  camera.presence.arrive (a face missed for under a second does not reset
  it), and **leaves** when none has been for camera.presence.linger. Leaving
  is also checked every second without frames, so a vision service that
  stops cannot leave someone present.
* **Near and far** follow the nearest face while someone is present, with
  a margin of a tenth of camera.presence.near so a person on the line does
  not flap; far comes before left. camera.presence.near off has no near
  events.
* **Facing** is the nose between the eyes (`facing`): a guess at a turned
  head, not gaze tracking.
* **Classified** is said once per face whose age and gender settled, once
  someone is present: a face that settles before anyone arrived is said
  with the arrival. A face's `id` is remembered until it has been gone for
  camera.presence.linger and a second, so a vision service that starts over
  and hands out the same `id` again is said again.

## Events

**Each event goes to the journal (`presence: arrived, with 2 face(s) in
view`, `presence: classified face #3 as female, about 34`), then:**

* **to the page as `tessaro:presence`** with camera.presence.page, while
  the page has the bridge ([bridge.md](bridge.md)): `event`, `present`,
  `near`, `count` and the `faces`; with camera.presence.demographics on,
  `genders` (the faces by gender), and for `classified` the `face` that
  settled;
* **to the scripts that run on it** with camera.presence.scripts
  ([scripts.md](scripts.md)): `script create|set --presence
  arrived,left,near,far,classified`, `TESSARO_TRIGGER=presence` and
  `TESSARO_PRESENCE_EVENT`; with camera.presence.demographics on,
  `TESSARO_PRESENCE_MALE`, `_FEMALE` and `_UNKNOWN`, and for `classified`
  `TESSARO_PRESENCE_GENDER` and `TESSARO_PRESENCE_AGE`. They cannot be in
  the run's unit name, so the agent writes them to the run's file in
  `scans/`, as `NAME=value` lines of numbers and fixed words only, and the
  run's shell sources it (`script_env` in `control/presence.rs`,
  `run_shell` in `scripts.rs`).

**A face settled with camera.presence.demographics on carries `age`,
`gender` (`male`, `female` or `unknown`) and `male`, how likely it is a
man's**, everywhere a face goes: `tessaro:faces`, `tessaro:presence`,
`tessaro.presence.status()` and `tessaro-ctl camera presence`. A face not
settled yet has none of them. `device status` and the status add the
counts by gender.

**The faces of every frame reach the page as `tessaro:faces` only while it
watches them** (`tessaro.presence.watch()`): a lease the preamble renews
every few seconds, so a page that navigates away stops them by itself.
Boxes and keypoints are shares of the frame (0 to 1) in the camera's own
view, never mirrored, with the frame's `width` and `height` for its aspect
ratio.

## Testing

* **Unit tests** cover the anchors, decoding and suppression, the
  letterbox, the crop, tracking and settling, and both models on
  `agent/vision/tests/two-faces.jpg` (two portrait photographs, CC0);
  FaceRes on the man of that photo and on `woman.jpg` (CC0); the agent's
  state machine, distance, calibration and `classified`; the script
  trigger and its variables.
* **The e2e `presence` lane** loops a clip made from that photo through the
  fake webcam ([e2e.md](e2e.md)): the hidden mirror and its permissions,
  arriving, near, far and leaving in the journal, a `--presence` script and
  the page, the faces, the refused calibration, age and gender settling
  for the journal, a `classified` script and the page, and switching it
  off.

## What does not work

* **Faces only.** Someone turned away is not there; a body detector is a
  later step.
* **One camera** is watched, camera.presence.camera.
* **Small boards.** It is meant for a Pi 4 and up; on a smaller board,
  `tessaro-vision --bench` shows whether inference keeps up with
  camera.presence.fps.
* **Age and gender are only as good as the face.** The woman of
  `two-faces.jpg`, a small black and white passport photo, comes out
  `unknown`; accuracy at a distance, at an angle or in poor light has not
  been measured. `tessaro-vision --bench --demographics` on pictures from
  the device's own camera is how to find out.
