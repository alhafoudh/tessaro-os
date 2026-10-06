#!/bin/sh
# Several qemu devices on one network the host sees them on, so
# `tessaro-ctl nodes list` and tessaro-gui's node list find every one over
# mDNS: what `mise run qemu:run --count N` and `qemu:vnc --count N` run for
# N > 1, on qemux86-64 and genericarm64 (docs/build.md "Several devices").
#
#   scripts/qemu-devices.sh window|vnc COUNT
#
# Each device gets one NIC, on the shared network: NetworkManager in the
# image puts its one ethernet profile on whichever device comes up first,
# so a second NIC would get no address. Each gets its own MAC
# (52:54:00:77:00:<n>) and a fresh SMBIOS UUID, and boots with -snapshot, so
# its machine id, node id and name are its own. Device n's serial console
# goes to build/qemu-devices/vm<n>.serial.log; in vnc mode its screen is on
# 127.0.0.1:590<n> with the password "tessaro".
#
# Linux: QEMU is bitbake's, in one kas container as qemu:run has it, and the
# network is the bridge scripts/qemu-net.sh builds under sudo when the run
# starts and removes when it ends. macOS: the devices are on vmnet-shared,
# which already puts every guest on the Mac's bridge100 with its DHCP.
# Ctrl-C stops every device.
set -eu

mode=${1:-window}
count=${2:-}
case "$mode" in
    window|vnc) ;;
    *) echo "usage: $0 window|vnc COUNT" >&2; exit 64 ;;
esac
case "$count" in
    ''|*[!0-9]*) echo "usage: $0 window|vnc COUNT" >&2; exit 64 ;;
esac
[ "$count" -ge 2 ] && [ "$count" -le 99 ] || {
    echo "COUNT must be 2 to 99; one device is plain qemu:run or qemu:vnc" >&2
    exit 64
}

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
state=$root/build/qemu-devices
mkdir -p "$state"
rm -f "$state"/vm*.log "$state"/inside.sh
CONTAINER=tessaro-qemu-devices

mac() { printf '52:54:00:77:00:%02x' "$1"; }
uuid() {
    if [ -r /proc/sys/kernel/random/uuid ]; then
        cat /proc/sys/kernel/random/uuid
    else
        uuidgen | tr 'A-Z' 'a-z'
    fi
}

# The address the host's DHCP gave MAC, if any yet.
lease() {
    if [ "$(uname -s)" = Darwin ]; then
        # macOS writes the MAC without leading zeros: 52:54:0:77:0:1.
        short=$(echo "$1" | awk -F: '{ for (i = 1; i <= NF; i++) { sub(/^0/, "", $i); printf "%s%s", $i, (i < NF ? ":" : "") } }')
        awk -v mac="1,$short" '
            /ip_address=/ { ip = substr($0, index($0, "=") + 1) }
            /hw_address=/ { if (substr($0, index($0, "=") + 1) == mac) found = ip }
            END { print found }' /var/db/dhcpd_leases 2>/dev/null || true
    else
        busybox dumpleases -f "$state/udhcpd.leases" 2>/dev/null |
            awk -v mac="$1" 'tolower($1) == mac { print $2; exit }'
    fi
}

# One line per device: what it is and where to reach it. Again every few
# seconds until each has an address, while the devices still run.
table() {
    printf '%-4s %-18s %-15s %-15s %s\n' dev mac address vnc "serial log"
    n=1
    while [ "$n" -le "$count" ]; do
        address=$(lease "$(mac "$n")")
        vnc=-
        [ "$mode" = vnc ] && vnc=127.0.0.1:$((5900 + n))
        printf '%-4s %-18s %-15s %-15s %s\n' "$n" "$(mac "$n")" "${address:-(waiting)}" "$vnc" \
            "build/qemu-devices/vm$n.serial.log"
        n=$((n + 1))
    done
}

watch_leases() {
    shown=-1
    while running; do
        leased=0
        n=1
        while [ "$n" -le "$count" ]; do
            [ -n "$(lease "$(mac "$n")")" ] && leased=$((leased + 1))
            n=$((n + 1))
        done
        if [ "$leased" != "$shown" ]; then
            echo
            table
            shown=$leased
        fi
        [ "$leased" = "$count" ] && break
        sleep 3
    done
    echo
    echo "Ctrl-C stops every device."
}

