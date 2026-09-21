# Keep /home on the persistent partition. This used to come from
# meta-moonforge-wpe, which the kiosk no longer uses; Chromium still wants a
# writable HOME for the weston user, and the read-only rootfs does not provide
# one.
#
# Same trap as every other volatile bind: do NOT add /data/overlay-home via
# this variable for paths under /var/lib, /var/cache, /var/spool or /srv -
# the generated bind units race var-volatile-*.service and lose silently.
# /home is not under a volatile path, which is exactly why this one is safe.
VOLATILE_BINDS += "\
    /data/overlay-home /home\n\
"
