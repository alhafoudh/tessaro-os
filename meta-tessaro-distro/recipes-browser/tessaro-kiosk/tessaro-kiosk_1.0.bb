SUMMARY = "Tessaro kiosk: Chromium under Weston, supervised by tessaro-agent"
DESCRIPTION = "systemd units, runtime configuration, the tessaro-agent supervisor and \
the offline page for the Tessaro web kiosk. The browser itself is Chromium, from \
meta-browser's meta-chromium layer; the agent is the Rust program in agent/ at the root \
of this repo, built here as a native binary. This recipe owns the way both are launched \
and supervised."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

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
    file://frame-unlock/manifest.json \
    file://frame-unlock/rules.json \
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
# maintenance.html, on the same loopback nginx as the welcome page.
TESSARO_MAINTENANCE_URL ?= "http://127.0.0.1/maintenance.html"

# The same site as an *origin* - scheme, host and port, no path. Chromium's
# device-permission policies match on origin only, and reject the whole policy
# file if a value is not a valid one, so a TESSARO_KIOSK_URL with a path in it
# has to be trimmed rather than pasted through.
TESSARO_KIOSK_ORIGIN ?= "${@'/'.join((d.getVar('TESSARO_KIOSK_URL') or '').split('/')[:3])}"

# Where nginx serves the welcome page and the demo. It has to appear in the
# policy in its own right, not only as whatever TESSARO_KIOSK_URL happens to
# be: on a deployed device the kiosk URL is the customer's site, and without
# this line the demo would lose exactly the grants it exists to show.
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

# Where the Raspberry Pi firmware's config.txt is, on a machine that boots
# through that firmware; empty everywhere else. The agent writes the settings
# the firmware reads (tessaro.txt) there, and offers those settings only when
# this is set. Set per machine in the kas config.
TESSARO_BOOT_CONFIG_DIR ?= ""

# tessaro-flash applies an image update from the initramfs, which must not
# pull in the agent, Chromium and everything else ${PN} depends on. cargo
# installs every binary in the workspace into ${bindir}; this takes that one
# into a package of its own, with nothing but glibc behind it.
PACKAGES =+ "${PN}-flash"
FILES:${PN}-flash = "${bindir}/tessaro-flash"

# tessaro-ctl's bash completion, installed by the image's bash-completion-pkgs
# feature. Spelled out rather than `inherit bash-completion`: that class also
# adds bash-completion to DEPENDS, which would re-run the whole Rust build for
# a package that needs nothing at compile time.
PACKAGES =+ "${PN}-bash-completion"
FILES:${PN}-bash-completion = "${datadir}/bash-completion"
RDEPENDS:${PN}-bash-completion = "bash-completion"

do_install:append() {
    install -Dm0644 ${WORKDIR}/tessaro-kiosk.service \
        ${D}${systemd_system_unitdir}/tessaro-kiosk.service
    install -Dm0644 ${WORKDIR}/tessaro-agent.service \
        ${D}${systemd_system_unitdir}/tessaro-agent.service
    install -Dm0644 ${WORKDIR}/tessaro-config.service \
        ${D}${systemd_system_unitdir}/tessaro-config.service

    # Build-time defaults under /usr/lib, outside the /etc overlay, so a later
    # image can still move them. See the comments in the file itself. There is
    # no /etc/default/tessaro-kiosk: a device's settings live in
    # /data/tessaro/tessaro.db and are changed with tessaro-ctl.
    sed -e "s|@kiosk-url@|${TESSARO_KIOSK_URL}|g" \
        -e "s|@maintenance-url@|${TESSARO_MAINTENANCE_URL}|g" \
        -e "s|@selftest-origin@|${TESSARO_SELFTEST_ORIGIN}|g" \
        -e "s|@machine@|${MACHINE}|g" \
        -e "s|@kernel-file@|${TESSARO_KERNEL_FILE}|g" \
        -e "s|@boot-config-dir@|${TESSARO_BOOT_CONFIG_DIR}|g" \
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

    # The face detectors tessaro-vision runs (KIOSK_VISION_MODELS), from the
    # workspace this recipe builds anyway: agent/vision/models, converted from
    # MediaPipe's with `mise run vision:models`. The unit that runs it is
    # tessaro-camera's, beside the mirrors whose hidden mirror it reads.
    install -d ${D}${datadir}/tessaro-vision
    install -m0644 ${S}/vision/models/*.onnx ${D}${datadir}/tessaro-vision/

    # The frame unlock extension, loaded by tessaro-kiosk.service through
    # KIOSK_EXTENSION_ARGS. Rules only, no code: frames the player page at
    # http://127.0.0.1 opens get their X-Frame-Options and CSP headers
    # removed, so a playlist can show a site that forbids framing. Nothing
    # else the browser loads is touched.
    install -Dm0644 ${WORKDIR}/frame-unlock/manifest.json \
        ${D}${datadir}/tessaro-kiosk/frame-unlock/manifest.json
    install -m0644 ${WORKDIR}/frame-unlock/rules.json \
        ${D}${datadir}/tessaro-kiosk/frame-unlock/rules.json

    # bash-completion loads this file on the first Tab after `tessaro-ctl`. It
    # asks the binary for its script rather than shipping a generated copy:
    # the target binary cannot run on the build host, and this way the
    # completion can never fall behind the commands. Written here, not added
    # to SRC_URI, because anything in SRC_URI re-hashes do_fetch and with it
    # the whole Rust build.
    install -d ${D}${datadir}/bash-completion/completions
    printf '%s\n' \
        '# The script is generated by the binary itself, so it always matches it.' \
        'eval "$(tessaro-ctl completion bash 2>/dev/null)"' \
        > ${D}${datadir}/bash-completion/completions/tessaro-ctl
    chmod 0644 ${D}${datadir}/bash-completion/completions/tessaro-ctl

    # drivetemp gives a SATA disk's temperature to hwmon, which `status`
    # reads (docs/hardware.md). It has no modalias, so nothing loads it on
    # its own. Written here rather than added to SRC_URI, for the same reason
    # as the completion above.
    install -d ${D}${nonarch_libdir}/modules-load.d
    printf '%s\n' \
        '# SATA disk temperatures for tessaro-agent (docs/hardware.md).' \
        'drivetemp' \
        > ${D}${nonarch_libdir}/modules-load.d/tessaro-drivetemp.conf
    chmod 0644 ${D}${nonarch_libdir}/modules-load.d/tessaro-drivetemp.conf
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
    ${nonarch_libdir}/modules-load.d/tessaro-drivetemp.conf \
    ${datadir}/tessaro-kiosk \
    ${datadir}/tessaro-vision \
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
#
# systemd-analyze checks a schedule's OnCalendar expressions with systemd's
# own parser before `tessaro-ctl schedule` saves them (docs/scheduler.md);
# systemd splits it into a package of its own.
RDEPENDS:${PN} = " \
    chromium-ozone-wayland \
    ca-certificates \
    dbus \
    util-linux-sfdisk \
    util-linux-partx \
    e2fsprogs-resize2fs \
    systemd-analyze \
"

# The module modules-load.d/tessaro-drivetemp.conf loads. A recommendation,
# not a dependency: genericx86-64 brings every module already, and qemu,
# which installs none on its own, still needs this one.
RRECOMMENDS:${PN} += "kernel-module-drivetemp"
