# meta-tessaro-distro

Product layer for Tessaro OS, a derivative of
[Moonforge](https://moonforgelinux.org/) built with
[meta-moonforge](https://github.com/moonforgelinux/meta-moonforge).

It provides:

* `conf/distro/tessaro.conf` - the `tessaro` distro, which requires
  `conf/distro/moonforge.conf` and overrides only the identity fields, plus the
  kiosk URL, cog's `PACKAGECONFIG`, and the NetworkManager-for-systemd-networkd
  swap.
* `recipes-core/images/moonforge-image-base.bbappend` - product-specific
  additions to the Moonforge base image.
* `recipes-browser/tessaro-kiosk/` - the kiosk itself: the systemd units that
  run Igalia's cog under Weston, the watchdog that supervises it, the runtime
  configuration in `/etc/default/tessaro-kiosk`, and the offline page.
* `recipes-connectivity/tessaro-network/` - NetworkManager's configuration: the
  `conf.d` drop-in that defers DNS to systemd-resolved and leaves
  `/etc/resolv.conf` alone, and the unit that keeps `/var/lib/NetworkManager`
  on `/data` instead of tmpfs.
* `recipes-graphics/wayland/weston-init.bbappend` - the desktop background.
* `wic/` - the genericx86-64 partition layout.

The layer is activated by the machine fragments in `kas/machine/`, each of
which includes `kas/common/tessaro.yml`.
