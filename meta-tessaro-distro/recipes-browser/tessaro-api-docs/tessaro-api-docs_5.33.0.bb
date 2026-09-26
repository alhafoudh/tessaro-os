SUMMARY = "Swagger UI for the device's API"
DESCRIPTION = "Swagger UI, from the swagger-ui-dist npm package, installed at \
/usr/share/tessaro-api/docs and served by the agent at https://<device>:7400/api/docs/ \
against its own /api/v1/openapi.json. See docs/api.md."
HOMEPAGE = "https://github.com/swagger-api/swagger-ui"
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://LICENSE;md5=3b83ef96387f14655fc854ddc3c6bd57"

# The released files, not a build: swagger-ui-dist is what upstream ships
# for serving as-is. A recipe of its own, not a crate (utoipa-swagger-ui
# downloads the same files at build time, which the offline cargo build of
# tessaro-kiosk cannot), and not in tessaro-kiosk's SRC_URI, so a bump never
# re-hashes the Rust build.
SRC_URI = " \
    https://registry.npmjs.org/swagger-ui-dist/-/swagger-ui-dist-${PV}.tgz;subdir=swagger-ui-dist \
    file://swagger-initializer.js \
"
SRC_URI[sha256sum] = "434c69385aa02154348e6dcce0076df3a25ed88f673ac16cf4fed3fcf62c3b1b"

S = "${WORKDIR}/swagger-ui-dist/package"

inherit allarch

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    install -d ${D}${datadir}/tessaro-api/docs
    for file in index.html index.css swagger-ui.css swagger-ui-bundle.js \
            swagger-ui-standalone-preset.js favicon-16x16.png favicon-32x32.png \
            LICENSE NOTICE; do
        install -m0644 ${S}/$file ${D}${datadir}/tessaro-api/docs/
    done
    # Ours: pointed at the device's own document instead of the petstore.
    install -m0644 ${WORKDIR}/swagger-initializer.js ${D}${datadir}/tessaro-api/docs/
}

FILES:${PN} = "${datadir}/tessaro-api/docs"
