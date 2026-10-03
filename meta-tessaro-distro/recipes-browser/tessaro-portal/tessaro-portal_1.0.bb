SUMMARY = "Tessaro's captive portal plumbing, for phones on the hotspot"
DESCRIPTION = "What sends a phone that joins the hotspot to Quick Setup: a \
dnsmasq drop-in and nginx on port 80 answer its captive portal probes with a \
redirect to https://10.42.0.1:7400/, where the agent serves Webconfig \
(tessaro-webconfig), so the sign-in sheet opens by itself. See \
docs/quick-setup.md."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

SRC_URI = " \
    file://20-tessaro-portal.conf \
    file://tessaro-captive.conf \
    file://tmpfiles-tessaro-portal.conf \
"

inherit allarch

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
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
    ${nonarch_libdir}/nginx/conf.d/20-tessaro-portal.conf \
    ${sysconfdir}/NetworkManager/dnsmasq-shared.d/tessaro-captive.conf \
    ${nonarch_libdir}/tmpfiles.d/tessaro-portal.conf \
"

# nginx answers the probes and owns the www group the flag's directory is
# given to; tessaro-network brings the hotspot's dnsmasq that reads the
# drop-in; tessaro-webconfig is the page the probes are sent to.
RDEPENDS:${PN} = " \
    nginx \
    tessaro-network \
    tessaro-webconfig \
"
