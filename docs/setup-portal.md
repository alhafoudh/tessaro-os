# The setup portal

**A phone sets a device up without a laptop: it scans the QR code on the
welcome page, joins the hotspot, and the phone's own captive portal sheet
opens a one-page setup served by the device.** From there it joins WiFi,
sets Ethernet to DHCP or a static address, sets the kiosk page, the device
name and the timezone, and flips maintenance mode and the debug screen with
one tap each. The portal never claims the device; claiming stays
`tessaro-ctl access claim`, and closes the portal.

## How long each part lasts

* **The portal answers at `http://10.42.0.1/` for as long as the device is
  unclaimed and its hotspot is up.** A claim closes it: the API answers 403
  and the page says setup is closed. From then on the device is managed by
  whoever claimed it.
* **The sign-in sheet ends sooner: with the first change the portal saves.**
  `network.wifi.captive` (on by default) turns the sheet on, and the portal
  adds `network.wifi.captive=0` to its first successful `set`
  (`with_captive_off` in `portal.rs`): whoever made the change found the
  portal, and a phone joining later should not be sent there uninvited. The
  portal's first toggle turns it back on. A claim ends it too.
* **The QR code stays until the claim**, with the portal it leads to; after
  the sheet is off, the phone joins and the written address opens the
  portal.

## From the QR code to the sheet

* **The QR code is on the welcome page only while the device is unclaimed
  and its hotspot is up.** That is when the hotspot is open, so the code is
  `WIFI:T:nopass;S:<ssid>;;` and carries no password (`qr.rs`). A claimed
  hotspot has a WPA2 password (see **WiFi** in
  [networking.md](networking.md)), and putting it in a QR code on a public
  screen would give it to everyone walking past. The agent draws the SVG
  itself and puts it in `welcome.json` as `setup.qr`, next to `setup.url`;
  the page shows it with a written fallback, "or join SSID and open
  http://10.42.0.1/".
* **The hotspot's address is pinned to 10.42.0.1** (`HOTSPOT_ADDRESS` in
  `nm/profiles.rs`, NM's own default stated in the keyfile), because the
  pieces below name it.
* **The phone's connectivity probe is answered by the device.**
  `tessaro-captive.conf`, in the one directory NM's shared-mode dnsmasq
  reads (`/etc/NetworkManager/dnsmasq-shared.d/`), resolves the probe host
  names of Apple, Android, Windows, Firefox and Linux NetworkManager to
  10.42.0.1 and nothing else, so with `network.wifi.nat` on the phone still
  reaches the internet by every other name. The file never changes; the
  sheet is switched in nginx, so turning it off does not restart the hotspot
  under the phone.
* **nginx decides per probe, from a flag file.** Every request whose `Host`
  is not 10.42.0.1 is a probe (`20-tessaro-portal.conf`). While
  `/run/tessaro-portal/captive` exists it gets a `302` to the portal, and a
  probe that gets a redirect instead of its expected answer is what makes a
  phone show its sign-in sheet. The agent keeps the flag, there while the
  device is unclaimed and `network.wifi.captive` is on, from the same loop
  that writes `welcome.json`.
* **With the sheet off, the probe gets the real server's answer, fetched by
  the device** (nginx resolving through systemd-resolved), so the phone sees
  the internet as the device does. When the device cannot reach it the
  connection is dropped (444): a phone reads that as "no internet", where
  an error page would make an iPhone show the sheet anyway.
* **No DHCP option 114 (RFC 8910).** It cannot be withdrawn without
  restarting the hotspot, and it names an RFC 8908 API over HTTPS, which the
  portal is not.

## nginx and the socket

* **The portal's server listens on port 80 of every address and answers
  only 10.42.0.0/24.** The hotspot's address may not exist yet when nginx
  starts, so it cannot `listen` on it. The loopback server
  (`10-tessaro-selftest.conf`) names `127.0.0.1:80` exactly, which keeps the
  kiosk's own pages on it. Everything outside the hotspot's subnet, the
  Ethernet side included, gets a 403. An Ethernet network that itself uses
  10.42.0.0/24 would reach it; that is accepted.
