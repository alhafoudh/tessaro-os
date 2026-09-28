SUMMARY = "Tessaro disk: the boot disk's partitions found by label on that disk only"
DESCRIPTION = "A helper that finds a partition by filesystem label on the disk \
/ is mounted from, used by the overlayfs-etc preinit for /data \
(OVERLAYFS_ETC_DEVICE in kas/common/tessaro.yml), and a udev rule that links \
those partitions as /dev/disk/tessaro/<label>. tessaro-disk-boot mounts the \
Raspberry Pi boot partition from that link (docs/build.md, Pi storage boot)."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

SRC_URI = " \
    file://tessaro-disk-part \
    file://61-tessaro-disk.rules \
    file://boot.mount \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    install -Dm0755 ${WORKDIR}/tessaro-disk-part \
        ${D}${libexecdir}/tessaro/tessaro-disk-part
    install -Dm0644 ${WORKDIR}/61-tessaro-disk.rules \
        ${D}${nonarch_base_libdir}/udev/rules.d/61-tessaro-disk.rules
    install -Dm0644 ${WORKDIR}/boot.mount \
        ${D}${systemd_system_unitdir}/boot.mount
}

PACKAGES =+ "${PN}-boot"
FILES:${PN} += "${nonarch_base_libdir}/udev/rules.d"
FILES:${PN}-boot = "${systemd_system_unitdir}/boot.mount"
RDEPENDS:${PN}-boot = "${PN}"

SYSTEMD_PACKAGES = "${PN}-boot"
SYSTEMD_SERVICE:${PN}-boot = "boot.mount"
SYSTEMD_AUTO_ENABLE:${PN}-boot = "enable"
