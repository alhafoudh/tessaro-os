SUMMARY = "Tessaro's local pages: the welcome page, the self-test and maintenance"
DESCRIPTION = "Self-contained HTML pages installed at /usr/share/tessaro-selftest and \
served on the loopback. index.html is the welcome page a factory image opens: the \
node name, its addresses and whether it is claimed. selftest.html is the self-test a \
technician points KIOSK_URL at to check rendering, fonts, emoji, form inputs, scrolling \
and multi-touch, WebSerial and WebHID, audio and video playback, and WebAudio synthesis \
- on one screen, with the network down. maintenance.html is the page maintenance mode \
shows by default."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

# Deliberately its own recipe rather than more files in tessaro-kiosk. That
# recipe inherits cargo and builds tessaro-agent, so anything added to its
# SRC_URI re-hashes do_fetch and drags the whole Rust build along behind every
# edit to this page. Here a change costs one do_install.
SRC_URI = " \
    file://index.html \
    file://selftest.html \
    file://maintenance.html \
    file://10-tessaro-selftest.conf \
    file://media/sample-video-3s-fullhd.mp4 \
    file://media/sample-video-3s-4k.mp4 \
    file://media/sample-tone.wav \
    file://media/sample-tone.mp3 \
    file://media/sample-tone.opus \
"

# Nothing here is compiled or architecture-specific.
inherit allarch

S = "${WORKDIR}"

do_install() {
    # The welcome page at http://127.0.0.1/, what a factory image opens. Its
    # live values are /welcome.json, which the agent writes and nginx serves.
    install -Dm0644 ${WORKDIR}/index.html ${D}${datadir}/tessaro-selftest/index.html
    install -m0644 ${WORKDIR}/selftest.html ${D}${datadir}/tessaro-selftest/selftest.html

    # Maintenance mode's default page, TESSARO_MAINTENANCE_URL. Here because
    # this directory is already nginx's root on the loopback, so it is
    # http://127.0.0.1/maintenance.html with no server config of its own.
    install -m0644 ${WORKDIR}/maintenance.html ${D}${datadir}/tessaro-selftest/maintenance.html

    # The page refers to these by relative path, so the layout under
    # ${datadir}/tessaro-selftest has to match what selftest.html asks for.
    install -d ${D}${datadir}/tessaro-selftest/media
    install -m0644 ${WORKDIR}/media/sample-video-3s-fullhd.mp4 \
        ${D}${datadir}/tessaro-selftest/media/
    install -m0644 ${WORKDIR}/media/sample-video-3s-4k.mp4 \
        ${D}${datadir}/tessaro-selftest/media/
    install -m0644 ${WORKDIR}/media/sample-tone.wav ${D}${datadir}/tessaro-selftest/media/
    install -m0644 ${WORKDIR}/media/sample-tone.mp3 ${D}${datadir}/tessaro-selftest/media/
    install -m0644 ${WORKDIR}/media/sample-tone.opus ${D}${datadir}/tessaro-selftest/media/

    # Under /usr/lib rather than /etc/nginx/conf.d, for the reason the file
    # itself explains. The nginx bbappend in this layer is what makes that
    # directory an include path.
    install -Dm0644 ${WORKDIR}/10-tessaro-selftest.conf \
        ${D}${nonarch_libdir}/nginx/conf.d/10-tessaro-selftest.conf
}

FILES:${PN} = " \
    ${datadir}/tessaro-selftest \
    ${nonarch_libdir}/nginx/conf.d/10-tessaro-selftest.conf \
"

# nginx, because the page is served over http rather than opened as a file.
# That is not a preference: a file:// page has a null origin, and Chromium's
# SerialAllowAllPortsForUrls and WebHidAllowAllDevicesForUrls match on origin,
# so the self-test could never be pre-granted a device. http://127.0.0.1 is
# both a real origin and a potentially trustworthy one, so the grants apply and
# the page is a secure context.
#
# The browser is not named here - it is tessaro-kiosk's dependency - and
# neither are the fonts the page reports on, which are a product-wide decision
# made by the image, not something a diagnostic page should pull in on its own.
RDEPENDS:${PN} = "nginx"
