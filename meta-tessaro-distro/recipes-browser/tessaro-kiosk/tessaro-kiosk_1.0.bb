SUMMARY = "Tessaro kiosk: Chromium under Weston, supervised by tessaro-agent"
DESCRIPTION = "systemd units, runtime configuration, the tessaro-agent supervisor and \
the offline page for the Tessaro web kiosk. The browser itself is Chromium, from \
meta-browser's meta-chromium layer; the agent is the Rust program in agent/ at the root \
of this repo, built here as a native binary. This recipe owns the way both are launched \
and supervised."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# cargo brings do_configure/do_compile and installs the binary from
# ${B}/target; everything else this recipe ships is added by do_install:append
# below. pkgconfig is how openssl-sys finds the target's openssl - cargo_common
# already exports PKG_CONFIG_ALLOW_CROSS.
inherit cargo cargo-update-recipe-crates systemd pkgconfig

# The crate lives at the root of this repo, next to kas/ and mise.toml, so that
# cargo, rust-analyzer and the mise tasks all see an ordinary Rust project.
# Three levels up from this recipe is that root; kas never touches this repo,
# so the path is stable.
FILESEXTRAPATHS:prepend := "${THISDIR}/../../..:"

SRC_URI = " \
    file://agent \
    file://tessaro-kiosk.service \
    file://tessaro-agent.service \
    file://tessaro-config.service \
    file://tessaro-kiosk.env.in \
    file://tessaro-kiosk-policy.json.in \
    file://70-tessaro-devices.rules \
    file://tmpfiles-tessaro-kiosk.conf \
    file://offline.html \
"

# Every crate in Cargo.lock, as crate:// entries with their checksums. Do not
# edit by hand - regenerate with:
#
#     bitbake -c update_crates tessaro-kiosk
#
# after any change to agent/Cargo.lock, and commit the result. do_compile runs
# with --frozen and no network, so this file and the lock file have to agree.
require tessaro-kiosk-crates.inc

S = "${WORKDIR}/agent"

# TLS for the agent's probe. native-tls means the platform's openssl, which is
# the point: the trust store is the image's /etc/ssl/certs (ca-certificates
# below), not a copy of the Mozilla roots baked into the binary.
DEPENDS += "openssl"
export OPENSSL_NO_VENDOR = "1"

# crypt(3), for the SHA-512 root password hash the agent writes into
# /etc/shadow when a device is claimed (agent/tessaro-agent/src/shadow.rs).
DEPENDS += "libxcrypt"

# Build-time default only; tessaro.conf sets the product value.
TESSARO_KIOSK_URL ?= "https://www.moonforgelinux.org"

# The page maintenance mode shows. The default is tessaro-selftest's
# maintenance.html, on the same loopback nginx as the self-test page.
TESSARO_MAINTENANCE_URL ?= "http://127.0.0.1/maintenance.html"

# The same site as an *origin* - scheme, host and port, no path. Chromium's
# device-permission policies match on origin only, and reject the whole policy
# file if a value is not a valid one, so a TESSARO_KIOSK_URL with a path in it
# has to be trimmed rather than pasted through.
TESSARO_KIOSK_ORIGIN ?= "${@'/'.join((d.getVar('TESSARO_KIOSK_URL') or '').split('/')[:3])}"

# Where nginx serves the self-test page. It has to appear in the policy in its
# own right, not only as whatever TESSARO_KIOSK_URL happens to be: on a
# deployed device the kiosk URL is the customer's site, and without this line
# the diagnostic page would lose exactly the grants it exists to exercise.
# Must match the listen address in tessaro-selftest's nginx conf.
TESSARO_SELFTEST_ORIGIN ?= "http://127.0.0.1"

# Both origins, deduplicated and order-stable. On a factory image they are
# the same string and this collapses to one entry.
#
# A plain space-separated list, *not* a ready-made JSON array: this value is
# expanded into the shell of do_install below, and a string carrying its own
# double quotes would have them eaten by the surrounding "..." there, leaving
# an unquoted bareword in the policy. Chromium drops the whole file on a syntax
# error with a single SYSLOG(WARNING), so that failure would be silent. The
# quoting happens in shell, where it can be done safely. An origin cannot
# contain a space, so splitting on one is sound.
def tessaro_device_origins(d):
    origins = []
    for key in ("TESSARO_KIOSK_ORIGIN", "TESSARO_SELFTEST_ORIGIN"):
        value = (d.getVar(key) or "").strip()
        if value and value not in origins:
            origins.append(value)
    return " ".join(origins)

TESSARO_DEVICE_ORIGINS = "${@tessaro_device_origins(d)}"

# The kernel file on the boot partition that an image update replaces. Set per
# machine in the kas config; this default is bootimg-efi's name for a kernel
# with a bundled initramfs (KERNEL_IMAGETYPE-INITRAMFS_LINK_NAME.bin).
TESSARO_KERNEL_FILE ?= "${KERNEL_IMAGETYPE}-initramfs-${MACHINE}.bin"

# tessaro-flash applies an image update from the initramfs, which must not
# pull in the agent, Chromium and everything else ${PN} depends on. cargo
# installs every binary in the workspace into ${bindir}; this takes that one
# into a package of its own, with nothing but glibc behind it.
PACKAGES =+ "${PN}-flash"
FILES:${PN}-flash = "${bindir}/tessaro-flash"

