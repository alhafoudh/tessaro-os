# Make the kiosk URL changeable on a running device, and make the unit survive
# Weston coming up slowly.
#
# Upstream seds WPE_SIMPLE_LAUNCHER_URL into wpe-simple-launcher.service at
# do_compile, so the URL ends up as a literal in the unit and can only be
# changed by rebuilding. Tessaro adds a drop-in that carries the build-time
# default in Environment= and lets /etc/default/tessaro-kiosk override it.
#
# The default deliberately lives in the drop-in under /lib rather than in /etc:
# /etc is an overlayfs upper on /data, so anything written there shadows the
# image copy permanently and later images could never move the default again.

FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

# The QEMU blank-screen problem (WPEWebProcess being refused a GBM buffer on
# /dev/dri/card0, because only the DRM master may create dumb buffers) is fixed
# on the compositor side instead - see the use-pixman PACKAGECONFIG in
# recipes-graphics/wayland/weston-init.bbappend. WEBKIT_DISABLE_DMABUF_RENDERER
# does not exist in WPE 2.52 and setting it here achieved nothing.

SRC_URI += " \
    file://tessaro-kiosk \
    file://10-tessaro-kiosk.conf.in \
"

do_install:append() {
    install -Dm644 ${WORKDIR}/tessaro-kiosk ${D}${sysconfdir}/default/tessaro-kiosk

    sed -e "s|@kiosk-url@|${WPE_SIMPLE_LAUNCHER_URL}|g" \
        ${WORKDIR}/10-tessaro-kiosk.conf.in > ${WORKDIR}/10-tessaro-kiosk.conf
    install -Dm644 ${WORKDIR}/10-tessaro-kiosk.conf \
        ${D}${systemd_system_unitdir}/wpe-simple-launcher.service.d/10-tessaro-kiosk.conf
}

# systemd.bbclass only packages the units named in SYSTEMD_SERVICE, not a
# .service.d/ directory, so this has to be spelled out.
FILES:${PN} += "${systemd_system_unitdir}/wpe-simple-launcher.service.d"

CONFFILES:${PN} += "${sysconfdir}/default/tessaro-kiosk"
