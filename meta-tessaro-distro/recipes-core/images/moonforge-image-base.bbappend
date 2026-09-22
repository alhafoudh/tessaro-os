# Product-specific additions on top of the Moonforge base image.

# Chromium and the CA store arrive as RDEPENDS of tessaro-kiosk, which owns the
# units, the runtime configuration, the tessaro-agent binary and the offline
# page.
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
