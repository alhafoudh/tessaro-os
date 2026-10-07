SUMMARY = "Tessaro cameras: a mirror per USB camera, shared through v4l2loopback"
DESCRIPTION = "The udev rule that hides every USB camera node from everyone \
but its mirror and starts one, the tessaro-camera@.service unit that runs it, \
tessaro-vision.service for presence detection on a camera's hidden mirror, \
and the v4l2loopback module configuration. The mirror and the vision \
service themselves, /usr/bin/tessaro-camera and /usr/bin/tessaro-vision, \
are built with the rest of the agent workspace by tessaro-kiosk."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

SRC_URI = " \
    file://tessaro-camera@.service \
    file://tessaro-vision.service \
    file://71-tessaro-camera.rules \
    file://tmpfiles-tessaro-camera.conf \
    file://modules-load-v4l2loopback.conf \
    file://modprobe-v4l2loopback.conf \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

# No systemd.bbclass: the mirror's unit is a template that udev starts per
# camera (SYSTEMD_WANTS in the rule), and presence detection's is started by
# tessaro-agent with camera.presence.enable, so there is nothing to enable.
do_install() {
    install -Dm0644 ${WORKDIR}/tessaro-camera@.service \
        ${D}${systemd_system_unitdir}/tessaro-camera@.service
    install -Dm0644 ${WORKDIR}/tessaro-vision.service \
        ${D}${systemd_system_unitdir}/tessaro-vision.service

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
    ${systemd_system_unitdir}/tessaro-vision.service \
    ${nonarch_libdir}/udev/rules.d/71-tessaro-camera.rules \
    ${nonarch_libdir}/tmpfiles.d/tessaro-camera.conf \
    ${nonarch_libdir}/modules-load.d/v4l2loopback.conf \
    ${nonarch_libdir}/modprobe.d/v4l2loopback.conf \
"

# The module the mirrors add their virtual cameras through, and
# tessaro-kiosk, whose package carries /usr/bin/tessaro-camera: cargo
# installs every binary of the agent workspace there. The module goes by its
# recipe's package, v4l2loopback, the meta package module.bbclass makes
# depend on kernel-module-v4l2loopback: kernel-module-split names that one
# only at do_package, so bitbake has no provider for it at parse time and
# never builds the recipe.
RDEPENDS:${PN} = " \
    v4l2loopback \
    tessaro-kiosk \
"
