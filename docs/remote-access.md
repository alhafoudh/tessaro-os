# Remote access

**A technician sees the real panel over VNC on `127.0.0.1:5900`**, reached
through an SSH tunnel. It is the live screen with the live Chromium on it, not
a second session.

**The viewer can click and type, unless `screen.vnc=view-only`.** `on` (the
default) is view and control, `view-only` is the picture alone, `off` is no
mirror. Remote and local input share one seat: one cursor, one keyboard
focus. On a touch-only panel a cursor appears while a viewer is connected
and goes when they leave.

**Remote input goes into the compositor's own seat, by a patch to
`screen-share.c`** (`0002-screen-share-inject-remote-input-into-the-compositor-seat.patch`,
applied by `weston_%.bbappend`). Upstream gives the viewer a seat of its own,
a *second* `wl_seat` (`weston_seat_init(&seat->base, compositor,
"screen-share")`), and Chromium binds exactly one seat -
`wayland_seat.cc:34` returns early once `connection->seat_` is set, which the
libinput seat has done at startup - so remote clicks and keys reached nobody.
Patching Chromium to bind more than one seat is the wrong end: a Chromium
patch and a multi-hour rebuild against a module nothing else uses. What the
patch does, and why each part:

* **The target is the first seat on `compositor->seat_list`**, the libinput
  one. With no input device at all there is none; the patch makes one and
  keeps it until Weston exits, so a client that bound it keeps it across
  viewers. It is named `screen-share`, never `default`: the libinput backend
  looks its seats up by name (`udev_seat_get_named`) and would treat ours as
  its own struct.
* **The seat gets a pointer and a keyboard only while a viewer has them**
  (`weston_seat_init_pointer`/`_keyboard`, released on disconnect). libweston
  counts devices per seat, so a local mouse or keyboard is unaffected.
* **The panel's keymap and modifiers are left alone.** The child's keymap is
  ignored and its modifier mask is not written into the seat's xkb state;
  keys update the state themselves (`STATE_UPDATE_AUTOMATIC`). Both
  compositors build their keymap from the same xkb defaults, and nothing in
  the image sets a layout. Setting one on the device means setting the same
  one for the child, or keycodes would mean different keys.
* **Focus is the seat's.** Enter and leave from the child move no focus; a
  click or touch decides where keys go, as with a local keyboard. Keys and
  buttons the viewer still holds when it goes are released, so nothing stays
  pressed. Enter's position does move the pointer: it is the viewer's first
  position, and a viewer that moves once and clicks (the e2e case does) sends
  no motion besides it, so the click would land where the pointer last was.
* **Positions go through `weston_coord_global_from_output_point`.** The
  child's surface is the output's buffer, so a viewer's position is in
  output pixels; upstream passed them on as global coordinates, which is
  wrong once `screen.scale` scales the output or a second output is not at
  0,0.
* **`view-only` is `input=false` in `[screen-share]`**, a key the patch
  adds. The child's `wl_seat` is then never bound, so nothing the viewer
  sends arrives, whatever the client.

**It cannot be done by adding a VNC backend to the running compositor.** In
Weston a backend is what drives the display, and this one is on
`drm-backend.so`. So the mirror is `screen-share.so` (loaded from the
`weston.service` drop-in): it forks a *second* Weston on `vnc-backend.so` plus
`fullscreen-shell.so`, presents this compositor's output surface into it over
`zwp_fullscreen_shell_v1`, and injects the remote pointer and key events back
into this compositor's seat as described above.

* **The `[screen-share]` section is generated**, by `tessaro-weston-config`,
  under `screen.vnc` (`KIOSK_VNC`: `on` by default, `view-only`, `off`) - the same file, the
  same log (`journalctl -t tessaro-weston-config`) and the same "a section
  written by hand in `/etc/xdg/weston/weston.ini` wins" rule as `[output]` and
  `[input-method]`. The `weston-init` bbappend deletes the `[screen-share]`
  block the shipped `weston.ini` inherits from oe-core through Moonforge; that
  block names `rdp-backend.so`, which is not built, and Weston honours the
  *first* matching section, so a leftover would silently shadow ours. A
  `bbfatal` in `do_install` guards that.
* **`--address` and `--port` are command-line only.** weston.ini's `[vnc]`
  section takes `refresh-rate`, `tls-cert` and `tls-key` and nothing that binds
  a socket, so loopback is enforced from the generated `command=`. The child
  also gets `--no-config`, without which it loads `weston.ini`, finds a
  `[screen-share]` section and shares itself recursively.
* **Do not copy oe-core's stock command verbatim.** It carries
  `--no-clients-resize`, an RDP-backend option; an unrecognised option is fatal
  to the child, and the failure reads as a screen-share problem rather than a
  typo.
