SUMMARY = "v4l2loopback, virtual V4L2 video devices"
DESCRIPTION = "An out-of-tree kernel module for video devices that a program \
writes frames into and any number of others read as a camera. tessaro-camera \
mirrors each USB camera into one, so the browser and anything else can read \
the same camera at once."
HOMEPAGE = "https://github.com/v4l2loopback/v4l2loopback"
# "version 2 of the License, or (at your option) any later version", in
# v4l2loopback.c; COPYING is the GPL 2 text.
LICENSE = "GPL-2.0-or-later"
LIC_FILES_CHKSUM = "file://COPYING;md5=b234ee4d69f5fce4486a80fdaf4a4263"

inherit module

# The v0.15.4 tag. It builds against every kernel these images use (6.6 on
# linux-yocto and linux-raspberrypi); the driver's version checks reach
# 6.18. The project lives at github.com/v4l2loopback, where
# github.com/umlaeute/v4l2loopback redirects.
SRC_URI = "git://github.com/v4l2loopback/v4l2loopback.git;protocol=https;branch=main"
SRCREV = "0f9ee86760b7f2bea174b7e3e7a1d38845da0ab4"

S = "${WORKDIR}/git"

# Its Makefile calls the kernel tree KERNEL_DIR. The module target alone:
# the default one also builds the v4l2loopback-ctl utility, which nothing
# here needs (tessaro-camera talks to /dev/v4l2loopback itself). Its install
# target is the kernel's modules_install, with MODLIB from module.bbclass.
EXTRA_OEMAKE += "KERNEL_DIR=${STAGING_KERNEL_DIR}"
MAKE_TARGETS = "v4l2loopback.ko"
MODULES_INSTALL_TARGET = "install"

# The module package, kernel-module-v4l2loopback, comes from
# kernel-module-split. Loading it, and with which options, is
# tessaro-camera's business (its modules-load.d and modprobe.d).
