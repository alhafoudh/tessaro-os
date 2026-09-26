# Product-specific additions on top of the Moonforge base image.

# What the artifacts in tmp/deploy/images/ are called. IMAGE_BASENAME defaults
# to ${PN}, so without this every .wic, .ext4 and .manifest this product ships
# would be named after the upstream recipe it inherits from.
#
# It is the only variable involved: IMAGE_NAME (moonforge-image.bbclass) and
# IMAGE_LINK_NAME (image-artifact-names.bbclass) are both built from it, so the
# versioned file and the stable symlink move together -
# tessaro-os-<machine>-<IMAGE_VERSION>.wic.zst and tessaro-os-<machine>.rootfs.wic.zst.
#
# Deliberately here and not in tessaro.conf: a bare global would put it in
# every recipe's datastore. Set on the image recipe it changes nothing outside
# it - the recipe's own do_rootfs and do_image_* re-run, because IMAGE_NAME and
# IMAGE_LINK_NAME are in do_rootfs[vardeps], and no package is rebuilt.
#
# The bitbake target stays moonforge-image-base; only the output is renamed.
# WKS_FILE is unaffected - image_types_wic.bbclass would derive it from
# IMAGE_BASENAME, but that is a ??= default and every machine here already has
# a real value (our own, from meta-tessaro-distro/wic/).
IMAGE_BASENAME = "tessaro-os"

# The disk image ships as .wic.zst, with a block map next to it so `mise run
# image:flash` and an update write only the blocks in use. zstd because the
# device decompresses an update twice (the dry run, then the initramfs), and
# zstd does that an order of magnitude faster than bz2 at a smaller size.
#
# moonforge-image.bbclass appends "ext4 wic.bz2", and rpi-base.inc's ?=
# default lists wic.bz2 and wic.bmap; the :remove takes out both bz2 entries,
# since :remove is applied after every append. Set on the image recipe, the
# way upstream appends, so only its do_image_wic re-runs. The duplicate
# wic.bmap on the Pi is harmless.
IMAGE_FSTYPES:append = " wic.zst wic.bmap"
IMAGE_FSTYPES:remove = "wic.bz2"

# The size is what goes over the network (base64, see docs/updates.md), and
# decompression speed barely depends on the level; compression runs on every
# core (ZSTD_THREADS). Here and not in tessaro.conf, so no other recipe that
# compresses with zstd changes its hash.
ZSTD_COMPRESSION_LEVEL = "-19"

# Chromium and the CA store arrive as RDEPENDS of tessaro-kiosk, which owns the
# units, the runtime configuration, the tessaro-agent binary and the offline
# page.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-kiosk \
"

# NetworkManager, replacing systemd-networkd (see tessaro.conf for the why and
# for the PACKAGECONFIG side of it).
#
# Named one split package at a time on purpose. The plain "networkmanager"
# package is ALLOW_EMPTY and RRECOMMENDS every plugin that was built - ppp,
# wwan, adsl, ovs, bluetooth, cloud-setup - so installing it would quietly pull
# in a mobile-broadband and VPN stack this device has no use for.
#
# networkmanager-wifi RDEPENDS on wpa-supplicant, which is already in the image
# via the wifi DISTRO_FEATURE. networkmanager-nmtui is the field tool;
# networkmanager-nmcli is the same thing for scripts and for a serial console
# too dumb for curses. tessaro-network carries the daemon's configuration.
CORE_IMAGE_EXTRA_INSTALL += " \
    networkmanager-daemon \
    networkmanager-nmcli \
    networkmanager-nmtui \
    networkmanager-wifi \
    tessaro-network \
"