if [ "$(uname -s)" = Darwin ]; then
    [ "${TESSARO_QEMU_NET:-vmnet-shared}" = vmnet-shared ] || {
        echo "several devices need vmnet-shared, not --no-vmnet: slirp has no shared network" >&2
        exit 64
    }
    # vmnet needs root: one prompt here, and every device's sudo reuses it.
    sudo -v
    pids=
    stop() {
        trap - EXIT INT TERM
        [ -n "$pids" ] && kill $pids 2>/dev/null || true
        wait 2>/dev/null || true
    }
    trap stop EXIT
    trap 'exit 130' INT TERM
    running() { for pid in $pids; do kill -0 "$pid" 2>/dev/null && return 0; done; return 1; }
    n=1
    while [ "$n" -le "$count" ]; do
        TESSARO_QEMU_NET=vmnet-shared TESSARO_QEMU_MAC=$(mac "$n") TESSARO_QEMU_UUID=$(uuid) \
            TESSARO_QEMU_VNC=$n TESSARO_QEMU_SERIAL=$state/vm$n.serial.log \
            sh "$root/scripts/qemu-arm64.sh" "$mode" > "$state/vm$n.qemu.log" 2>&1 < /dev/null &
        pids="$pids $!"
        n=$((n + 1))
    done
    watch_leases
    wait
    exit 0
fi

# Linux: the network first, under sudo, and gone again however the run ends.
# A run that was killed may have left one behind: down clears it first.
sudo -v
sudo sh "$root/scripts/qemu-net.sh" down > /dev/null 2>&1 || true
keepalive=
stop() {
    trap - EXIT INT TERM
    echo "stopping the devices ..."
    kill "$keepalive" 2>/dev/null || true
    # Every process in the container but its init, as root there: the TERM
    # docker stop sends goes through the kas entrypoint and does not always
    # reach QEMU. docker stop then only waits for the container to end. The
    # shell's kill: procps' own cannot parse -1 as "every process".
    docker exec "$CONTAINER" sh -c 'kill -TERM -1' > /dev/null 2>&1 || true
    docker stop -t 30 "$CONTAINER" > /dev/null 2>&1 || true
    wait 2>/dev/null || true
    sudo sh "$root/scripts/qemu-net.sh" down
    echo "network removed"
}
trap stop EXIT
trap 'exit 130' INT TERM
sudo sh "$root/scripts/qemu-net.sh" up "$count" "$(id -un)"
# Keep sudo's timestamp fresh, so taking the network down at the end does
# not ask again after a long run.
( while sleep 60; do sudo -n -v 2>/dev/null || exit 0; done ) &
keepalive=$!
running() { docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null | grep -q true; }

# What runs in the container: every device in the background, the way the
# e2e harness starts runqemu (test/e2e/spec/support/vm.rb), and on TERM -
# docker stop, through the container's init - all of them stopped.
runtime="--name $CONTAINER --device /dev/net/tun --network=host"
[ -d /dev/dri ] && runtime="$runtime --device /dev/dri"
{
    echo 'trap "trap \"\" TERM INT; kill -TERM 0; wait; exit 0" TERM INT'
    n=1
    while [ "$n" -le "$count" ]; do
        log=/repo/build/qemu-devices/vm$n
        if [ "$TESSARO_MACHINE" = genericarm64 ]; then
            echo "KAS_CONTAINER_INSIDE=1 WIC=$WIC TESSARO_QEMU_NET=tap TESSARO_QEMU_TAP=tessaro-tap$n" \
                "TESSARO_QEMU_MAC=$(mac "$n") TESSARO_QEMU_UUID=$(uuid) TESSARO_QEMU_VNC=$n" \
                "TESSARO_QEMU_SERIAL=$log.serial.log sh /repo/scripts/qemu-arm64.sh $mode" \
                "> $log.qemu.log 2>&1 < /dev/null &"
        else
            params="-netdev tap,id=net0,ifname=tessaro-tap$n,script=no,downscript=no"
            params="$params -device virtio-net-pci,netdev=net0,mac=$(mac "$n") -uuid $(uuid)"
            [ "$mode" = vnc ] && params="$params -object secret,id=vncpw,data=tessaro -vnc 127.0.0.1:$n,password-secret=vncpw"
            accel=
            [ -e /dev/kvm ] && accel=kvm
            display=nographic
            [ -d /dev/dri ] && display=egl-headless
            echo "runqemu $WIC ovmf nonetwork snapshot $accel $display serialstdio qemuparams='$params'" \
                "> $log.serial.log 2>&1 < /dev/null &"
        fi
        n=$((n + 1))
    done
    echo 'wait'
} > "$state/inside.sh"
if [ "$TESSARO_MACHINE" != genericarm64 ] && [ -e /dev/kvm ]; then
    # See qemu:run in mise.toml: the kvm gid as builder's primary group.
    runtime="$runtime --device /dev/kvm -e GROUP_ID=$(stat -c %g /dev/kvm)"
fi
echo "booting $count devices in the kas container (log: build/qemu-devices/kas.log)"
kas-container --runtime-args "$runtime" shell $KAS_CONFIG -c "sh /repo/build/qemu-devices/inside.sh" \
    > "$state/kas.log" 2>&1 < /dev/null &
kas=$!
# The container takes a moment to exist; until then nothing is running yet.
tries=0
until running || ! kill -0 "$kas" 2>/dev/null || [ "$tries" -ge 60 ]; do
    sleep 1
    tries=$((tries + 1))
done
running || { echo "the kas container did not start; see build/qemu-devices/kas.log" >&2; exit 1; }
watch_leases
wait "$kas" || true
