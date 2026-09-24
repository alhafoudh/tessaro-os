# PipeWire as the kiosk's sound server - see recipes-multimedia/tessaro-audio
# and docs/audio.md.
#
# A bbappend and not a `PACKAGECONFIG:pn-pipewire` in tessaro.conf, because
# the recipe sets PACKAGECONFIG:class-target, and CLASSOVERRIDE comes after
# pn-${PN} in OVERRIDES: the class override would win and a :pn- value would
# be silently ignored. This replaces the recipe's weak default outright.
#
# What is left out, and why:
#   systemd-system-service  the recipe's SYSTEMD_SERVICE follows it, so it
#       would auto-enable a system-wide pipewire.service running as its own
#       `pipewire` user, next to ours. tessaro-audio ships units of its own,
#       as the weston user that Chromium runs as.
#   pulseaudio  only the libpulse *tunnel* module (to a remote PulseAudio).
#       The Pulse server Chromium talks to, pipewire-pulse, is built either
#       way.
#   libcamera, v4l2, gstreamer, jack, webrtc-echo-cancelling, avahi, raop,
#   flatpak, gsettings, libusb, volume, vulkan, ffmpeg  cameras, other audio
#       APIs, network audio and desktop integration; each is dependencies
#       and build time for nothing a kiosk plays.
#
# bluez stays: Bluetooth speakers are the next step (TODO.md), and with the
# codec plugins already built that is configuration, not a rebuild.
# pw-cat is pw-play and pw-record, which the agent's `audio test` uses; they
# need sndfile. readline is only pw-cli's line editing, for a technician.
PACKAGECONFIG:class-target = " \
    alsa \
    bluez \
    bluez-opus \
    pw-cat \
    readline \
    sndfile \
    systemd \
    systemd-user-service \
    udev \
    wireplumber \
"
