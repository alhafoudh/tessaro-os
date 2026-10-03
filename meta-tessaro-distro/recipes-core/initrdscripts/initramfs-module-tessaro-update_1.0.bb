SUMMARY = "initramfs-framework module: apply a pending Tessaro image update"
DESCRIPTION = "Runs before the root filesystem is mounted. If tessaro-agent staged an \
image update on /data and marked it, tessaro-flash writes it to the root partition and \
swaps the kernel on the boot partition - or, for a disk update, writes the whole disk \
from a copy in RAM - then the device reboots into it."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

SRC_URI = "file://tessaro-update"

S = "${WORKDIR}"

# A shell script; the machine-specific part is tessaro-flash itself.
inherit allarch

do_install() {
    install -Dm0755 ${WORKDIR}/tessaro-update ${D}/init.d/80-tessaro_update
}

FILES:${PN} = "/init.d/80-tessaro_update"

# The framework's /init, the program that does the work, and mke2fs for
# `--wipe-data`, which re-creates /data from here.
RDEPENDS:${PN} = " \
    initramfs-framework-base \
    tessaro-kiosk-flash \
    e2fsprogs-mke2fs \
"
