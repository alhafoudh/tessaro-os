# The setup portal

**A phone sets a device up without a laptop: it scans the QR code on the
welcome page, joins the hotspot, and the phone's own captive portal sheet
opens a one-page setup served by the device.** From there it joins WiFi,
sets Ethernet to DHCP or a static address, sets the kiosk page, the device
name and the timezone, and flips maintenance mode and the debug screen with
one tap each. The page never claims the device; claiming stays
`tessaro-ctl access claim`, and closes the portal.

## How long each part lasts

* **The page answers at `https://10.42.0.1:7400/` for as long as the device
  is unclaimed and its hotspot is up.** A claim closes it: the page carries
  no token, the API answers it `401 token-required`, and the page says setup
  is closed. From then on the device is managed by whoever claimed it.
* **The sign-in sheet ends sooner: with the first change the page saves.**
  `network.wifi.captive` (on by default) turns the sheet on, and the page
  adds `network.wifi.captive=0` to its first successful change
  (`withCaptiveOff` in `index.html`): whoever made the change found the
  page, and a phone joining later should not be sent there uninvited. The
  page's first toggle turns it back on. A claim ends it too.
* **The QR code stays until the claim**, with the page it leads to; after
  the sheet is off, the phone joins and the written address opens the page.

## From the QR code to the sheet

* **The QR code is on the welcome page only while the device is unclaimed
  and its hotspot is up.** That is when the hotspot is open, so the code is
  `WIFI:T:nopass;S:<ssid>;;` and carries no password (`qr.rs`). A claimed
  hotspot has a WPA2 password (see **WiFi** in
  [networking.md](networking.md)), and putting it in a QR code on a public
  screen would give it to everyone walking past. The agent draws the SVG
  itself and puts it in `welcome.json` as `setup.qr`, next to `setup.url`;
  the page shows it with a written fallback, "or join SSID and open" the
  address.
* **The hotspot's address is pinned to 10.42.0.1** (`HOTSPOT_ADDRESS` in
  `nm/profiles.rs`, NM's own default stated in the keyfile), because the
  pieces below name it.
* **The phone's connectivity probe is answered by the device.**
  `tessaro-captive.conf`, in the one directory NM's shared-mode dnsmasq
  reads (`/etc/NetworkManager/dnsmasq-shared.d/`), resolves the probe host
  names of Apple, Android, Windows, Firefox and Linux NetworkManager to
  10.42.0.1 and nothing else, so with `network.wifi.nat` on the phone still
  reaches the internet by every other name. Each probe name is also
  `local=`, so its AAAA query gets no answer instead of Apple's real IPv6
  address: an iPhone prefers that address, sends it over cellular (the
  hotspot has no IPv6) and shows the real "Success" page in the sheet. The
  file never changes; the
  sheet is switched in nginx, so turning it off does not restart the hotspot
  under the phone.
* **nginx decides per probe, from a flag file.** Every request whose `Host`
  is not 10.42.0.1 is a probe (`20-tessaro-portal.conf`). While
  `/run/tessaro-portal/captive` exists it gets a `302` to
  `https://10.42.0.1:7400/`, and a probe that gets a redirect instead of its
  expected answer is what makes a phone show its sign-in sheet. The agent
  keeps the flag, there while the device is unclaimed and
  `network.wifi.captive` is on, from the same loop that writes
  `welcome.json`, and at once after a change of the key.
* **With the sheet off, the probe gets the real server's answer, fetched by
  the device** (nginx resolving through systemd-resolved), so the phone sees
  the internet as the device does. When the device cannot reach it the
  connection is dropped (444): a phone reads that as "no internet", where
  an error page would make an iPhone show the sheet anyway.
* **No DHCP option 114 (RFC 8910).** It cannot be withdrawn without
  restarting the hotspot, and the RFC 8908 API it names needs a certificate
  the phone trusts, which the device's self-signed one is not.

## Where it is served

* **The page and its API are the agent's, on the API's port over TLS**
  ([api.md](api.md)). The agent serves `/usr/share/tessaro-portal` at `/`
  (`KIOSK_PORTAL_ROOT`, `api/statics.rs`), every path it does not know
  answered with `index.html`, and the page calls `/api/v1/...` on its own
  origin. The phone's browser shows a warning for the self-signed
  certificate before the page; that is accepted.
* **nginx on port 80 only redirects.** It listens on every address and
  answers only 10.42.0.0/24: the hotspot's address may not exist yet when
  nginx starts, so it cannot `listen` on it. The loopback server
  (`10-tessaro-selftest.conf`) names `127.0.0.1:80` exactly, which keeps the
  kiosk's own pages on it. Everything outside the hotspot's subnet gets a
  403. A typed `http://10.42.0.1/` is sent to the page as a probe is.
* **The flag's directory is root:www 0750 from tmpfiles**, so nginx's
  workers can read it and only root writes. The agent never makes it:
  without it there is no captive portal.

## What the page may do

**Everything the API offers, as anyone unclaimed may** (the claim model in
[settings.md](settings.md)): nothing is restricted by address, and the page
uses a few endpoints. The journal names the phone by its address, and every
change goes through the same validation, network transaction and rollback
as one from `tessaro-ctl`.

* **It reads** `device/welcome` (which keeps the online check running, see
  below), `device/status` and `config`, every 5s, `network/wifi/scan`, and
  `time/zones`.
* **It changes** settings with `config/set` and joins WiFi with
  `network/wifi/join`.
* **A change that takes the hotspot down loses its answer**: renaming the
  device renames the hotspot, and a WiFi join takes the hotspot and the
  phone with it. The page says the device is still applying it. If the
  device cannot reach the new network's gateway it rolls back to the hotspot
  on its own, as for `tessaro-ctl network wifi join`, and the welcome page
  shows the result.
* **Opening the WiFi section scans, from beside the hotspot.** The page asks
  once per load, and the scan runs on a station interface next to the
  hotspot, so the phone stays connected (see **Scanning from the hotspot**
  in [networking.md](networking.md)). A radio that cannot have that
  interface lists nothing new, so the page always offers a typed SSID.

## Online

**"Online" is whether the agent's last lookup of the public address
answered**: Cloudflare's trace at `https://1.1.1.1/cdn-cgi/trace` (`net.rs`),
the lookup behind `network.public_ip`. It is not the cached address, which a
failure leaves in place. The welcome page and the setup page show it, and
`tessaro.network.online()` answers it to the kiosk page (see
[bridge.md](bridge.md)).

* **It is asked every minute only while someone is looking**: the welcome
  page on screen (`welcome_shown` in `control/watchers.rs`), or
  `device/welcome` read in the last 2 minutes. Otherwise the lookup keeps its
  own rule of running only while a template uses `{network.public_ip}`,
  since a link may be metered. `null` in `welcome.json` means it has not
  been asked yet.

## The page

**`tessaro-portal/files/index.html` is one scrolling page of accordion
sections**, no windows, in tessaro-gui's palette and Manrope (the fonts are
copied from `gui/tessaro-gui/fonts/`), everything inline so it works on a
hotspot with no internet. It leaves a field the user has typed into alone.
Maintenance and the debug screen are one change each, using the
maintenance URL and debug template already set.

## Testing

qemu has no WiFi, so the hotspot, the QR code and the captive sheet are
checked on the Pi with a phone of each kind. The e2e case in the control
lane puts 10.42.0.1 on the guest's loopback and probes with busybox `wget`
through nginx the way a phone's probe goes, and asks the page and its API
from the host over the forward of port 7400 (`support/api.rb`), the way a
phone's browser does.