* **A login is required, encryption is not: the SSH tunnel encrypts.**
  Upstream calls `nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH |
  NVNC_AUTH_REQUIRE_ENCRYPTION, ...)`, which leaves neatvnc offering only
  VeNCrypt (a username and password inside TLS) and RSA-AES, logins most
  viewers do not speak.
  `0003-vnc-offer-logins-for-viewers-without-TLS.patch` drops the encryption
  flag and sets `NVNC_AUTH_ALLOW_BROKEN_CRYPTO`, so neatvnc also offers
  Apple's Diffie-Hellman login (type 30) and the classic VNC password
  (type 2), in that order after VeNCrypt (`init_security_types` in neatvnc's
  `server.c`). Neither encrypts the session. Encrypting again inside the
  tunnel buys nothing, and the server binds the loopback only, so no path to
  it skips SSH. There is still no unauthenticated mode.
  * TLS stays: `vnc.c` refuses to start without a cert and key, and
    `nvnc_has_auth()` is false without `PACKAGECONFIG:append:pn-neatvnc =
    " tls"` in `tessaro.conf` (its own default is `""`; Weston logs `Neat VNC
    built without TLS support` and dies). The certificate is self-signed,
    generated at build time by the `weston-init` bbappend into
    `/usr/lib/tessaro-vnc/`.
  * Apple's login, RSA-AES and the classic password exist only when neatvnc
    is built with nettle (`HAVE_CRYPTO`). The recipe has no `PACKAGECONFIG`
    for it and meson's `auto` finds nettle only if something else put it in
    the sysroot, so `tessaro.conf` enables `-Dnettle=enabled` and depends on
    `nettle gmp`. Both already ship, for gnutls.
* **The classic VNC password, RFB 3.3 and the depth fix are backports to
  neatvnc 0.8.1** (`recipes-graphics/neatvnc/`). Upstream added all three
  after 0.8.1 (`58a6fbe`, `6109e61`, `8c646d0`), but only in 1.0, whose
  asynchronous auth API Weston 13's VNC backend cannot use. Each patch names
  its commit; drop them when Weston moves to a neatvnc that has them.
  * The classic password is a DES challenge the server checks with the
    password itself, so PAM cannot check it. `tessaro-weston-config` writes
    `KIOSK_VNC_PASSWORD` from the image defaults to
    `/run/weston/vnc-password` (0600, the weston user's), and the child gets
    `--vnc-password-file=`, an option the same Weston patch adds. Only its
    first 8 characters count, as in every VNC server, and the login has no
    username. The file is written only on the real run, never into the
    scratch config the agent compares on hotplug.
  * macOS Screen Sharing answers a 3.8 server with RFB 3.3, which has no
    list of logins: the server names one, and names the classic password.
    neatvnc 0.8.1 refused every version but 3.8.
  * macOS asks for 32 bits per pixel with depth 32 though its colours take
    24. neatvnc sized ZRLE's compact pixels from the depth and sent 4 bytes
    where the viewer reads 3; the viewer hung up after the first frame. The
    depth is now recomputed from the colour masks.
* **The credential is `tessaro` / `tessaro`, and it is deliberately not a
  system account.** `weston_authenticate_user()` is
  `pam_start("weston-remote-access", <username the client sent>, ...)`, and the
  stack Weston ships is `auth include login` - which can only ever accept one
  username, `weston`. The compositor is unprivileged, and `pam_unix`'s helper
  refuses to check any account but the caller's own: `unix_chkpwd.c:133-146`
  drops its setuid when the requested user differs, and then cannot read
  `/etc/shadow` (0400 root). Weston's own man page says as much - "the VNC
  client has to authenticate as the user running weston". Creating a `tessaro`
  account does *not* work around it; that was tried and every login was
  refused.
  So `weston_%.bbappend` replaces that PAM stack with `pam_exec` running
  `/usr/libexec/tessaro-vnc-auth`, which compares against `KIOSK_VNC_USER` and
  `KIOSK_VNC_PASSWORD` from the image defaults
  (`/usr/lib/tessaro-kiosk/tessaro-kiosk.env`) only. No account, no
  `/etc/shadow`, no privilege. The credential is an image property on
  purpose: there is no setting for it (a test in `keys.rs` keeps it that way),
  `generated.env` is not read, and changing it takes a new image. The stack
  alone was not enough: `vnc_handle_auth` in `vnc.c` also refused any username
  that did not resolve to the compositor's own uid before PAM was ever called,
  and `0001-vnc-let-the-PAM-stack-decide-which-user-may-log-in.patch`, applied
  by the same bbappend, removes that check. It needs `pam-plugin-exec`, which is not in the
  image by default and is an `RDEPENDS` of weston for that reason; without it
  every login fails with a bare `PAM: authentication failed`.
* **Which viewer gets which login.** The viewer picks from the list.
  TigerVNC, Remmina and the viewer built into `tessaro-gui` (VNC in
  [gui.md](gui.md)) take VeNCrypt, `tessaro` / `tessaro`; neatvnc names its
  sub-type TLSPlain on the wire, and its cert is self-signed and identical
  across an image, so the fingerprint warning means nothing. macOS Screen
  Sharing, Royal TSX, RealVNC and noVNC take the classic password and ask for
  the password alone.
* **A viewer that leaves at once does not take the mirror down.** Weston's
  VNC backend made a seat per viewer and destroyed it on disconnect, so its
  `wl_seat` global vanished while screen-share could still be binding it;
  the child answered with `invalid global wl_seat` and exited (`Primary
  client died`). `0004-vnc-keep-one-seat-for-every-client.patch` keeps one
  seat for the backend's life and only adds and releases a viewer's pointer
  and keyboard.
* **Sharing is not free while it is on.** `weston_output_disable_planes_incr()`
  takes the output off hardware overlay and cursor planes for as long as it is
  shared, and every damage rectangle goes through `read_pixels()`. A static
  page is nearly free; full-screen video is a readback per frame. `screen.vnc=off`
  is the first thing to try on a Pi that feels slow.
* **One client at a time** - a second connection disconnects the first - and
  **only outputs present when Weston starts are shared**. A monitor plugged
  in later is picked up by the agent's hotplug restart (see **Display
  hotplug** in [display.md](display.md)), and the share with it.
