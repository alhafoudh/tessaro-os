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
# compares against KIOSK_VNC_USER / KIOSK_VNC_PASSWORD from the kiosk's
# environment files. No system account, no /etc/shadow, no privilege, and the
# credential becomes something an operator can change in
# /etc/default/tessaro-kiosk without a rebuild.
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

FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += " \
    file://0001-vnc-let-the-PAM-stack-decide-which-user-may-log-in.patch \
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
