# tessaro-os

Yocto/OpenEmbedded build for **Tessaro**, a web kiosk. It produces bootable
Linux images for the hardware platforms Tessaro ships on.

Built as a derivative of [Moonforge](https://moonforgelinux.org/), so images
come with an immutable read-only rootfs, systemd, an overlayfs `/etc` and a
persistent `/data` partition out of the box.

## Quick start

```sh
mise run build     # build the image and OVMF firmware
mise run run-vnc   # boot it in QEMU, framebuffer on localhost:5900
mise run run       # same, but serial console only (Ctrl-a x to exit)
```

Use `run-vnc` to see the kiosk: `run` boots with `nographic`, so there is no
display to render the browser on.

Images land in `build/tmp/deploy/images/<machine>/`.

## Status

| Platform | State |
| --- | --- |
| `qemux86-64` | working, used for development, kiosk browser enabled |
| Raspberry Pi 4 / 5 | not wired up yet (`meta-moonforge-raspberrypi`) |

## Kiosk browser

Images boot straight into a fullscreen WPEWebKit window on Weston, showing
`https://www.freevision.sk`. The build-time default is
`WPE_SIMPLE_LAUNCHER_URL` in `meta-tessaro-distro/conf/distro/tessaro.conf`.

On a running device, change the URL without rebuilding by uncommenting and
editing `KIOSK_URL` in `/etc/default/tessaro-kiosk`:

```sh
vi /etc/default/tessaro-kiosk
systemctl restart wpe-simple-launcher
```

`/etc` is an overlayfs whose upper layer is the persistent `/data` partition,
so the change survives a reboot.

## Structure

`kas/*.yml` pins every upstream repo and selects the layers; `meta-tessaro-distro`
holds everything specific to this product. See [CLAUDE.md](CLAUDE.md) for the
full layout, the task list and the gotchas worth knowing before your first build.
