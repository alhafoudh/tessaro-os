# Who may open the VNC screen share, and with what.
#
# Weston's VNC backend has no credential of its own: it calls
# pam_start("weston-remote-access", <username the client sent>, ...) and ships
# a stack that is just "auth include login". That cannot be made to accept a
# product login, because the compositor runs unprivileged: pam_unix's helper
# refuses to check any account but the caller's own (it drops its setuid and
# then cannot read /etc/shadow, 0400 root), so the only username that can ever
# succeed there is "weston".
#
# So the stack is replaced with pam_exec calling our own checker, which
# compares against KIOSK_VNC_USER / KIOSK_VNC_PASSWORD from the kiosk's image
# defaults. No system account, no /etc/shadow, no privilege. The credential is
# static, an image property: tessaro-ctl deliberately has no key for it.
#
# Both files land in the *weston* package rather than weston-init because the
# PAM service file has to replace the one weston's own do_install writes -
# pam/meson.build installs it whenever backend-vnc is enabled - and two
# packages owning one path is a rootfs conflict, not an override.

#
# The patch is the other half of the same decision. The VNC backend refuses
# any username that does not resolve to the compositor's own uid
# (`getpwnam(username)->pw_uid != getuid()` in vnc_handle_auth), *before* PAM
# is called - so without it the login would still be forced to be "weston" no
# matter what the PAM stack says. That check is there because upstream assumes
# pam_unix, which could not have authenticated anyone else anyway; once the
# credential is checked by our own script the premise is gone. Six lines.

#
# The second patch is what makes the share interactive. screen-share.so
# forwards the viewer's pointer and keyboard through a seat of its own, a
# second wl_seat, and Chromium binds only the first one - so remote input
# reached nobody. The patch injects it into the compositor's own seat
# instead, leaves that seat's keymap and modifiers alone, releases whatever
# the viewer still holds when it goes, and maps positions through the
# output's scale. It also adds `input=` to [screen-share]: false shares the
# picture only, which is screen.vnc=view-only (tessaro-weston-config writes
# it). See docs/remote-access.md.

#
# The third lets any common viewer log in, not only those that speak
# VeNCrypt. It keeps the login required but drops
# NVNC_AUTH_REQUIRE_ENCRYPTION, so neatvnc also offers Apple's
# Diffie-Hellman login and the classic VNC password - the one macOS Screen
# Sharing, Royal TSX and RealVNC use - and adds --vnc-password-file= to hand
# neatvnc that password (tessaro-weston-config writes the file). The server
# is on 127.0.0.1 behind an SSH tunnel, which already encrypts the link. Both
# logins need neatvnc built with nettle (tessaro.conf) and its backports
# (recipes-graphics/neatvnc).
#
# The fourth keeps one VNC seat for the backend's life. A seat made and
# destroyed per viewer took its wl_seat global with it, and a viewer that left
# right after logging in removed it while screen-share was still binding it -
# a protocol error that ended the mirror until Weston restarted.
#
# The fifth makes the mirror on demand. A shared output is read back from the
# renderer on every damaged frame and kept off hardware planes, which made
# scrolling lag on a device nobody watched. screen-share listens on
# screen-share.sock in Weston's runtime directory for share, unshare and
# status, so tessaro-agent starts the mirror when a VNC tunnel asks and stops
# it when the viewer is gone; tessaro-weston-config writes
# start-on-startup=false. It also takes an ended share out of screen-share's
# list, which upstream leaves there freed. See docs/remote-access.md.

FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += " \
    file://0001-vnc-let-the-PAM-stack-decide-which-user-may-log-in.patch \
    file://0002-screen-share-inject-remote-input-into-the-compositor-seat.patch \
    file://0003-vnc-offer-logins-for-viewers-without-TLS.patch \
    file://0004-vnc-keep-one-seat-for-every-client.patch \
    file://0005-screen-share-control-socket.patch \
    file://weston-remote-access.pam \
    file://tessaro-vnc-auth \
"

do_install:append() {
    install -Dm0755 ${WORKDIR}/tessaro-vnc-auth \
        ${D}${libexecdir}/tessaro-vnc-auth

    # Substituted for the same reason oe-core seds paths into weston.service:
    # a hardcoded /usr/libexec here would silently stop matching if a distro
    # moved libexecdir, and the symptom would be every VNC login refused.
    sed -e "s|@libexecdir@|${libexecdir}|g" \
        ${WORKDIR}/weston-remote-access.pam > ${WORKDIR}/weston-remote-access
    install -Dm0644 ${WORKDIR}/weston-remote-access \
        ${D}${sysconfdir}/pam.d/weston-remote-access
}

# pam_exec.so is its own split package and is not in the image by default.
# Without it every VNC login fails with "PAM: authentication failed" and
# nothing says why.
RDEPENDS:${PN} += "pam-plugin-exec"
