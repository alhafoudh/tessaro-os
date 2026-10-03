SUMMARY = "Tessaro's initramfs: apply a pending image update, then find the root filesystem"
DESCRIPTION = "Bundled into the kernel (INITRAMFS_IMAGE_BUNDLE in kas/common/tessaro.yml). \
Modelled on oe-core's core-image-initramfs-boot: initramfs-framework with udev to find \
the root filesystem, plus the one module that makes in-place updates possible - it has to \
run while the root partition is not mounted, which nothing in the root filesystem can do."
LICENSE = "Apache-2.0"

# 01-udev (so /dev/disk/by-* exists), 80-tessaro_update, 90-rootfs, 99-finish.
# finish switch_roots to /sbin/init, which on this image is the overlayfs-etc
# preinit, so /data and the /etc overlay are mounted exactly as before.
INITRAMFS_SCRIPTS = " \
    initramfs-framework-base \
    initramfs-module-udev \
    initramfs-module-rootfs \
    initramfs-module-tessaro-update \
"

PACKAGE_INSTALL = "${INITRAMFS_SCRIPTS} ${VIRTUAL-RUNTIME_base-utils} base-passwd"

# The bare minimum, as core-image-initramfs-boot.
IMAGE_FEATURES = ""
IMAGE_LINGUAS = ""

# Bundled into the kernel, so it must not contain one.
PACKAGE_EXCLUDE = "kernel-image-*"

IMAGE_FSTYPES = "${INITRAMFS_FSTYPES}"
IMAGE_NAME_SUFFIX ?= ""
IMAGE_ROOTFS_SIZE = "8192"
IMAGE_ROOTFS_EXTRA_SPACE = "0"

inherit image
