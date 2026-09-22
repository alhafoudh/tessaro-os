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

do_install:append() {
    install -Dm0755 ${WORKDIR}/tessaro-weston-config \
        ${D}${libexecdir}/tessaro-weston-config

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
    # A sed rather than our own copy of weston.ini: the delta is two keys, and
    # forking the file would mean silently dropping whatever Moonforge changes
    # in it next. oe-core's own do_install patches this file the same way.
    sed -i -e '/^\[shell\]/a background-type=centered' \
           -e '/^\[shell\]/a background-color=0xff042120' \
        ${D}${sysconfdir}/xdg/weston/weston.ini
}

# ${libexecdir} is already in the default FILES:${PN}; the drop-in directory is
# not, because systemd.bbclass only packages units named in SYSTEMD_SERVICE.
FILES:${PN} += "${systemd_system_unitdir}/weston.service.d"

# NOTE: do not add "use-pixman" here. It was tried while chasing the blank
# kiosk under QEMU, and it makes things worse rather than better: the pixman
# renderer stops Weston advertising linux-dmabuf, and WPE 2.52 has no wl_shm
# fallback, so the browser fails with "no valid format found" instead. The
# real answer is to give QEMU a working GPU (virtio-vga-gl + virgl), which
# needs /dev/dri on the host - see the "QEMU needs a real GPU" note in
# CLAUDE.md.
