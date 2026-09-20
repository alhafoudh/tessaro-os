SUMMARY = "Tessaro kiosk: cog under Weston, with a health watchdog and an offline page"
DESCRIPTION = "systemd units, runtime configuration, watchdog and offline page for the \
Tessaro web kiosk. The browser itself is Igalia's cog, from meta-webkit; this recipe only \
owns the way it is launched and supervised."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

SRC_URI = " \
    file://tessaro-kiosk.service \
    file://tessaro-kiosk-watchdog.service \
    file://tessaro-kiosk-watchdog \
    file://tessaro-kiosk.env.in \
    file://tessaro-kiosk \
    file://tmpfiles-tessaro-kiosk.conf \
    file://tessaro-kiosk-dbus.conf \
    file://offline.html \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

# Build-time default only; tessaro.conf sets the product value.
TESSARO_KIOSK_URL ?= "https://www.moonforgelinux.org"

do_install() {
    install -Dm0644 ${WORKDIR}/tessaro-kiosk.service \
        ${D}${systemd_system_unitdir}/tessaro-kiosk.service
    install -Dm0644 ${WORKDIR}/tessaro-kiosk-watchdog.service \
        ${D}${systemd_system_unitdir}/tessaro-kiosk-watchdog.service

    install -Dm0755 ${WORKDIR}/tessaro-kiosk-watchdog \
        ${D}${bindir}/tessaro-kiosk-watchdog

    # Build-time defaults under /usr/lib, outside the /etc overlay, so a later
    # image can still move them. See the comments in the file itself.
    sed -e "s|@kiosk-url@|${TESSARO_KIOSK_URL}|g" \
        ${WORKDIR}/tessaro-kiosk.env.in > ${WORKDIR}/tessaro-kiosk.env
    install -Dm0644 ${WORKDIR}/tessaro-kiosk.env \
        ${D}${nonarch_libdir}/tessaro-kiosk/tessaro-kiosk.env

    # Runtime override, shipped with every assignment commented out.
    install -Dm0644 ${WORKDIR}/tessaro-kiosk ${D}${sysconfdir}/default/tessaro-kiosk

    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-kiosk.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-kiosk.conf

    # /etc/dbus-1/system.d, not next to cog's own policy in
    # ${datadir}/dbus-1/system.d: dbus parses that directory first and this one
    # second, which is the only ordering guarantee available. See the file.
    install -Dm0644 ${WORKDIR}/tessaro-kiosk-dbus.conf \
        ${D}${sysconfdir}/dbus-1/system.d/tessaro-kiosk.conf

    install -Dm0644 ${WORKDIR}/offline.html \
        ${D}${datadir}/tessaro-kiosk/offline.html
}

SYSTEMD_SERVICE:${PN} = "tessaro-kiosk.service tessaro-kiosk-watchdog.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

# systemd.bbclass only packages the units named in SYSTEMD_SERVICE, and the
# default FILES:${PN} covers neither /usr/lib/tessaro-kiosk nor the tmpfiles and
# D-Bus fragments, so all of it has to be spelled out.
FILES:${PN} += " \
    ${nonarch_libdir}/tessaro-kiosk \
    ${nonarch_libdir}/tmpfiles.d/tessaro-kiosk.conf \
    ${sysconfdir}/dbus-1/system.d/tessaro-kiosk.conf \
    ${datadir}/tessaro-kiosk \
"

CONFFILES:${PN} += "${sysconfdir}/default/tessaro-kiosk"

# curl is not in the image (only libcurl4 is, pulled in by something else), and
# the watchdog's probe needs it. It goes here rather than in the image bbappend
# because it is this recipe's own runtime dependency, not a product package
# choice. busctl comes from systemd: cogctl's own "ping" is broken in 0.18.5,
# see the comment in tessaro-kiosk-watchdog.
RDEPENDS:${PN} = " \
    cog \
    curl \
    busybox \
    systemd \
    dbus \
"
