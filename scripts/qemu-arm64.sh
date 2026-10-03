#!/bin/sh
# Boots the genericarm64 image: what `mise run qemu:run` and `qemu:vnc` run
# for that machine, on Linux and on macOS alike (AARCH64.md).
#
#   scripts/qemu-arm64.sh window|vnc
#
# Linux: QEMU is bitbake's own (qemu-helper-native's sysroot), so it runs in
# the kas container, on TCG - an x86 host cannot run Arm code natively. The
# firmware is the U-Boot image:build builds for genericarm64; its EFI loader
# starts systemd-boot from the ESP.
#
# macOS on Apple Silicon: Homebrew's qemu (`brew install qemu`), on
# Hypervisor.framework, with the edk2 firmware it ships and CoreAudio for
# sound. The image is the one `mise run image:pull` fetched into the repo
# root, unpacked by qemu:unpack.
#
# Both: the serial console on this terminal (Ctrl-a x quits), -snapshot so the
# .wic stays as built, and the forwards qemu:run has on qemux86-64:
# 127.0.0.1:2222 to SSH and 127.0.0.1:7401 to the API, Webconfig included
# (https://127.0.0.1:7401). vnc mode puts the screen on 127.0.0.1:5901, with
# the password "tessaro".
set -eu

mode=${1:-window}
case "$mode" in
    window|vnc) ;;
    *) echo "usage: $0 window|vnc" >&2; exit 64 ;;
esac

set -- \
    -machine virt -smp 4 -m 4096 \
    -snapshot \
    -device qemu-xhci -device usb-kbd -device usb-tablet \
    -device intel-hda \
    -netdev user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22,hostfwd=tcp:127.0.0.1:7401-:7400 \
    -device virtio-net-pci,netdev=net0 \
    -serial mon:stdio
if [ "$mode" = vnc ]; then
    set -- "$@" -object secret,id=vncpw,data=tessaro -vnc 127.0.0.1:1,password-secret=vncpw
fi

if [ "$(uname -s)" = Darwin ]; then
    command -v qemu-system-aarch64 >/dev/null || {
        echo "needs qemu: brew install qemu" >&2
        exit 1
    }
    image=$(basename "$WIC")
    [ -f "$image" ] || {
        echo "$image not found - run 'mise run image:pull' and 'mise run qemu:unpack' first" >&2
        exit 1
    }
    # Homebrew's qemu is built without virglrenderer, so the guest gets a
    # plain virtio-gpu unless this QEMU has the GL device: a kiosk that
    # needs GPU rendering may stay blank on it (AARCH64.md, Peripherals).
    if qemu-system-aarch64 -device help | grep -q '"virtio-gpu-gl-pci"'; then
        gpu=virtio-gpu-gl-pci display=cocoa,gl=es
    else
        gpu=virtio-gpu-pci display=cocoa
    fi
    [ "$mode" = vnc ] && display=none
    exec qemu-system-aarch64 "$@" \
        -accel hvf -cpu host \
        -bios "$(brew --prefix qemu)/share/qemu/edk2-aarch64-code.fd" \
        -drive "file=$image,format=raw,if=virtio" \
        -device "$gpu" -display "$display" \
        -audiodev coreaudio,id=snd0 -device hda-duplex,audiodev=snd0
fi

if [ "${KAS_CONTAINER_INSIDE:-}" != 1 ]; then
    # Host side: re-run this script in the kas container, with the host's GPU
    # and network as qemu:run has them (see its comments in mise.toml).
    if [ -d /dev/dri ]; then
        gpu_runtime="--device /dev/dri"
    else
        echo "warning: no /dev/dri on the host - the kiosk browser will not render" >&2
        gpu_runtime=""
    fi
    exec kas-container --runtime-args "$gpu_runtime --network=host" shell $KAS_CONFIG \
        -c "KAS_CONTAINER_INSIDE=1 WIC=$WIC sh /repo/scripts/qemu-arm64.sh $mode"
fi

# Inside the container; cwd is the build dir. The Mesa drivers QEMU's EGL
# display loads are in the native sysroot, which runqemu points libGL at the
# same way (set_dri_path in openembedded-core/scripts/runqemu).
bindir=tmp/work/x86_64-linux/qemu-helper-native/1.0/recipe-sysroot-native/usr/bin
deploy=$(dirname "$WIC")
[ -x "$bindir/qemu-system-aarch64" ] && [ -f "$deploy/u-boot.bin" ] || {
    echo "no QEMU or U-Boot for genericarm64 - run 'TESSARO_MACHINE=genericarm64 mise run image:build' first" >&2
    exit 1
}
export LIBGL_DRIVERS_PATH="$PWD/$bindir/../lib/dri"
if [ -e /dev/dri ]; then
    gpu=virtio-gpu-gl-pci display=egl-headless
else
    gpu=virtio-gpu-pci display=none
fi
exec "$bindir/qemu-system-aarch64" "$@" \
    -accel tcg,thread=multi -cpu max,pauth-impdef=on \
    -bios "$deploy/u-boot.bin" \
    -drive "file=$WIC,format=raw,if=virtio" \
    -device "$gpu" -display "$display" \
    -audiodev none,id=snd0 -device hda-duplex,audiodev=snd0
