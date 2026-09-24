SUMMARY = "Tessaro network configuration for NetworkManager"
DESCRIPTION = "The NetworkManager conf.d drop-in that defers DNS to \
systemd-resolved and keeps NM's hands off /etc/resolv.conf, plus the unit that \
keeps NM's runtime state on /data instead of tmpfs. NetworkManager itself comes \
from meta-networking; this recipe only owns the way it is configured."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

SRC_URI = " \
    file://10-tessaro.conf \
    file://tessaro-network-state.service \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    # /usr/lib, not /etc: NM reads conf.d from both and /etc wins, but /etc is
    # the overlay on /data and a file written there can never be taken back by a
    # later image. See the comments in the file itself.
    install -Dm0644 ${WORKDIR}/10-tessaro.conf \
        ${D}${nonarch_libdir}/NetworkManager/conf.d/10-tessaro.conf

    install -Dm0644 ${WORKDIR}/tessaro-network-state.service \
        ${D}${systemd_system_unitdir}/tessaro-network-state.service
}

SYSTEMD_SERVICE:${PN} = "tessaro-network-state.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

# Nothing under ${nonarch_libdir} is in the default FILES:${PN}; the unit itself
# is covered by SYSTEMD_SERVICE.
FILES:${PN} += " \
    ${nonarch_libdir}/NetworkManager/conf.d/10-tessaro.conf \
"

# The configuration is meaningless without the daemon that reads it, and the
# state unit binds a directory that package owns. util-linux-mount rather than
# busybox's mount: the bind is the only thing standing between a technician's
# saved state and a tmpfs, so it should be the same mount the preinit trusts for
# /data. The rest of the stack - nmtui, nmcli, the wifi plugin - is a product
# choice and lives in the image bbappend instead.
RDEPENDS:${PN} = " \
    networkmanager-daemon \
    util-linux-mount \
"

# The hotspot, tessaro-wifi-hotspot, is ipv4.method=shared: NetworkManager
# runs dnsmasq for its DHCP and DNS - found at runtime on NM's fixed search
# path, which is why tessaro.conf can keep the dnsmasq PACKAGECONFIG off -
# and, with firewall-backend=nftables in 10-tessaro.conf, masquerades it with
# nft. The agent's own wifi.nat=0 table goes through nft too. See the dnsmasq
# bbappend for the two things that package must not do here.
RDEPENDS:${PN} += " \
    dnsmasq \
    nftables \
"

# NetworkManager's masquerade table (table ip nm-shared-<iface>) needs the NAT
# and conntrack parts of nftables, which linux-yocto builds as modules and
# qemux86-64 does not install by default. Recommended, not required: on a
# kernel that builds them in, the packages do not exist and nothing is lost.
# The agent's own drop table needs only what CONFIG_NF_TABLES builds in.
RRECOMMENDS:${PN} += " \
    kernel-module-nft-masq \
    kernel-module-nft-nat \
    kernel-module-nft-chain-nat \
    kernel-module-nft-ct \
    kernel-module-nft-reject-inet \
    kernel-module-nft-reject-ipv4 \
"
