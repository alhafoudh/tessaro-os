# Audio

**Sound is PipeWire and WirePlumber, and the audio.* settings are the only
truth about it.** `tessaro-ctl audio output hdmi|jack|usb|bluetooth|auto|off`
(or one output's name from `audio outputs`), `audio volume N`, `audio mute
on|off`, `audio input ...` and `audio input-volume N` are each a `config set`
of one `audio.*` key. The agent applies it to the running sound server at
once: nothing restarts, and a sound already playing moves over. `audio show`
says what the settings resolved to and why, `audio test` plays a 1s tone on
the output in use, `audio test --input` records 3s and prints the level. The
logic is `agent/tessaro-agent/src/audio.rs`; the image side is
`meta-tessaro-distro/recipes-multimedia/tessaro-audio/`.

* **The sound server runs as weston, from units of our own.**
  `tessaro-pipewire`, `tessaro-wireplumber` and `tessaro-pipewire-pulse` are
  system units with `User=weston`, sharing `/run/tessaro-audio` (tmpfiles,
  0700) as their runtime directory. PipeWire's own user units need a logind
  session the image never has, and the recipe's system-wide unit runs as a
  `pipewire` user whose Pulse socket Chromium could not reach. The recipe's
  unit is kept from being enabled by leaving `systemd-system-service` out of
  PipeWire's `PACKAGECONFIG` - in a bbappend, because the recipe sets
  `PACKAGECONFIG:class-target`, and a `:pn-pipewire` value in `tessaro.conf`
  would lose to it silently (`CLASSOVERRIDE` comes after `pn-${PN}` in
  `OVERRIDES`).
* **Chromium plays through the Pulse socket and nothing else.**
  `PULSE_SERVER=unix:/run/tessaro-audio/pulse/native` in
  `tessaro-kiosk.service`, and `libpulse` in the image, which is what makes
  Chromium's dlopened Pulse backend load at all. Chromium is deliberately not in the `audio` group: it cannot
  open a card itself and fight PipeWire for it, and with no sound server it
  plays nothing rather than something unpredictable.
* **Chromium does not rebuild for any of this, and must not.** It is built
  with `use_pulseaudio=true` because `pulseaudio` is a *backfilled*
  `DISTRO_FEATURE`, so `libpulse` costs nothing new. Do not touch
  `DISTRO_FEATURES` or `DISTRO_FEATURES_BACKFILL_CONSIDERED` for sound: that
  flips `use_pulseaudio` and rebuilds Chromium. `bitbake -n
  moonforge-image-base | grep -i chromium` is the check.
* **WirePlumber remembers nothing.** Its drop-in,
  `/usr/share/wireplumber/wireplumber.conf.d/50-tessaro.conf`, turns off
  every restore setting, and the units point `XDG_STATE_HOME` and
  `XDG_CONFIG_HOME` into `/run`. So a volume or default output never
  outlives a factory reset or fights the settings after a reboot. The same
  drop-in turns off what needs a session bus (device reservation), Bluetooth
  (for now) and MIDI.
* **When the agent applies.** On a `set` of an `audio.*` key (the answer
  says where sound now plays: `Applied.audio`); at startup once PipeWire
  answers; when the sound hardware changes and has held still for 2s; and
  once a minute anyway, which puts back anything else that moved it. The
  hardware check reads `/proc/asound/cards`, the DRM connectors' status and
  the PipeWire socket's inode every 2s, which is cheap; PipeWire itself
  (`pw-dump`) is only asked when there is something to apply. Every
  `pw-dump`, `wpctl` and `pw-play` is under `deadline::within`. Applying is
  idempotent and logs `audio: ...` lines only for real changes. A server
  that is not up yet only means the setting is saved and applied later.
* **What `auto` picks.** The USB or Bluetooth output plugged in last
  (PipeWire's object serial, which is never reused), else HDMI with a screen
  connected, else the jack. HDMI counts as connected when its port says so,
  or - on the Pi's vc4-hdmi, which cannot tell - when a DRM connector named
  HDMI or DP is connected. A kind that is not there (`usb` with nothing
  plugged in) is kept and plays on `auto` meanwhile, and `audio show` says
  why; one output *by name* must exist at `set` time, like
  `screen.resolution`. `off` mutes what `auto` would pick.
* **HDMI and analog are often one card.** An Intel HDA card offers them as
  profiles, and only the active profile's outputs exist. Outputs a card has
  only in another profile are listed too (`audio outputs` says it switches
  the card over), and choosing one switches the profile first, preferring a
  profile that keeps the analog input. Tested against a fixture only, not on
  real x86 hardware.
* **Volumes are wpctl's cubic scale**, the one every desktop slider uses:
  50 sounds about half as loud as 100. PipeWire stores the linear value, the
  cube of it.
* **The microphone is granted by policy.** `AudioCaptureAllowedUrls` lists
  the same origins as the serial and HID grants and moves with them
  (`render::ORIGIN_POLICIES`), so `getUserMedia({audio: true})` is answered
  with no prompt. `audio.input=off` mutes the input at PipeWire and leaves
  the grant alone, so it never restarts the browser. The Pi has no audio
  input of its own; a microphone there is a USB one.
* **On the Pi, HDMI sound comes from vc4, not bcm2835.** Under full KMS the
  `vc4-kms-v3d` overlay boots `snd_bcm2835.enable_hdmi=0`, so bcm2835 is the
  headphone jack only, and HDMI is vc4-hdmi's own card, which takes IEC958
  frames only - PipeWire handles that through alsa-lib's `vc4-hdmi.conf`.
  `dtparam=audio=on` is already in meta-raspberrypi's `config.txt`.
