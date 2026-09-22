SUMMARY = "Tessaro self-test page: one static page that exercises the kiosk stack"
DESCRIPTION = "A single self-contained HTML page plus its media, installed at \
/usr/share/tessaro-selftest. A technician points KIOSK_URL at it to check rendering, \
fonts, emoji, form inputs, scrolling and multi-touch, WebSerial and WebHID, audio and \
video playback, and WebAudio synthesis - on one screen, with the network down."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# Deliberately its own recipe rather than more files in tessaro-kiosk. That
# recipe inherits cargo and builds tessaro-agent, so anything added to its
# SRC_URI re-hashes do_fetch and drags the whole Rust build along behind every
# edit to this page. Here a change costs one do_install.
SRC_URI = " \
    file://index.html \
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
    install -Dm0644 ${WORKDIR}/index.html ${D}${datadir}/tessaro-selftest/index.html

    # The page refers to these by relative path, so the layout under
    # ${datadir}/tessaro-selftest has to match what index.html asks for.
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
