# tessaro-os

Yocto/OpenEmbedded product repository for Tessaro OS, a derivative of
[Moonforge](https://moonforgelinux.org/).

Obsidian tracking: no

## Layout

This repository is the kas *root repo*. Everything else is a build input that
kas clones and checks out from pins in `kas/*.yml`, and is gitignored:

```
tessaro-os/
├── kas/
│   ├── tessaro-image-base-qemux86-64.yml   # product config, pins meta-moonforge
│   └── common/debug.yml                    # passwordless root, dev images only
├── meta-tessaro-distro/                    # the product layer
├── meta-moonforge/                         # kas-managed, pinned
├── openembedded-core/, bitbake/            # kas-managed, pinned
├── build/                                  # kas build dir (TOPDIR)
└── cache/                                  # DL_DIR + SSTATE_DIR
```

In kas, a `repos:` entry with no `url:` is the repo holding the config file,
which kas never touches - that is the `tessaro-os:` entry. Upstream layers get
a `url`/`commit`/`branch` instead.

## Workflow

Use the mise tasks rather than calling kas-container directly:

| Task | Purpose |
| --- | --- |
| `mise run build` | Build the image and OVMF firmware |
| `mise run shell` | Interactive kas shell (cwd is the build dir) |
| `mise run unpack` | Decompress the `.wic` for runqemu |
| `mise run run` | Boot in QEMU, serial console on the terminal |
| `mise run run-vnc` | Boot in QEMU with VNC on localhost:5900 |
| `mise run clean` | Drop build artifacts, keep sstate and downloads |

Exit the QEMU serial console with `Ctrl-a x`.

## Gotchas

* **runqemu needs a file path, not an image name.** `runqemu ... qemux86-64
  moonforge-image-base wic` fails with `IMAGE_LINK_NAME wasn't set`: the image
  name is treated as a lazy rootfs, and the machine argument makes runqemu run
  `bitbake -e` with no recipe target, where `IMAGE_LINK_NAME` (set by the
  recipe-scope `image-artifact-names.bbclass`) does not exist. Pass the
  `.rootfs.wic` path instead - that is what the `run` tasks do.
* **KVM inside the kas container** needs `-e GROUP_ID=$(stat -c %g /dev/kvm)`,
  not `--group-add`: the entrypoint's `gosu builder` rebuilds supplementary
  groups from the container's `/etc/group`, so only the primary gid survives.
* **The build host is headless.** `nographic` is the default for `run`; use
  `run-vnc` plus an SSH tunnel if you need the framebuffer. The base image only
  ships psplash, so there is little to see until a graphics layer is added.
