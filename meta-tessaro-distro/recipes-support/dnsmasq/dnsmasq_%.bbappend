# dnsmasq is in the image for one thing: NetworkManager runs it itself, per
# shared connection, to hand out DHCP and forward DNS on the hotspot
# (ipv4.method=shared). These things the package does on its own would break
# the rest of the device, so they go:
#
# * /etc/systemd/resolved.conf.d/dnsmasq-resolved.conf sets
#   DNSStubListener=no. That switches off systemd-resolved's 127.0.0.53
#   stub, which /etc/resolv.conf points at (dns=systemd-resolved in
#   10-tessaro.conf) - every name lookup on the device would fail, and the
#   hotspot's own dnsmasq, which forwards to resolv.conf, with them.
# * dnsmasq.service, enabled by default, binds port 53 on every address
#   without --bind-interfaces, and NetworkManager's dnsmasq for the hotspot
#   then cannot bind 10.42.0.1:53. The unit stays installed; it is only never
#   enabled.
do_install:append() {
    rm -f ${D}${sysconfdir}/systemd/resolved.conf.d/dnsmasq-resolved.conf
    rmdir --ignore-fail-on-non-empty ${D}${sysconfdir}/systemd/resolved.conf.d 2>/dev/null || true
}

SYSTEMD_AUTO_ENABLE:${PN} = "disable"
