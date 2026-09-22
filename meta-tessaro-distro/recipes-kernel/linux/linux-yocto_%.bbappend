# Kernel options for touch panels and the WebHID/WebSerial/WebBluetooth device
# APIs. See the comments in the fragment for what each one is for.
#
# linux-yocto is the kernel on qemux86-64 and genericx86-64. raspberrypi3-64
# builds linux-raspberrypi instead, which this bbappend does not touch - check
# meta-raspberrypi's defconfig with
#
#     TESSARO_MACHINE=raspberrypi3-64 mise run shell
#     bitbake -e linux-raspberrypi | grep '^KERNEL_FEATURES='
#
# before assuming the Pi is covered. It usually is, on-board Bluetooth being a
# supported feature of that BSP.
#
# kernel-yocto picks .cfg files out of SRC_URI and merges them over the
# defconfig, warning about anything the final config did not honour - so a
# symbol silently dropped by a dependency shows up in the do_kernel_configcheck
# log rather than only at runtime.
#
# %-suffixed so this survives a kernel version bump; the recipe file name
# carries the version.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += "file://tessaro-devices.cfg"
