# What lets any common VNC viewer log in to the screen mirror, not only the
# ones that speak VeNCrypt (TigerVNC, Remmina, tessaro-gui).
#
# macOS Screen Sharing, Royal TSX, RealVNC and noVNC log in with the classic
# VNC password, and macOS speaks RFB 3.3 to a 3.8 server; neatvnc 0.8.1 has
# neither. And macOS asks for depth 32 with 24-bit colours, which made
# neatvnc send ZRLE pixels a byte too wide, so the picture broke after the
# first frame. Upstream fixed all of it after 0.8.1, but only in 1.0, whose
# API Weston 13's VNC backend cannot use - so these are backports to 0.8.1,
# each naming its upstream commit. Drop them when Weston moves to a neatvnc
# that has them.
#
# The classic login exists only with HAVE_CRYPTO, which is neatvnc built with
# nettle (tessaro.conf), and is offered only because Weston's patch asks for
# a login without encryption: the mirror binds 127.0.0.1 and is reached
# through an SSH tunnel, which is what encrypts. See docs/remote-access.md.

FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += " \
    file://0001-server-accept-RFB-3.3-and-3.7.patch \
    file://0002-add-the-classic-VNC-password-login.patch \
    file://0003-server-normalize-pixel-format-depth-from-client.patch \
"
