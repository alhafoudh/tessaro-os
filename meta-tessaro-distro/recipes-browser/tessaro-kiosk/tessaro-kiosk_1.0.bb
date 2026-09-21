SUMMARY = "Tessaro kiosk: Chromium under Weston, with a Ruby CDP watchdog and an offline page"
DESCRIPTION = "systemd units, runtime configuration, watchdog and offline page for the \
Tessaro web kiosk. The browser itself is Chromium, from meta-browser's meta-chromium \
layer; the watchdog is a Ruby 4 container image built by `mise run watchdog-image` and \
shipped here as a podman-loadable archive. This recipe only owns the way both are \
launched and supervised."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

# unpack=0 on the OCI archive: it is for podman load, not a source tarball.
# The default unpack class would happily extract it into blobs/ + oci-layout
# in WORKDIR and do_install would find no file to copy.
#
# Note: no # comments inside the continued assignment below - bitbake's line
# continuation ends at the first #, and the following line becomes "unparsed".
SRC_URI = " \
    file://tessaro-kiosk.service \
    file://tessaro-kiosk-watchdog.service \
    file://tessaro-kiosk-watchdog-image.service \
    file://tessaro-kiosk-watchdog-image \
    file://tessaro-kiosk.env.in \
    file://tessaro-kiosk \
    file://tmpfiles-tessaro-kiosk.conf \
    file://offline.html \
    file://tessaro-kiosk-watchdog-image.tar.gz;unpack=0 \
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
    install -Dm0644 ${WORKDIR}/tessaro-kiosk-watchdog-image.service \
        ${D}${systemd_system_unitdir}/tessaro-kiosk-watchdog-image.service

    install -Dm0755 ${WORKDIR}/tessaro-kiosk-watchdog-image \
        ${D}${bindir}/tessaro-kiosk-watchdog-image

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

    install -Dm0644 ${WORKDIR}/offline.html \
        ${D}${datadir}/tessaro-kiosk/offline.html

    # The watchdog container image, loaded into podman's storage on first
    # boot by tessaro-kiosk-watchdog-image.service. Built by
    # `mise run watchdog-image` (docker build + docker save); gitignored.
    install -Dm0644 ${WORKDIR}/tessaro-kiosk-watchdog-image.tar.gz \
        ${D}${datadir}/tessaro-kiosk/tessaro-kiosk-watchdog-image.tar.gz
}

SYSTEMD_SERVICE:${PN} = "tessaro-kiosk.service tessaro-kiosk-watchdog.service tessaro-kiosk-watchdog-image.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

# systemd.bbclass only packages the units named in SYSTEMD_SERVICE, and the
# default FILES:${PN} covers neither /usr/lib/tessaro-kiosk nor the tmpfiles
# fragment, so all of it has to be spelled out.
FILES:${PN} += " \
    ${nonarch_libdir}/tessaro-kiosk \
    ${nonarch_libdir}/tmpfiles.d/tessaro-kiosk.conf \
    ${datadir}/tessaro-kiosk \
"

CONFFILES:${PN} += "${sysconfdir}/default/tessaro-kiosk"

# The browser (its recipe's ${PN} is chromium-ozone-wayland, and that package
# carries the /usr/bin/chromium wrapper), the container runtime that runs the
# watchdog, and container-host-config, whose storage.conf (bbappended by the
# Moonforge podman layer) points podman's graphroot at /data/containers.
# dbus is what the watchdog restarts tessaro-kiosk.service through, and
# busybox runs the first-boot image-load script.
RDEPENDS:${PN} = " \
    chromium-ozone-wayland \
    podman \
    container-host-config \
    dbus \
    busybox \
"
