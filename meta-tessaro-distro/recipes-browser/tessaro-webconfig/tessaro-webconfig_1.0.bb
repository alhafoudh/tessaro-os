SUMMARY = "Webconfig, the device's management pages in a browser"
DESCRIPTION = "Webconfig: Quick Setup and everything tessaro-ctl and tessaro-gui \
manage, as a React app the agent serves with its API on port 7400. Built here \
from webconfig/ in the repo, offline, with the Node the Chromium build already \
has. See docs/webconfig.md."
LICENSE = "MIT & OFL-1.1"
LIC_FILES_CHKSUM = " \
    file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302 \
    file://src/fonts/OFL.txt;md5=a216ac8723e9b95b204d3bc619ebcabd \
"

# The repo root, as tessaro-kiosk has it, so webconfig/ and the API's
# document come from the checkout.
FILESEXTRAPATHS:prepend := "${THISDIR}/../../..:"

# The sources by name, not webconfig/ whole: a developer's node_modules or
# a build left in the tree must not change what is fetched or hashed. The
# API's document sits at the same ../agent/protocol path as in the repo, so
# the codegen finds it the same way. The dependencies come from
# bitbake-lock.json, package-lock.json filtered to what npmsw can fetch and
# a Linux build host runs (webconfig/scripts/bitbake-lock.mjs); npmsw
# unpacks them into a ready node_modules, devDependencies included, since
# the build tools are what is needed.
SRC_URI = " \
    file://webconfig/src \
    file://webconfig/index.html \
    file://webconfig/package.json \
    file://webconfig/tsconfig.json \
    file://webconfig/vite.config.ts \
    file://webconfig/dev-proxy.ts \
    file://agent/protocol/openapi.json \
    npmsw://${THISDIR}/../../../webconfig/bitbake-lock.json;dev=1;destsuffix=webconfig \
"

S = "${WORKDIR}/webconfig"
B = "${WORKDIR}/build"

# The Node meta-browser builds for Chromium's own build; nothing new to
# build for this.
DEPENDS = "nodejs-native"

inherit allarch

do_configure[noexec] = "1"

# npmsw runs no install scripts and makes no node_modules/.bin links, so the
# tools run by their paths. esbuild, rollup, lightningcss and Tailwind's
# oxide find their binaries in their per-platform packages without scripts.
do_compile() {
    export HOME="${WORKDIR}"
    cd ${S}
    node node_modules/openapi-typescript/bin/cli.js ../agent/protocol/openapi.json -o src/api/schema.d.ts
    node node_modules/typescript/bin/tsc --noEmit
    TESSARO_WEBCONFIG_OUT="${B}/dist" node node_modules/vite/bin/vite.js build
}

do_install() {
    # Served by the agent at / (KIOSK_WEBCONFIG_ROOT).
    install -d ${D}${datadir}/tessaro-webconfig
    cp -R --no-preserve=ownership ${B}/dist/. ${D}${datadir}/tessaro-webconfig/
    install -m0644 ${S}/src/fonts/OFL.txt ${D}${datadir}/tessaro-webconfig/OFL.txt
}

FILES:${PN} = "${datadir}/tessaro-webconfig"
