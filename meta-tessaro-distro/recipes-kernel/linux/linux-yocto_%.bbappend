# Kernel options for touch panels and the WebHID/WebSerial/WebBluetooth device
# APIs, the temperature sensors of NVMe and SATA drives (tessaro-sensors.cfg),
# and on genericx86-64 the WiFi drivers linux-yocto leaves out. See the
# comments in each fragment for what each option is for.
#
# The wireless fragment comes after tessaro-devices.cfg, so its =m for btusb
# wins over the =y there. genericx86-64 also gets HDMI-CEC over DisplayPort
# (tessaro-x86-cec.cfg), the CPU's temperature sensors
# (tessaro-x86-sensors.cfg), USB sound cards (tessaro-x86-audio.cfg), and
# what the e2e suite needs when the same image boots under QEMU: USB/IP's
# client for the test camera and scanner the host serves
# (tessaro-qemu-usbip.cfg) and vivid's emulated CEC bus
# (tessaro-qemu-cec.cfg). Those are modules nothing loads unless asked to.
# genericarm64 gets V4L2, the VM sound cards and USB/IP, which its BSP config
# leaves out (tessaro-genericarm64.cfg).
#
# genericx86-64's kernel version is in linux-yocto_6.6.bbappend.
#
# linux-yocto is the kernel on genericx86-64 and genericarm64. raspberrypi3-64
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

SRC_URI += "file://tessaro-devices.cfg file://tessaro-sensors.cfg"
SRC_URI:append:genericx86-64 = " file://tessaro-x86-wireless.cfg file://tessaro-x86-cec.cfg file://tessaro-x86-sensors.cfg file://tessaro-x86-audio.cfg file://tessaro-qemu-usbip.cfg file://tessaro-qemu-cec.cfg"
SRC_URI:append:genericarm64 = " file://tessaro-genericarm64.cfg"
