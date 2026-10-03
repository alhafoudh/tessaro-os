SUMMARY = "Weston module that switches the outputs off and on from outside"
DESCRIPTION = "tessaro-power.so listens on /run/weston/power.sock and powers \
Weston's outputs off or on (weston_output_power_off/on), which Weston 13 \
offers through no protocol of its own. tessaro-agent drives it for \
tessaro-ctl screen power; see docs/display.md."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

# Built out of tree against the installed plugin headers (weston.pc), so a
# change here never rebuilds Weston itself.
DEPENDS = "weston wayland"

inherit meson pkgconfig

SRC_URI = " \
    file://meson.build \
    file://tessaro-power.c \
"

S = "${WORKDIR}"

FILES:${PN} = "${libdir}/weston/tessaro-power.so"

# The module is named in weston.service's --modules
# (weston-tessaro-scale.conf.in); Weston refuses to start without it.
RDEPENDS:${PN} = "weston"
