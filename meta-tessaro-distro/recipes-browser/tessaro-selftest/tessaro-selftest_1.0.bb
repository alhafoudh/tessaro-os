SUMMARY = "Tessaro's local pages: the welcome page, maintenance and the player"
DESCRIPTION = "Self-contained HTML pages installed at /usr/share/tessaro-selftest and \
served on the loopback, with the nginx server that serves them and the demo \
(tessaro-demo) at /demo/. index.html is the welcome page a factory image opens: the \
node name, its addresses, whether it is claimed and the way into the demo. \
maintenance.html is the page maintenance mode shows by default. player.html plays \
the playlist the agent writes to /playlist.json."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

# Deliberately its own recipe rather than more files in tessaro-kiosk. That
# recipe inherits cargo and builds tessaro-agent, so anything added to its
# SRC_URI re-hashes do_fetch and drags the whole Rust build along behind every
# edit to this page. Here a change costs one do_install.
SRC_URI = " \
    file://index.html \
    file://maintenance.html \
    file://player/player.html \
    file://player/player.js \
    file://player/player-core.js \
    file://10-tessaro-selftest.conf \
    file://media/sample-video-3s-fullhd.mp4 \
"

# Nothing here is compiled or architecture-specific.
inherit allarch

S = "${WORKDIR}"

do_install() {
    # The welcome page at http://127.0.0.1/, what a factory image opens. Its
    # live values are /welcome.json, which the agent writes and nginx serves.
    install -Dm0644 ${WORKDIR}/index.html ${D}${datadir}/tessaro-selftest/index.html

    # Maintenance mode's default page, TESSARO_MAINTENANCE_URL. Here because
    # this directory is already nginx's root on the loopback, so it is
    # http://127.0.0.1/maintenance.html with no server config of its own.
    install -m0644 ${WORKDIR}/maintenance.html ${D}${datadir}/tessaro-selftest/maintenance.html

    # The playlist player, http://127.0.0.1/player.html, which the agent puts
    # on screen when a playlist plays. Flat in the root, next to the other
    # pages: player.js imports ./player-core.js by relative path. The unit
    # test and the dev/ samples next to them in player/ stay out of the image.
    install -m0644 ${WORKDIR}/player/player.html ${D}${datadir}/tessaro-selftest/player.html
    install -m0644 ${WORKDIR}/player/player.js ${D}${datadir}/tessaro-selftest/player.js
    install -m0644 ${WORKDIR}/player/player-core.js ${D}${datadir}/tessaro-selftest/player-core.js

    # A short local video at http://127.0.0.1/media/, for a playlist that
    # plays with the network down: the e2e playlist lane and the player's
    # dev sample use it.
    install -d ${D}${datadir}/tessaro-selftest/media
    install -m0644 ${WORKDIR}/media/sample-video-3s-fullhd.mp4 \
        ${D}${datadir}/tessaro-selftest/media/

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
# so the welcome page and the demo could never be pre-granted a device. http://127.0.0.1 is
# both a real origin and a potentially trustworthy one, so the grants apply and
# the page is a secure context.
#
# The browser is not named here - it is tessaro-kiosk's dependency - and
# neither are the fonts the page reports on, which are a product-wide decision
# made by the image, not something a diagnostic page should pull in on its own.
RDEPENDS:${PN} = "nginx"