* **SSH ships in every image**:
  `ssh-server-dropbear empty-root-password allow-empty-password` in
  `moonforge-image-base.bbappend`. `allow-empty-password` adds dropbear's
  `-B`, without which a blank password is refused whatever the hash says. The
  root password is empty only while the device is unclaimed (see the claim
  model in [settings.md](settings.md)), so a fresh or reset device is a root
  shell with no credential - the reason to claim it before it leaves the
  bench. On a claimed device the way in is a key: see **SSH keys** below.

## SSH keys

**`tessaro-ctl --node NAME ssh connect` is a root shell by key, with no password and
no first-use prompt.** It sends your public key (`~/.ssh/id_ed25519.pub` and
the other ssh-keygen defaults, or `--key PATH`) over the pinned, token-
authenticated control connection; the agent adds it to root's
`authorized_keys` and answers with the device's host key; the client writes
that to `~/.config/tessaro/known_hosts` under `tessaro-<node id>` and execs
`ssh -o HostKeyAlias=... -o StrictHostKeyChecking=yes root@<address>`.
Anything after `--` goes to ssh. `--print` pushes the key and prints the
command instead. `tessaro-ctl ssh keys list` and `ssh keys revoke
<fingerprint|prefix|comment>` manage what is there. The logic is
`agent/tessaro-agent/src/ssh.rs`; key parsing is `agent/protocol/src/sshkey.rs`,
shared so both ends refuse the same keys.

* **The claim model owns `authorized_keys`, as it owns the root password.**
  Unclaim, factory reset and revoking the last token empty it, and the boot
  oneshot empties it on any device with no tokens, which heals a power cut
  in the middle of an unclaim. An unclaimed device refuses a key outright.
  Revoking a key never unclaims: only tokens decide that.
* **An unclaimed device is reached by its empty password, not a key, and its
  host key is not checked.** The agent refuses `SshAuthorize` until a claim,
  and the control session to an unclaimed device is not pinned, so a host key
  it reported would prove nothing. So when the welcome says unclaimed,
  `tessaro_client::ssh::authorize` sends nothing, ignores `--key`, leaves
  `known_hosts` alone and builds
  `ssh -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no root@<address>`.
  There is no prompt: OpenSSH tries the `none` method first, and dropbear
  with `-B` accepts it for a blank password - which is also why the GUI's
  VNC tunnel works in `BatchMode`.
* **Options are refused.** A line with `command=`, `from=`, `no-pty` and the
  like is rejected on both ends. Whoever holds a token could otherwise plant a
  forced command for root. Lines already in the file that the agent does not
  understand are kept by add and revoke, and go with unclaim.
* **The file lives in `/root/.ssh`, and `/root` had to be made writable.**
  `ROOT_HOME` is `/root` on a systemd distro (oe-core's
  `init-manager-systemd.inc`, not `/home/root`), it is on the read-only
  rootfs, and dropbear 2022.83 only ever reads `$HOME/.ssh/authorized_keys` -
  there is no option to point it elsewhere. So the volatile-binds bbappend
  binds `/data/overlay-root` over `/root`, like `/home`. It persists and
  survives updates. Every write replaces it whole, `.ssh`
  at 0700 and the file at 0600, set explicitly: dropbear silently ignores a
  key file that is group or world writable.
* **The host key persists because of `overlayfs-etc`.** oe-core's `read_only_rootfs_hook` moves dropbear's key to tmpfs
  `/var/lib/dropbear` - a new key every boot - but only on images without
  `overlayfs-etc`. Ours has it, so the key stays in `/etc/dropbear` on the
  overlay (the built rootfs's `/etc/default/dropbear` has no
  `DROPBEAR_RSAKEY_DIR`). Losing `overlayfs-etc` would bring the per-boot key
  back, and the pin with it would refuse every login after a reboot.
* **Dropbear is socket-activated**, and `dropbearkey.service` only runs on the
  first connection. So a device nobody has logged in to has no host key yet;
  the agent makes it (`dropbearkey -t rsa`, the unit's own command) before
  answering, and the first `tessaro-ctl ssh connect` already gets a pin. When no host
  key can be read, the client drops the stale pin and ssh asks as usual.
* **The address is the one the control connection used**, from the known
  nodes or mDNS, so a device that moved is found the same way `device status` finds it -
  and a different device at the old address fails the TLS pin before any
  key is sent.