do_install:append() {
    install -Dm0644 ${WORKDIR}/tessaro-kiosk.service \
        ${D}${systemd_system_unitdir}/tessaro-kiosk.service
    install -Dm0644 ${WORKDIR}/tessaro-agent.service \
        ${D}${systemd_system_unitdir}/tessaro-agent.service
    install -Dm0644 ${WORKDIR}/tessaro-config.service \
        ${D}${systemd_system_unitdir}/tessaro-config.service

    # Build-time defaults under /usr/lib, outside the /etc overlay, so a later
    # image can still move them. See the comments in the file itself. There is
    # no /etc/default/tessaro-kiosk any more: a device's settings live in
    # /data/tessaro/state.json and are changed with tessaro-ctl, and the boot
    # oneshot imports a leftover override file once.
    sed -e "s|@kiosk-url@|${TESSARO_KIOSK_URL}|g" \
        -e "s|@maintenance-url@|${TESSARO_MAINTENANCE_URL}|g" \
        -e "s|@selftest-origin@|${TESSARO_SELFTEST_ORIGIN}|g" \
        -e "s|@machine@|${MACHINE}|g" \
        -e "s|@kernel-file@|${TESSARO_KERNEL_FILE}|g" \
        ${WORKDIR}/tessaro-kiosk.env.in > ${WORKDIR}/tessaro-kiosk.env
    install -Dm0644 ${WORKDIR}/tessaro-kiosk.env \
        ${D}${nonarch_libdir}/tessaro-kiosk/tessaro-kiosk.env

    # Chromium enterprise policy. This one path cannot follow the /usr/lib
    # convention above: it is compiled into the binary (policy_paths.cc), and
    # /etc/chromium/policies/managed is where Chromium looks, full stop.
    # JSON-quote the origins here rather than in the bitbake variable - see the
    # comment on tessaro_device_origins above for why that matters.
    #
    # The same file goes to /etc and /usr/lib. The copy in /etc is what Chromium
    # reads on a first boot before anything else has run. The copy in /usr/lib
    # is what tessaro-agent renders the /etc one from, with the device-API
    # origins rewritten for the kiosk URL as set on the device - so the /etc
    # copy is, after the first render, generated, and the /usr/lib one is
    # where its documentation lives.
    device_origins=""
    for origin in ${TESSARO_DEVICE_ORIGINS}; do
        if [ -n "$device_origins" ]; then
            device_origins="$device_origins, "
        fi
        device_origins="$device_origins\"$origin\""
    done

    sed -e "s|@kiosk-origin@|${TESSARO_KIOSK_ORIGIN}|g" \
        -e "s|@device-origins@|$device_origins|g" \
        ${WORKDIR}/tessaro-kiosk-policy.json.in > ${WORKDIR}/10-tessaro.json
    install -Dm0644 ${WORKDIR}/10-tessaro.json \
        ${D}${sysconfdir}/chromium/policies/managed/10-tessaro.json
    install -Dm0644 ${WORKDIR}/10-tessaro.json \
        ${D}${nonarch_libdir}/tessaro-kiosk/policy.json

    # /dev/hidraw* and /dev/bus/usb/* for WebHID and WebUSB. The matching
    # SupplementaryGroups= line is in tessaro-kiosk.service.
    install -Dm0644 ${WORKDIR}/70-tessaro-devices.rules \
        ${D}${nonarch_libdir}/udev/rules.d/70-tessaro-devices.rules

    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-kiosk.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-kiosk.conf

    install -Dm0644 ${WORKDIR}/offline.html \
        ${D}${datadir}/tessaro-kiosk/offline.html
}

SYSTEMD_SERVICE:${PN} = "tessaro-config.service tessaro-kiosk.service tessaro-agent.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

# systemd.bbclass only packages the units named in SYSTEMD_SERVICE, and the
# default FILES:${PN} covers neither /usr/lib/tessaro-kiosk nor the tmpfiles
# fragment, so all of it has to be spelled out.
FILES:${PN} += " \
    ${nonarch_libdir}/tessaro-kiosk \
    ${nonarch_libdir}/tmpfiles.d/tessaro-kiosk.conf \
    ${nonarch_libdir}/udev/rules.d/70-tessaro-devices.rules \
    ${datadir}/tessaro-kiosk \
"

# Ours and in /etc, so marked as configuration. It is only in /etc because
# Chromium's search path leaves no choice, and tessaro-agent regenerates it on
# every boot from the /usr/lib copy, which an image update does move.
CONFFILES:${PN} += " \
    ${sysconfdir}/chromium/policies/managed/10-tessaro.json \
"

# The browser (its recipe's ${PN} is chromium-ozone-wayland, and that package
# carries the /usr/bin/chromium wrapper) and dbus, which is what the agent
# restarts tessaro-kiosk.service through.
#
# ca-certificates is named here rather than inherited: it used to arrive with
# the Moonforge podman layer, which this image no longer includes, and without
# it every https probe fails certificate verification and the device sits on
# the offline page forever.
#
# sfdisk, partx and resize2fs are what `tessaro-ctl storage grow` runs to give
# /data the rest of the disk; none of them is in the image otherwise.
RDEPENDS:${PN} = " \
    chromium-ozone-wayland \
    ca-certificates \
    dbus \
    util-linux-sfdisk \
    util-linux-partx \
    e2fsprogs-resize2fs \
"
