# Keep /home on the persistent partition. This used to come from
# meta-moonforge-wpe, which the kiosk no longer uses; Chromium still wants a
# writable HOME for the weston user, and the read-only rootfs does not provide
# one.
#
# Same trap as every other volatile bind: do NOT add /data/overlay-home via
# this variable for paths under /var/lib, /var/cache, /var/spool or /srv -
# the generated bind units race var-volatile-*.service and lose silently.
# /home is not under a volatile path, which is exactly why this one is safe.
#
# /root the same way, for root's ~/.ssh/authorized_keys: ROOT_HOME is /root
# on a systemd distro (init-manager-systemd.inc), it sits on the read-only
# rootfs, and dropbear 2022.83 has no option to read keys from anywhere but
# $HOME/.ssh. Moving ROOT_HOME instead would re-hash half the image.
VOLATILE_BINDS += "\
    /data/overlay-home /home\n\
    /data/overlay-root /root\n\
"
