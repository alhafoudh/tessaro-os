# nginx on Tessaro serves exactly one thing: the local self-test page, on the
# loopback. These changes are needed to make that true.

# 1. Give nginx a config directory outside /etc.
#
#    Everything nginx ships lives under /etc/nginx, and /etc here is an
#    overlayfs upper on /data - so the first on-device write to any of it
#    shadows the image's copy permanently and no later image can move that
#    default again. The rest of this product keeps its build-time defaults in
#    /usr/lib for exactly that reason (see tessaro-kiosk.env and the
#    NetworkManager drop-in), and this puts nginx's server blocks on the same
#    footing: /usr/lib/nginx/conf.d is ours and upgradeable, /etc/nginx/conf.d
#    stays where a technician overrides it.
#
#    nginx.conf itself has to stay in /etc - the path is compiled into the
#    binary by --conf-path - so this is one sed on that file rather than a
#    replacement of it. The include is added before the upstream ones, so
#    an operator's /etc/nginx/conf.d entry is parsed after ours.
#
# 2. Drop the stock default_server.
#
#    It listens on 0.0.0.0:80 and [::]:80 and serves /var/www/localhost/html,
#    which on a kiosk means an nginx welcome page reachable from the network.
#    Removing the symlink leaves the site file in sites-available for reference
#    and leaves our own server block, which binds 127.0.0.1 only, as the one
#    thing nginx answers.

# The upstream do_install has already rewritten the shipped nginx.conf's paths
# with ${sysconfdir}, so match on that rather than on a literal /etc.
do_install:append() {
    sed -i \
        -e 's|^\( *\)include ${sysconfdir}/nginx/conf.d/\*.conf;|\1include ${nonarch_libdir}/nginx/conf.d/*.conf;\n\1include ${sysconfdir}/nginx/conf.d/*.conf;|' \
        ${D}${sysconfdir}/nginx/nginx.conf

    grep -q '${nonarch_libdir}/nginx/conf.d' ${D}${sysconfdir}/nginx/nginx.conf || \
        bbfatal "nginx.conf has no conf.d include to hook - upstream layout changed"

    rm -f ${D}${sysconfdir}/nginx/sites-enabled/default_server

    install -d ${D}${nonarch_libdir}/nginx/conf.d
}

FILES:${PN} += " ${nonarch_libdir}/nginx "
