# The Tessaro boot splash: the mark on the welcome page's background, with a
# progress bar in its accent colour, in place of Moonforge's logo on
# upstream's beige.
#
# psplash has no runtime theme. The logo is compiled in from SPLASH_IMAGES,
# the colours from psplash-colors.h and the bar's frame from
# base-images/psplash-bar.png, so all of them are replaced in the source before
# it is configured. The PNGs are rendered from the SVGs beside them by
# files/generate.sh, which says why they carry no alpha.
#
# SPLASH_IMAGES has an :rpi override too because meta-moonforge-raspberrypi
# sets its own there (outsuffix=raspberrypi, the package name the Pi image
# installs). This layer has the higher BBFILE_PRIORITY (20 vs 6 and 10), so
# both assignments here are parsed last and win.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SPLASH_IMAGES = "file://tessaro-splash.png;outsuffix=default"
SPLASH_IMAGES:rpi = "file://tessaro-splash.png;outsuffix=raspberrypi"

SRC_URI += " \
    file://psplash-colors.h \
    file://psplash-bar.png \
    file://psplash-tessaro-angle.conf \
"

do_configure:prepend() {
    install -m0644 ${WORKDIR}/psplash-colors.h ${S}/psplash-colors.h
    install -m0644 ${WORKDIR}/psplash-bar.png ${S}/base-images/psplash-bar.png
}

# The splash turns with screen.rotation: psplash's --angle, from a file the
# agent keeps on /data (see the drop-in). systemd.bbclass packages only the
# units it names, so the drop-in directory is listed by hand.
do_install:append() {
    install -Dm0644 ${WORKDIR}/psplash-tessaro-angle.conf \
        ${D}${systemd_system_unitdir}/psplash-start.service.d/20-tessaro-angle.conf
}

FILES:${PN} += "${systemd_system_unitdir}/psplash-start.service.d"
