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

# Fonts, and this image had almost none. Nothing in this tree ever named a font
# package: the only TTF family present was liberation-fonts, and it arrives by
# accident, as an RRECOMMENDS of the weston recipe. So every generic family a
# stylesheet asks for - serif, sans-serif, monospace - resolved to the same
# three Liberation faces, and every emoji anywhere on the kiosk rendered as a
# tofu box.
#
# ttf-noto-emoji-color is NotoColorEmoji.ttf, the CBDT colour font, and it is
# the larger cost here at roughly 10 MB. DejaVu adds about 2 MB and is what
# makes serif and monospace distinguishable from sans at all. Both recipes
# inherit fontcache, so fc-cache runs at rootfs time and Chromium's fontconfig
# fallback finds them with no further configuration.
#
# All four come from meta-openembedded/meta-oe, which is why layer.conf now
# names openembedded-layer in LAYERDEPENDS.
CORE_IMAGE_EXTRA_INSTALL += " \
    ttf-noto-emoji-color \
    ttf-dejavu-sans \
    ttf-dejavu-serif \
    ttf-dejavu-sans-mono \
"

# The self-test page, which is what a factory image opens: TESSARO_KIOSK_URL
# defaults to http://127.0.0.1/ and a deployment repoints it. About 1.5 MB with
# its media. Worth carrying even on a deployed device - the alternative is a
# technician in front of a black screen with no way to tell a codec from a
# compositor.
#
# nginx arrives as its RDEPENDS rather than being named here, because serving
# the page is that recipe's business: a file:// URL has a null origin and could
# never be granted a serial port by policy. See the comment on
# TESSARO_KIOSK_URL in tessaro.conf.
CORE_IMAGE_EXTRA_INSTALL += " \
    tessaro-selftest \
"
