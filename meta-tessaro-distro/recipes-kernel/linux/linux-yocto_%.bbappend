# Kernel options for touch panels and the WebHID/WebSerial/WebBluetooth device
# APIs, and on genericx86-64 the WiFi drivers linux-yocto leaves out. See the
# comments in each fragment for what each option is for.
#
# The wireless fragment comes after tessaro-devices.cfg, so its =m for btusb
# wins over the =y there. It is genericx86-64 only: qemux86-64 has no wireless
# NIC and installs no module set. genericx86-64 also gets HDMI-CEC over
# DisplayPort (tessaro-x86-cec.cfg). qemux86-64 gets USB/IP's client instead,
# for the test camera the host serves (tessaro-qemu-usbip.cfg), and vivid's
# emulated CEC bus for the e2e suite (tessaro-qemu-cec.cfg). genericarm64 gets
# V4L2, the VM sound cards and USB/IP, which its BSP config leaves out
# (tessaro-genericarm64.cfg).
#
# linux-yocto is the kernel on qemux86-64, genericx86-64 and genericarm64. raspberrypi3-64
# builds linux-raspberrypi instead, which this bbappend does not touch - check
# meta-raspberrypi's defconfig with
#
#     TESSARO_MACHINE=raspberrypi3-64 mise run image:shell
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
SRC_URI:append:genericx86-64 = " file://tessaro-x86-wireless.cfg file://tessaro-x86-cec.cfg"
SRC_URI:append:qemux86-64 = " file://tessaro-qemu-usbip.cfg file://tessaro-qemu-cec.cfg"
SRC_URI:append:genericarm64 = " file://tessaro-genericarm64.cfg"
