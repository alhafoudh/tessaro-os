SUMMARY = "Tessaro cameras: a mirror per USB camera, shared through v4l2loopback"
DESCRIPTION = "The udev rule that hides every USB camera node from everyone \
but its mirror and starts one, the tessaro-camera@.service unit that runs it, \
and the v4l2loopback module configuration. The mirror itself, \
/usr/bin/tessaro-camera, is built with the rest of the agent workspace by \
tessaro-kiosk."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://tessaro-camera@.service \
    file://71-tessaro-camera.rules \
    file://tmpfiles-tessaro-camera.conf \
    file://modules-load-v4l2loopback.conf \
    file://modprobe-v4l2loopback.conf \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

# No systemd.bbclass: the unit is a template that udev starts per camera
# (SYSTEMD_WANTS in the rule), so there is nothing to enable.
do_install() {
    install -Dm0644 ${WORKDIR}/tessaro-camera@.service \
        ${D}${systemd_system_unitdir}/tessaro-camera@.service

    install -Dm0644 ${WORKDIR}/71-tessaro-camera.rules \
        ${D}${nonarch_libdir}/udev/rules.d/71-tessaro-camera.rules

    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-camera.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-camera.conf

    # /usr/lib, not /etc, for the reason every shipped default here is:
    # /etc is the overlay on /data, and a file there could never be taken
    # back by a later image.
    install -Dm0644 ${WORKDIR}/modules-load-v4l2loopback.conf \
        ${D}${nonarch_libdir}/modules-load.d/v4l2loopback.conf
    install -Dm0644 ${WORKDIR}/modprobe-v4l2loopback.conf \
        ${D}${nonarch_libdir}/modprobe.d/v4l2loopback.conf
}

FILES:${PN} += " \
    ${systemd_system_unitdir}/tessaro-camera@.service \
    ${nonarch_libdir}/udev/rules.d/71-tessaro-camera.rules \
    ${nonarch_libdir}/tmpfiles.d/tessaro-camera.conf \
    ${nonarch_libdir}/modules-load.d/v4l2loopback.conf \
    ${nonarch_libdir}/modprobe.d/v4l2loopback.conf \
"

# The module the mirrors add their virtual cameras through, and
# tessaro-kiosk, whose package carries /usr/bin/tessaro-camera: cargo
# installs every binary of the agent workspace there.
RDEPENDS:${PN} = " \
    kernel-module-v4l2loopback \
    tessaro-kiosk \
"
