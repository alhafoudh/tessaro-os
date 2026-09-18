# meta-tessaro-distro

Product layer for Tessaro OS, a derivative of
[Moonforge](https://moonforgelinux.org/) built with
[meta-moonforge](https://github.com/moonforgelinux/meta-moonforge).

It provides:

* `conf/distro/tessaro.conf` - the `tessaro` distro, which requires
  `conf/distro/moonforge.conf` and overrides only the identity fields.
* `recipes-core/images/moonforge-image-base.bbappend` - product-specific
  additions to the Moonforge base image.

The layer is activated by `kas/tessaro-image-base-qemux86-64.yml` in the
repository root.
