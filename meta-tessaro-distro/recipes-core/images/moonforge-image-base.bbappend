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
