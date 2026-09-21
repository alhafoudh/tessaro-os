# Product-specific additions on top of the Moonforge base image.

# Swap the kiosk browser for cog. The wpe-simple-launcher package comes from
# meta-moonforge/kas/include/layer/meta-moonforge-wpe.yml (local_conf_header
# 20_meta-moonforge-wpe), which is pinned upstream and cannot be edited here.
# :remove applies to the fully expanded value of the variable, so it takes the
# package back out whichever fragment put it in - and, being silent, it would
# also say nothing if a Moonforge bump renamed the package. Check
# `bitbake -e moonforge-image-base | grep '^IMAGE_INSTALL='` after a bump.
#
# The meta-moonforge-wpe *layer* stays: it sets
# PREFERRED_PROVIDER_virtual/wpebackend (cog's wl plugin needs wpebackend-fdo),
# pulls in meta-webkit and meta-openembedded, carries the wpewebkit bbappend,
# and maps /home onto /data/overlay-home.
CORE_IMAGE_EXTRA_INSTALL:remove = "wpe-simple-launcher"

# cog itself and curl arrive as RDEPENDS of tessaro-kiosk, which owns the units,
# the runtime configuration, the watchdog and the offline page.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-kiosk \
"

# NetworkManager, replacing systemd-networkd (see tessaro.conf for the why and
# for the PACKAGECONFIG side of it).
#
# Named one split package at a time on purpose. The plain "networkmanager"
# package is ALLOW_EMPTY and RRECOMMENDS every plugin that was built - ppp,
# wwan, adsl, ovs, bluetooth, cloud-setup - so installing it would quietly pull
# in a mobile-broadband and VPN stack this device has no use for.
#
# networkmanager-wifi RDEPENDS on wpa-supplicant, which is already in the image
# via the wifi DISTRO_FEATURE. networkmanager-nmtui is the field tool;
# networkmanager-nmcli is the same thing for scripts and for a serial console
# too dumb for curses. tessaro-network carries the daemon's configuration.
CORE_IMAGE_EXTRA_INSTALL += " \
    networkmanager-daemon \
    networkmanager-nmcli \
    networkmanager-nmtui \
    networkmanager-wifi \
    tessaro-network \
"
