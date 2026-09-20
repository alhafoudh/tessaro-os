# meta-tessaro-distro

Product layer for Tessaro OS, a derivative of
[Moonforge](https://moonforgelinux.org/) built with
[meta-moonforge](https://github.com/moonforgelinux/meta-moonforge).

It provides:

* `conf/distro/tessaro.conf` - the `tessaro` distro, which requires
  `conf/distro/moonforge.conf` and overrides only the identity fields, plus the
  kiosk URL and cog's `PACKAGECONFIG`.
* `recipes-core/images/moonforge-image-base.bbappend` - product-specific
  additions to the Moonforge base image.
* `recipes-browser/tessaro-kiosk/` - the kiosk itself: the systemd units that
  run Igalia's cog under Weston, the watchdog that supervises it, the runtime
  configuration in `/etc/default/tessaro-kiosk`, and the offline page.
* `recipes-graphics/wayland/weston-init.bbappend` - the desktop background.

The layer is activated by `kas/tessaro-image-base-qemux86-64.yml` in the
repository root.
