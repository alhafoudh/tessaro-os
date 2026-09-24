# Replace Moonforge's wallpaper with the Tessaro one: dark green with scattered
# pale squares. Moonforge ships a plain white background.png, which is
# impossible to tell apart from an uninitialised framebuffer or a blank browser
# window - the exact ambiguity that made the kiosk bring-up hard to read under
# QEMU. The squares keep that property: a blank screen is still obviously blank.
#
# It is a 1920x1920 square, centre-cropped from the source art, so that the same
# file serves a landscape and a portrait panel: whichever way the display is
# turned, the visible window is a crop of the square and the art is never
# stretched. See the background-type sed in do_install:append for the other half
# of this.
#
# The size is also a memory decision. The source art was 10240x5760, which is
# ~236MB once desktop-shell has decoded it - not something the 1GB Pi can spare
# for a wallpaper. 1920x1920 is ~14MB and covers every panel we ship on at
# native pixels.
#
# meta-moonforge-graphics already puts "file://background.png" in SRC_URI and
# installs it. This layer has the higher BBFILE_PRIORITY (20 vs 6), so its
# bbappend is parsed later and this prepend lands ahead of theirs in FILESPATH,
# which is what makes our copy the one that gets picked up.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

# Output scale that follows the panel. Chromium cannot do this itself: its
# --force-device-scale-factor is only honoured when the compositor advertises
# wp_fractional_scale_manager_v1, which Weston 13 does not implement, and the
# switch is documented "TEST ONLY" on Wayland anyway. weston.ini's
# [output] scale= is the only lever, and it matches an exact connector name
# with no wildcard and no command-line equivalent on the DRM backend - so the
# config has to be generated per boot from what is actually plugged in.
#
# ${libexecdir}, not ${nonarch_libdir}/tessaro-kiosk next to the env file it
# reads: that directory belongs to the tessaro-kiosk package, and having two
# packages claim it is the kind of thing that works until it does not.
# /usr/lib/weston is not an option either - that is weston's module directory.
SRC_URI += " \
    file://tessaro-weston-config \
    file://weston-tessaro-scale.conf.in \
"

# Self-signed certificate for the VNC screen share, generated at build time.
#
# Weston 13's VNC backend will not start without one: vnc.c calls
# nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH | NVNC_AUTH_REQUIRE_ENCRYPTION) and
# bails with "requires a key and a certificate for TLS security" if either is
# missing. There is no unencrypted mode and no VNC-standard password auth, so
# the choice is not whether to have a cert but where it comes from.
#
# Build time, shipped read-only, rather than generated on first boot: it keeps
# openssl out of the image and leaves no per-device state to lose, back up or
# renew. The trade is that every device flashed from one image shares this key,
# which is acceptable only because the port is bound to 127.0.0.1 and reached
# through an SSH tunnel that does the real transport security. Widening the
# bind address means revisiting this.
#
# It does make the package non-reproducible - a fresh build with no sstate hit
# mints a new key. Nothing depends on the cert's identity, so that only costs
# re-accepting the fingerprint in the viewer.
DEPENDS += "openssl-native"

do_compile:append() {
    openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
        -subj "/CN=tessaro-kiosk" \
        -keyout ${WORKDIR}/tessaro-vnc-tls.key \
        -out ${WORKDIR}/tessaro-vnc-tls.crt
}