* **`/api/` goes to the agent over a unix socket,
  `/run/tessaro-portal/api.sock`** (`portal.rs`, `KIOSK_PORTAL_SOCKET`). Its
  directory is root:www 0750 from tmpfiles, so only root and nginx's workers
  can reach the socket; the browser, running as weston, cannot. The agent
  never makes that directory: without it there is no portal, and the journal
  says `setup portal: not started`.
* **The agent's HTTP is hyper's `http1` server**, one request per connection,
  bodies capped at 64 KiB, every wait under a deadline.

## What the portal may do

**Every route is the control-plane command `tessaro-ctl` would send, run as
`Caller::Portal`,** so the journal names it as `the setup portal (<phone's
address>)` and the command's own validation, network transaction and
rollback all apply.

| Route | Command |
| --- | --- |
| `GET /api/state` | the welcome page's values, `status`, and `config get` of the keys below |
| `GET /api/wifi` | `network wifi scan` |
| `GET /api/zones` | the timezone list of `time` |
| `POST /api/set` | `config set`, of the keys in `KEYS` in `portal.rs` only |
| `POST /api/wifi/join` | `network wifi join` |

* **Only `KEYS` may be set**: what setting a device up takes, the
  maintenance and debug switches, and the sign-in sheet. Anything else is a
  403. Nothing under `access`, no tokens, no passwords, which a test in
  `portal.rs` holds: whoever is on the hotspot can set a device up, not take
  it over.
* **Every route answers 403 once the device is claimed**, whoever asks.
* **A `set` is waited for up to 20s** (`SET_WAIT`), then answered `202
  pending` while it goes on. A change that renames the hotspot (`device.name`)
  takes it down under the phone, and the page must not hang on it. A restart
  the change asks for waits a second after the answer, so nginx has it
  first.
* **A WiFi join is answered `202` at once and runs on**: it takes the
  hotspot down, and the phone with it. If the device cannot reach the new
  network's gateway it rolls back to the hotspot on its own, as for
  `tessaro-ctl network wifi join`. The welcome page shows the result.
* **Opening the WiFi section scans, from beside the hotspot.** The page asks
  once per load, and the scan runs on a station interface next to the
  hotspot, so the phone stays connected (see **Scanning from the hotspot**
  in [networking.md](networking.md)). A radio that cannot have that
  interface lists nothing new, so the page always offers a typed SSID.

## Online

**"Online" is whether the agent's last lookup of the public address
answered**: Cloudflare's trace at `https://1.1.1.1/cdn-cgi/trace` (`net.rs`),
the lookup behind `network.public_ip`. It is not the cached address, which a
failure leaves in place. The welcome page and the portal show it, and
`tessaro.network.online()` answers it to the kiosk page (see
[bridge.md](bridge.md)).

* **It is asked every minute only while someone is looking**: the welcome
  page on screen (`welcome_shown` in `control/watchers.rs`), or the portal read
  in the last 2 minutes. Otherwise the lookup keeps its own rule of running
  only while a template uses `{network.public_ip}`, since a link may be
  metered. `null` in `welcome.json` means it has not been asked yet.

## The page

**`tessaro-portal/files/index.html` is one scrolling page of accordion
sections**, no windows, in tessaro-gui's palette and Manrope (the fonts are
copied from `gui/tessaro-gui/fonts/`), everything inline so it works on a
hotspot with no internet. It polls `/api/state` every 5s and leaves a field
the user has typed into alone. Maintenance and the debug screen are one
`set` each, using the maintenance URL and debug template already set.

## Testing

qemu has no WiFi, so the hotspot, the QR code and the captive sheet are
checked on the Pi with a phone of each kind. The e2e case in the control
lane puts 10.42.0.1 on the guest's loopback and asks with busybox `wget`,
which runs the guest's requests through nginx and the socket the way a
phone's go.
