#!/bin/sh
# The network several qemu devices share on a Linux host, so they see each
# other and the host sees them over mDNS: what `mise run qemu:run --count N`
# and `qemu:vnc --count N` build under sudo for the length of a run
# (scripts/qemu-devices.sh, docs/build.md "Several devices").
#
#   sudo scripts/qemu-net.sh up COUNT USER
#   sudo scripts/qemu-net.sh down
#
# up makes the bridge tessaro0 (10.77.77.1/24) with a tap per device,
# tessaro-tap1 .. tessaro-tapCOUNT, owned by USER so QEMU opens them without
# root - from inside the kas container too, which shares the host's network.
# busybox udhcpd hands out .100-.200 with the host as router and its DNS
# servers, and the host masquerades the bridge out to the world. mDNS needs
# nothing more: it is multicast on the one bridge, with snooping off so every
# port gets it.
#
# Docker sets the FORWARD policy to DROP and loads br_netfilter, which sends
# even frames between two taps on the bridge through FORWARD, and a host
# firewall such as ufw drops on INPUT, so the bridge gets ACCEPT rules of its
# own in both. Every rule carries the comment tessaro-qemu,
# which is how down finds exactly them again. down undoes all of up and is
# safe to run twice, or after a run that was killed.
set -eu

BRIDGE=tessaro0
TAP=tessaro-tap
NET=10.77.77
TAG=tessaro-qemu
STATE="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)/build/qemu-devices"

[ "$(id -u)" = 0 ] || { echo "$0: run it with sudo" >&2; exit 1; }

up() {
    count=$1
    owner=$2
    mkdir -p "$STATE"

    ip link add "$BRIDGE" type bridge
    echo 0 > "/sys/class/net/$BRIDGE/bridge/multicast_snooping"
    ip addr add "$NET.1/24" dev "$BRIDGE"
    ip link set "$BRIDGE" up
    i=1
    while [ "$i" -le "$count" ]; do
        ip tuntap add dev "$TAP$i" mode tap user "$owner"
        ip link set "$TAP$i" master "$BRIDGE" up
        i=$((i + 1))
    done

    cat /proc/sys/net/ipv4/ip_forward > "$STATE/ip_forward"
    echo 1 > /proc/sys/net/ipv4/ip_forward
    iptables -t nat -A POSTROUTING -s "$NET.0/24" ! -o "$BRIDGE" -j MASQUERADE \
        -m comment --comment "$TAG"
    # A host firewall (ufw's INPUT policy is DROP) would drop the devices'
    # DHCP requests and their mDNS answers to the host.
    iptables -I INPUT 1 -i "$BRIDGE" -j ACCEPT -m comment --comment "$TAG"
    iptables -I FORWARD 1 -i "$BRIDGE" -j ACCEPT -m comment --comment "$TAG"
    iptables -I FORWARD 1 -o "$BRIDGE" -m conntrack --ctstate RELATED,ESTABLISHED \
        -j ACCEPT -m comment --comment "$TAG"

    # The host's own DNS servers: systemd-resolved's stub on 127.0.0.53 is
    # not reachable from the guests. IPv4 only - udhcpd will not start with
    # an IPv6 one in `opt dns`, and DHCPv4 cannot carry them anyway.
    dns=$(awk '/^nameserver/ && $2 !~ /:/ { printf "%s ", $2 }' /run/systemd/resolve/resolv.conf 2>/dev/null || true)
    [ -n "$dns" ] || dns="1.1.1.1"
    : > "$STATE/udhcpd.leases"
    chmod 0644 "$STATE/udhcpd.leases"
    cat > "$STATE/udhcpd.conf" <<EOF
start $NET.100
end $NET.200
max_leases 101
interface $BRIDGE
lease_file $STATE/udhcpd.leases
auto_time 5
pidfile $STATE/udhcpd.pid
opt subnet 255.255.255.0
opt router $NET.1
opt dns $dns
opt lease 3600
EOF
    # In the foreground of a background job, so what it says goes to a log
    # instead of only to syslog, and one that dies at once is caught here.
    busybox udhcpd -f "$STATE/udhcpd.conf" > "$STATE/udhcpd.log" 2>&1 &
    sleep 1
    kill -0 $! 2>/dev/null || {
        echo "udhcpd did not start:" >&2
        cat "$STATE/udhcpd.log" >&2
        exit 1
    }
    echo "$BRIDGE up: $NET.1/24, $count tap(s), DHCP $NET.100-$NET.200"
}

down() {
    if [ -f "$STATE/udhcpd.pid" ]; then
        kill "$(cat "$STATE/udhcpd.pid")" 2>/dev/null || true
        rm -f "$STATE/udhcpd.pid"
    fi
    for table in filter nat; do
        iptables -t "$table" -S | grep -- "--comment $TAG" | sed 's/^-A /-D /' |
            while read -r rule; do
                eval "iptables -t $table $rule"
            done
    done
    if [ -f "$STATE/ip_forward" ]; then
        cat "$STATE/ip_forward" > /proc/sys/net/ipv4/ip_forward
        rm -f "$STATE/ip_forward"
    fi
    for link in /sys/class/net/"$TAP"*; do
        [ -e "$link" ] && ip link del "$(basename "$link")"
    done
    if [ -e "/sys/class/net/$BRIDGE" ]; then
        ip link del "$BRIDGE"
    fi
}

case "${1:-}" in
    up)
        [ $# = 3 ] || { echo "usage: $0 up COUNT USER" >&2; exit 64; }
        up "$2" "$3"
        ;;
    down) down ;;
    *) echo "usage: $0 up COUNT USER | down" >&2; exit 64 ;;
esac
