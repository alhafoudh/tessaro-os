# tessaro-os

Yocto/OpenEmbedded build for **Tessaro**, a web kiosk. It produces bootable
Linux images for the hardware platforms Tessaro ships on.

Built as a derivative of [Moonforge](https://moonforgelinux.org/), so images
come with an immutable read-only rootfs, systemd, an overlayfs `/etc` and a
persistent `/data` partition out of the box.

## Quick start

```sh
mise run build     # build the image and OVMF firmware
mise run run       # boot it in QEMU (Ctrl-a x to exit)
```

Images land in `build/tmp/deploy/images/<machine>/`.

## Status

| Platform | State |
| --- | --- |
| `qemux86-64` | working, used for development |
| Raspberry Pi 4 / 5 | not wired up yet (`meta-moonforge-raspberrypi`) |

The kiosk browser is not enabled yet. It comes from `meta-moonforge-wpe`
(WPEWebKit + `wpe-simple-launcher`); adding it means one `includes:` entry in
`kas/tessaro-image-base-qemux86-64.yml` and setting `WPE_SIMPLE_LAUNCHER_URL`.

## Structure

`kas/*.yml` pins every upstream repo and selects the layers; `meta-tessaro-distro`
holds everything specific to this product. See [CLAUDE.md](CLAUDE.md) for the
full layout, the task list and the gotchas worth knowing before your first build.
