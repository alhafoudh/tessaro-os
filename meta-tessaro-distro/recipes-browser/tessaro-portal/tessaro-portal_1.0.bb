SUMMARY = "Tessaro's setup portal, for phones on the hotspot"
DESCRIPTION = "The one-page setup portal a phone opens after scanning the welcome \
page's QR code: WiFi, Ethernet, the kiosk page, the device name and timezone, and \
maintenance and the debug screen. The agent serves it with its API on port 7400; \
a dnsmasq drop-in and nginx on port 80 send the phones' captive portal probes \
there, so the sheet opens by itself. See docs/setup-portal.md."
LICENSE = "MIT & OFL-1.1"
LIC_FILES_CHKSUM = " \
    file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302 \
    file://fonts/OFL.txt;md5=a216ac8723e9b95b204d3bc619ebcabd \
"

# Its own recipe, like tessaro-selftest, so a change to the page costs one
# do_install and never touches the cargo build in tessaro-kiosk.
SRC_URI = " \
    file://index.html \
    file://20-tessaro-portal.conf \
    file://tessaro-captive.conf \
    file://tmpfiles-tessaro-portal.conf \
    file://fonts/Manrope-Regular.ttf \
    file://fonts/Manrope-Bold.ttf \
    file://fonts/OFL.txt \
"

inherit allarch

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    install -Dm0644 ${WORKDIR}/index.html ${D}${datadir}/tessaro-portal/index.html

    # Served by the agent (KIOSK_PORTAL_ROOT). Manrope, the font
    # tessaro-gui uses, so the portal looks like it.
    install -d ${D}${datadir}/tessaro-portal/fonts
    install -m0644 ${WORKDIR}/fonts/Manrope-Regular.ttf ${D}${datadir}/tessaro-portal/fonts/
    install -m0644 ${WORKDIR}/fonts/Manrope-Bold.ttf ${D}${datadir}/tessaro-portal/fonts/
    install -m0644 ${WORKDIR}/fonts/OFL.txt ${D}${datadir}/tessaro-portal/fonts/

    # Under /usr/lib, like 10-tessaro-selftest.conf.
    install -Dm0644 ${WORKDIR}/20-tessaro-portal.conf \
        ${D}${nonarch_libdir}/nginx/conf.d/20-tessaro-portal.conf

    # In /etc, the exception to the /usr/lib rule: NetworkManager's
    # shared-mode dnsmasq reads this directory and no other.
    install -Dm0644 ${WORKDIR}/tessaro-captive.conf \
        ${D}${sysconfdir}/NetworkManager/dnsmasq-shared.d/tessaro-captive.conf

    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-portal.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-portal.conf
}

FILES:${PN} = " \
    ${datadir}/tessaro-portal \
    ${nonarch_libdir}/nginx/conf.d/20-tessaro-portal.conf \
    ${sysconfdir}/NetworkManager/dnsmasq-shared.d/tessaro-captive.conf \
    ${nonarch_libdir}/tmpfiles.d/tessaro-portal.conf \
"

# nginx answers the probes and owns the www group the flag's directory is
# given to; tessaro-network brings the hotspot's dnsmasq that reads the
# drop-in.
RDEPENDS:${PN} = " \
    nginx \
    tessaro-network \
"