do_install:append() {
    install -Dm0755 ${WORKDIR}/tessaro-weston-config \
        ${D}${libexecdir}/tessaro-weston-config

    # Both world readable, root owned, which for a private key wants saying out
    # loud: this one authenticates nothing. It is self-signed, identical on
    # every device built from this image and only ever presented on a loopback
    # port, so a viewer cannot learn anything from it and neither can anyone
    # who reads it off the rootfs. Its entire job is to satisfy a backend that
    # refuses to start without a key.
    #
    # The alternative, 0640 root:weston, was tried and is not worth it: the
    # weston group's dynamically allocated gid is 1000, the same as the user
    # running bitbake, so every build ends with a host-user-contaminated QA
    # warning about a file that is in fact perfectly fine. A standing false
    # warning costs more than this.
    install -Dm0644 ${WORKDIR}/tessaro-vnc-tls.crt \
        ${D}${libdir}/tessaro-vnc/tls.crt
    install -Dm0644 ${WORKDIR}/tessaro-vnc-tls.key \
        ${D}${libdir}/tessaro-vnc/tls.key

    # Substituted rather than hardcoded, for the same reason oe-core's own
    # do_install runs "sed -e s:/usr/bin:${bindir}:g" over weston.service: if a
    # distro moves either directory, a literal path here would point at a file
    # that is not there, weston would fail to start, and the symptom would be a
    # black screen with a working system behind it.
    sed -e "s|@libexecdir@|${libexecdir}|g" \
        -e "s|@bindir@|${bindir}|g" \
        ${WORKDIR}/weston-tessaro-scale.conf.in > ${WORKDIR}/10-tessaro-scale.conf
    install -Dm0644 ${WORKDIR}/10-tessaro-scale.conf \
        ${D}${systemd_system_unitdir}/weston.service.d/10-tessaro-scale.conf

    # Show the wallpaper at native size, centred, instead of tiled. Neither
    # oe-core's weston.ini nor Moonforge's names background-type, and
    # desktop-shell defaults to "tile" (clients/desktop-shell.c), which would
    # anchor our square at the top-left corner and repeat it - the one layout
    # that looks wrong on both orientations at once. "centered" is the only
    # mode that neither scales nor repeats: it clamps its scale factor to 1.0,
    # so the output gets the middle of the square at native pixels whichever
    # way the panel is turned. ("scale" letterboxes, "scale-crop" covers, and
    # both resample the art on every output that is not exactly square.)
    #
    # background-color is what fills the frame on an output larger than the
    # image in both axes - a 4K panel at scale=1. It is the square's own field
    # colour, so the seam is invisible; ARGB, and the alpha has to be there or
    # the fill is transparent and the frame shows black.
    #
    # A sed rather than our own copy of weston.ini: the delta is these keys, and
    # forking the file would mean silently dropping whatever Moonforge changes
    # in it next. oe-core's own do_install patches this file the same way.
    sed -i -e '/^\[shell\]/a background-type=centered' \
           -e '/^\[shell\]/a background-color=0xff042120' \
        ${D}${sysconfdir}/xdg/weston/weston.ini

    # Drop the [screen-share] section this file inherits from oe-core through
    # Moonforge. Its command= points at rdp-backend.so, which we do not build,
    # and tessaro-weston-config owns that section now: it appends a VNC one to
    # its copy of this file at boot, and - like [output] and [input-method] -
    # leaves any section already present here alone, so that a technician can
    # hand-edit one. Weston honours the *first* matching section, so a stale
    # block here would silently shadow the generated one and screen sharing
    # would fail with "Screen share failed: exec failed".
    sed -i '/^\[screen-share\]/,/^$/d' ${D}${sysconfdir}/xdg/weston/weston.ini
    if grep -q '^\[screen-share\]' ${D}${sysconfdir}/xdg/weston/weston.ini; then
        bbfatal "a [screen-share] section survived in weston.ini; it would shadow the one tessaro-weston-config generates"
    fi
}

# ${libexecdir} is already in the default FILES:${PN}; the drop-in directory is
# not, because systemd.bbclass only packages units named in SYSTEMD_SERVICE.
FILES:${PN} += "${systemd_system_unitdir}/weston.service.d"
FILES:${PN} += "${libdir}/tessaro-vnc"

# NOTE: do not add "use-pixman" here. It was tried while chasing the blank
# kiosk under QEMU, and it makes things worse rather than better: the pixman
# renderer stops Weston advertising linux-dmabuf, and WPE 2.52 has no wl_shm
# fallback, so the browser fails with "no valid format found" instead. The
# real answer is to give QEMU a working GPU (virtio-vga-gl + virgl), which
# needs /dev/dri on the host - see the "QEMU needs a real GPU" note in
# CLAUDE.md.
