# CUPS is in the image for tessaro-cups.service, which runs cupsd from a
# config in /usr/lib (recipes-printing/tessaro-printing, docs/printing.md).
# The recipe's own units would start a second cupsd from /etc/cups on the
# /etc overlay, and cups.socket would take /run/cups/cups.sock from ours. They
# stay installed; they are only never enabled - set here, not as a
# :pn-cups override in tessaro.conf, for the reason in tinyproxy_%.bbappend.
# The web interface is not in the recipe's default PACKAGECONFIG, and our
# cupsd.conf says WebInterface No besides.
SYSTEMD_AUTO_ENABLE:${PN} = "disable"
