SUMMARY = "The Tessaro demo, every kiosk feature on the device's own screen"
DESCRIPTION = "The demo: a React app that shows each feature of the kiosk through \
the page bridge, served by the loopback nginx at http://127.0.0.1/demo/ and \
opened from the welcome page. Built here from demo/ in the repo, offline, with \
the Node the Chromium build already has. See docs/demo.md."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

# The repo root, as tessaro-webconfig has it, so demo/ comes from the
# checkout.
FILESEXTRAPATHS:prepend := "${THISDIR}/../../..:"

# The sources by name, not demo/ whole: a developer's node_modules or a build
# left in the tree must not change what is fetched or hashed. The
# dependencies come from bitbake-lock.json (demo/scripts/bitbake-lock.mjs),
# which npmsw unpacks into a ready node_modules, devDependencies included,
# since the build tools are what is needed.
SRC_URI = " \
    file://demo/src \
    file://demo/public \
    file://demo/index.html \
    file://demo/package.json \
    file://demo/tsconfig.json \
    file://demo/vite.config.ts \
    npmsw://${THISDIR}/../../../demo/bitbake-lock.json;dev=1;destsuffix=demo \
"

S = "${WORKDIR}/demo"
B = "${WORKDIR}/build"

# The Node meta-browser builds for Chromium's own build; nothing new to
# build for this.
DEPENDS = "nodejs-native"

inherit allarch

do_configure[noexec] = "1"

# npmsw runs no install scripts and makes no node_modules/.bin links, so the
# tools run by their paths, as in tessaro-webconfig.
do_compile() {
    export HOME="${WORKDIR}"
    cd ${S}
    node node_modules/typescript/bin/tsc --noEmit
    TESSARO_DEMO_OUT="${B}/dist" node node_modules/vite/bin/vite.js build
}

do_install() {
    # Served at /demo/ by the loopback nginx (10-tessaro-selftest.conf), on
    # the welcome page's origin, so the device grants and the bridge apply.
    install -d ${D}${datadir}/tessaro-demo
    cp -R --no-preserve=ownership ${B}/dist/. ${D}${datadir}/tessaro-demo/
}

FILES:${PN} = "${datadir}/tessaro-demo"

# The nginx that serves it comes with tessaro-selftest, whose server block
# carries the /demo/ location.
RDEPENDS:${PN} = "tessaro-selftest"