# Fonts, and this image had almost none. Nothing in this tree ever named a font
# package: the only TTF family present was liberation-fonts, and it arrives by
# accident, as an RRECOMMENDS of the weston recipe. So every generic family a
# stylesheet asks for - serif, sans-serif, monospace - resolved to the same
# Liberation faces, and every emoji anywhere on the kiosk rendered as a
# tofu box.
#
# ttf-noto-emoji-color is NotoColorEmoji.ttf, the CBDT colour font, and it is
# the larger cost here at roughly 10 MB. DejaVu adds about 2 MB and is what
# makes serif and monospace distinguishable from sans at all. Both recipes
# inherit fontcache, so fc-cache runs at rootfs time and Chromium's fontconfig
# fallback finds them with no further configuration.
#
# All of them come from meta-openembedded/meta-oe, which is why layer.conf now
# names openembedded-layer in LAYERDEPENDS.
CORE_IMAGE_EXTRA_INSTALL += " \
    ttf-noto-emoji-color \
    ttf-dejavu-sans \
    ttf-dejavu-serif \
    ttf-dejavu-sans-mono \
"

# The self-test page, which is what a factory image opens: TESSARO_KIOSK_URL
# defaults to http://127.0.0.1/ and a deployment repoints it. About 1.5 MB with
# its media. Worth carrying even on a deployed device - the alternative is a
# technician in front of a black screen with no way to tell a codec from a
# compositor.
#
# nginx arrives as its RDEPENDS rather than being named here, because serving
# the page is that recipe's business: a file:// URL has a null origin and could
# never be granted a serial port by policy. See the comment on
# TESSARO_KIOSK_URL in tessaro.conf.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-selftest \
"

# The setup portal a phone opens from the welcome page's QR code, on the
# hotspot only. See docs/setup-portal.md.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-portal \
"

# Swagger UI for the API, which the agent serves at /api/docs/. See
# docs/api.md.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-api-docs \
"

# Sound: PipeWire, WirePlumber and the Pulse server Chromium plays through,
# with the units that run them as the weston user. Everything about which
# output plays and how loud is the audio.* settings, applied by the agent;
# see docs/audio.md.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-audio \
"

# The clock: timezone data for time.timezone and a saved timesyncd clock that
# survives a reboot. The time.* settings are applied by the agent; see
# docs/time.md.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-time \
"

# Remote access, on every image rather than only development ones.
#
# Until now an SSH server came exclusively from debug-tweaks, which
# meta-moonforge's kas/common/debug.yml adds and a production chain does not
# include - so a shipped device had no way in at all. The VNC screen share is
# bound to 127.0.0.1 (see KIOSK_VNC and tessaro-weston-config), which makes an
# SSH tunnel the only route to it, so without this the feature would exist and
# be unreachable in the field.
#
# allow-empty-password is the one that does the work for dropbear: it adds -B
# to its arguments. empty-root-password on its own only clears the password
# hash, and dropbear refuses a blank password without -B, so they are a
# pair. Development builds get all of this from debug-tweaks as well; stating
# it twice costs nothing.
#
# The empty password is the *unclaimed* state, not a permanent one. The first
# `tessaro-ctl access claim` sets a random root password (shown to that client once),
# and unclaim or a factory reset empties it again - tessaro-agent owns root's
# /etc/shadow entry from the first boot on (agent/tessaro-agent/src/shadow.rs).
# So a fresh or reset device is a root shell with no credential on whatever
# network NetworkManager attaches it to, until someone claims it. An
# authorized_keys story is still the obvious next step.
IMAGE_FEATURES += "ssh-server-dropbear empty-root-password allow-empty-password"

# Tab completion for whoever gets that shell: bash-completion itself, plus the
# -bash-completion package of everything installed (tessaro-ctl, systemctl,
# journalctl, nmcli and the rest). Root's /bin/sh is bash, and completion
# works in the POSIX mode it runs in as sh.
IMAGE_FEATURES += "bash-completion-pkgs"

# No account is created for the VNC login on purpose. It was tried: Weston
# authenticates VNC clients through PAM, and pam_unix can only ever check the
# password of the account the compositor itself runs as - its helper drops the
# setuid it needs to read /etc/shadow when a non-root caller asks about anybody
# else. So a "tessaro" system account was authenticated against by nothing and
# refused every login. The credential lives in the kiosk's environment files
# now and is checked by pam_exec; see recipes-graphics/wayland/weston_%.bbappend.
