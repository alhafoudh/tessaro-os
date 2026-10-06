# HDMI-CEC

**The agent talks to the TV over the HDMI cable's CEC line: `screen power`
puts the TV in standby and wakes it, the TV's state shows in `tessaro-ctl
screen show`, and what happens on the bus reaches the page and the
scripts.** A TV used for signage often ignores a signal that stops and stays
on, or shows "no signal"; switching the output off is not enough. Everything
here is off until `screen.cec.enable=1`: a monitor has no CEC, and a TV woken
by a device nobody set up for it surprises its owner.

The code is `agent/tessaro-agent/src/cec/` (the kernel interface in
`uapi.rs`, an adapter in `io.rs`, what the agent knows of the bus and
decides from it in `bus.rs`) and the worker in `control/cec.rs`. The
settings are `screen.cec.*` (`tessaro-ctl config keys`).

## Adapters

**The agent uses every `/dev/cecN` that belongs to a DRM connector, through
the kernel's CEC framework.** `CEC_ADAP_G_CONNECTOR_INFO` names the card
and connector id, which `display::connector_name` turns into `HDMI-A-1`
through the connector's `connector_id` in sysfs. An adapter that belongs to
no connector - a capture card's, a USB stick's - is left alone.
`KIOSK_CEC_DEVICES` (colon separated, `paths.rs`) names the adapters
instead; only the e2e suite sets it.

* **The Raspberry Pi has it built in**: meta-raspberrypi's kernel has
  `DRM_VC4_HDMI_CEC`, and the `vc4-kms-v3d` overlay gives every HDMI port an
  adapter.
* **genericx86-64 has CEC tunnelled over DisplayPort only**
  (`tessaro-x86-cec.cfg`, `DRM_DP_CEC`): the HDMI ports that are
  DisplayPort behind an LSPCON chip, common on Intel NUCs and mini PCs, and
  DisplayPort or USB-C to HDMI adapters that wire the CEC pin. A PC's own
  HDMI port drives no CEC line. A Pulse-Eight USB adapter is not supported:
  it needs a serial line discipline attached from userspace before it is an
  adapter.
* **genericarm64 has `DRM_DP_CEC`** in its BSP config, the same as x86.

## The claim

**Each adapter gets a worker that claims a logical address as a playback
device named `screen.cec.name`** (`io::claim`). Changing the name, or
switching CEC off, ends the workers, which give the address up, and starts
them again; the other `screen.cec.*` keys are read at every event, so
nothing restarts for them.

* **The address is claimed without `CEC_LOG_ADDRS_FL_ALLOW_RC_PASSTHRU`.**
  With it the kernel turns the remote's keys into an input device Weston
  reads as a keyboard, past `screen.cec.keys`.
* **`screen.cec.name` is a template**, `{device.name}` by default, filled in
  like the debug screen's and cut to the 14 printable ASCII characters
  `<Set OSD Name>` carries (`protocol::cec::osd_name`, in
  `state::Effective` for `KIOSK_CEC_NAME`). `config set` refuses literal
  text that does not fit already.
* **A TV that drops hot-plug in standby leaves the device no address.** The
  kernel claims again by itself when the physical address comes back, and
  the worker sees it in a state change event. Until then the one message it
  sends is `<Image View On>` from the unregistered address 15, which the
  kernel and the standard both allow, so the wake still reaches the TV.

## Power and the input

**`screen power off` sends `<Standby>` to the TV alone, `on` sends `<Image
View On>`, and with `screen.cec.source` not `off` also broadcasts `<Active
Source>`, which switches the TV to the device's input** (`Bus::wake`,
`Bus::standby`). Standby is never broadcast: that would switch a sound bar
or a receiver off too. `watch_screen_power` switching the output off again
after Weston restarts sends nothing over CEC.

* **The TV wakes once a boot.** The first claim after a boot, while the
  screen is meant to be on, wakes it and writes `/run/tessaro-kiosk/cec-woke`.
  An agent that restarts - a setting, a crash - must not wake a TV someone
  has put in standby since.
* **`screen.cec.source=always` takes the input back** when the TV switches
  to another device while the screen is meant to be on, at most once in 10s
  (`TAKE_BACK_EVERY`), so two devices set that way cannot take it from each
  other in a loop.
* **The agent never switches the TV off on its own.** A TV switched on with
  its remote while the screen is off stays on; a script on `tv-on` can put
  it back.

## Following the bus

**The worker is a follower: it gets every message to the device and every
broadcast, besides those the kernel answers itself** (physical address,
OSD name, vendor, CEC version). It answers `<Give Device Power Status>`
with the screen's state, `<Request Active Source>` and `<Set Stream Path>`
to its address with `<Active Source>`, `<Menu Request>` with "activated",
which is what makes a TV send its remote's keys on, and anything else
directed at it with `<Feature Abort>`.

* **The TV is asked for its power every 10s** (`ASK_POWER_EVERY`): many TVs
  do not say when they are switched on. The TV choosing a source or sending
  a key counts as on, and its `<Standby>` as standby.
* **The bus is polled every 5 minutes** (`SCAN_EVERY`), and once a device
  has an address: every logical address that acknowledges a poll is asked
  for its physical address, name and vendor. `screen show` lists them; a
  vendor is a name for the OUIs in `bus.rs`, else the OUI in hex.
* **A key held down repeats `<User Control Pressed>`**, and a release that
  never comes is assumed after 550ms (`KEY_HELD`), the standard's limit.

## Events

**What changes on the bus is an event: `tv-on`, `tv-standby`,
`source-gained`, `source-lost` and `key`** (`protocol::cec::EVENTS`). A
change is an event only between known states: the TV's first answer is
where the device starts from. Each goes to the journal (keys at debug) and:

* **to the page as `tessaro:cec`** on `window` with `screen.cec.page`, while
  the page has the bridge (docs/bridge.md). The detail has `event`,
  `connector`, `tv` and `showing`, and for a key `key`, `pressed` and
  `repeat`;
* **into the page as a key press with `screen.cec.keys`**: CDP's
  `Input.dispatchKeyEvent` on the page session (`Control::dispatch_key`).
  The map is `protocol::cec::KEYS`: OK and Enter are Enter (with a carriage
  return, so a form submits), Exit is Escape, the arrows, digits, colour
  keys (`ColorF0Red`...), channel up and down as PageUp and PageDown, and
  the media keys. Volume and power stay with the TV;
* **to the scripts that run on it with `screen.cec.scripts`**
  (docs/scripts.md): a key when it goes down, not again while held.

## Testing in qemu

**The e2e suite's `cec` lane runs on vivid's emulated bus**
(`tessaro-qemu-cec.cfg`): vivid's HDMI output and input each register an
adapter on one bus. The lane loads vivid itself, so no other lane sees its
fake capture devices as cameras, points the agent at the output's adapter
with `KIOSK_CEC_DEVICES` in a runtime drop-in, and runs `cec-follower` as
the TV on the input's, which logs what it is sent; `cec-ctl` on the TV's
adapter sends the TV's standby and its remote's keys. What vivid cannot
show - a real TV's quirks, hot-plug in standby, a TV that forwards no keys -
is checked on a Pi by hand.

## What does not work

* **TVs implement CEC loosely.** Some ignore `<Standby>` from a playback
  device, some switch input only on `<Image View On>` with a following
  `<Active Source>`, some forward the remote's keys only to devices they
  recognise. Nothing in the agent works around a particular brand.
* **A device behind an AV receiver** has a physical address below it
  (`1.1.0.0`); the input it takes is the receiver's, which the TV shows as
  long as the receiver passes it on.
