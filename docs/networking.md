# Networking

**NetworkManager**, from `meta-networking`, because WiFi has to be
reconfigurable in the field (`nmtui`, or `tessaro-ctl network`). systemd-networkd
is not built at all (`PACKAGECONFIG:remove:pn-systemd = "networkd"` and
`PACKAGECONFIG:remove:pn-systemd-conf = "dhcp-ethernet"` in `tessaro.conf`). A
technician reaches `nmtui` on `getty@tty1` (Ctrl-Alt-F1 - Weston is on tty7),
the serial console, or SSH.

Things to know:

* **Ethernet DHCP is zero-configuration** through the device's own
  `tessaro-ethernet-dhcp` profile, which takes the first port that comes up -
  see **Network control**. `10-tessaro.conf` sets `no-auto-default=*`, so NM
  makes no `Wired connection 1`; one saved by hand loses to priority 100.
* **Profiles persist for free, state needs a unit.**
  `/etc/NetworkManager/system-connections` is on the `/etc` overlay, so saved
  connections land on `/data` with no work. `/var/lib/NetworkManager` is tmpfs,
  because oe-core's `VOLATILE_BINDS` maps `/var/volatile/lib` over `/var/lib`,
  and `tessaro-network-state.service` binds it to `/data/overlay-nm`.
* **That state unit is deliberately *not* a `VOLATILE_BINDS` entry**, even
  though `/home` is one. Adding `/data/overlay-nm /var/lib/NetworkManager` to
  `VOLATILE_BINDS` is silently broken: every unit volatile-binds generates is
  `DefaultDependencies=no` and `Before=local-fs.target` with **no ordering
  between them**, and the template carries `ConditionPathIsReadWrite=!<where>`.
  Race `var-volatile-lib.service` and you lose both ways - if the tmpfs lands
  first the condition skips your unit without a word, and if yours lands first
  the tmpfs mounts over it. `/home` escapes only because it is not under a
  volatile path. Anything nested under `/var/lib`, `/var/cache`, `/var/spool`
  or `/srv` needs its own unit with `After=var-volatile-<x>.service`.
* **systemd-resolved stays and keeps `/etc/resolv.conf`.** That path is a
  symlink into `/run` recreated by a tmpfiles `L!` line each boot, which is why
  it survives the `/etc` overlay. `10-tessaro.conf` sets
  `dns=systemd-resolved` and `rc-manager=unmanaged` so NM never writes a real
  file there - one that would land in the overlay upper and outlive every
  future image.
* **The NM drop-in lives in `/usr/lib/NetworkManager/conf.d/`** and ships in
  `meta-tessaro-distro/recipes-connectivity/tessaro-network/`.
* **`auth-polkit=root-only` is load-bearing for SSH.** `polkit` is in
  `DISTRO_FEATURES` and in NM's `PACKAGECONFIG`, but the image ships no polkit
  *agent*. Upstream's policy grants `settings.modify.system` and
  `network-control` to `allow_active` and demands `auth_admin_keep` otherwise,
  so without this line `nmtui` saves a profile from a getty on tty1 (an active
  seat) and fails over dropbear with "Not authorized to modify the system
  settings". `nmcli general permissions` should read `yes` throughout.
* **Split packages only.** The plain `networkmanager` package is `ALLOW_EMPTY`
  and `RRECOMMENDS` every plugin built - ppp, wwan, adsl, ovs, bluetooth,
  cloud-setup. The image names `networkmanager-daemon`, `-nmcli`, `-nmtui`,
  `-wifi`. `nmtui` also needs `PACKAGECONFIG:append:pn-networkmanager = " nmtui"`;
  it is not in the recipe's default and pulls `libnewt` from oe-core.
* **WiFi drivers and firmware are both per machine, and both handled on
  the hardware targets.** They are separate things: drivers are
  `kernel-module-*` packages, firmware is `linux-firmware*`. A module is
  only *installed* if something recommends it.
  - `raspberrypi3-64`: `rpi-base.inc` adds `kernel-modules` (every built
    module), and `raspberrypi3-64.conf` adds the bcm43430/43455 rpidistro
    firmware. Nothing to do.
  - `genericx86-64`: meta-yocto-bsp's `genericx86-common.inc` adds
    `kernel-modules linux-firmware`, but linux-yocto's own config builds only
    the old wifi drivers (ath5k/ath9k, brcm, mt7601u, rt2x00), with no
    Intel, ath10k/11k/12k, mt76 or rtw88/rtw89.
    `recipes-kernel/linux/files/tessaro-x86-wireless.cfg` turns those on,
    genericx86-64 only. **Every one is `=m`, never built in**: these chips
    load firmware when the driver probes, a built-in driver probes from the
    initramfs before `/lib/firmware` is mounted, and the load fails with
    `-2`. That fragment makes btusb a module there for the same reason.
    Firmware is complete: oe-core splits that recipe into a few hundred
    packages, but `populate_packages:prepend` in `linux-firmware_*.bb` makes
    the base package `RRECOMMENDS` every split one, and nothing here sets
    `BAD_RECOMMENDATIONS` for them. So every blob lands in the image (a few
    hundred MB); the newer Intel ones come through `-iwlwifi-misc`. To trim
    it, `BAD_RECOMMENDATIONS` the split packages the board does not need, or
    list only the ones it does.
  - `qemux86-64`: neither, which is correct - QEMU emulates no wireless NIC,
    so WiFi is tested on the Pi.
* **`NetworkManager-wait-online.service` *is* enabled** - `preset-all` at rootfs
  time creates `/etc/systemd/system/network-online.target.wants/NetworkManager-wait-online.service`,
  even though `SYSTEMD_SERVICE:networkmanager-daemon` never names it. It is
  inert only because nothing in the image `Wants=` or `Requires=`
  `network-online.target`, so the target is never pulled into a transaction.
  The moment something does - an update agent, a VPN, an MQTT client - that
  unit starts gating boot with `nm-online`'s 30-second default on a link-less
  device. Ship a drop-in from `tessaro-network` capping the timeout at that
  point, and do not add one before, since an override with no consumer just
  rots.

## Network control

**The device manages NetworkManager profiles of its own and switches
between them through ordinary settings**, over the pinned, token-checked
control connection:

| profile | up when |
| --- | --- |
| `tessaro-ethernet-dhcp` | `network.ethernet.mode=dhcp` (the default) |
| `tessaro-ethernet-static` | `network.ethernet.mode=static`, with `network.ethernet.address`, `.gateway`, `.dns` |
| `tessaro-wifi-hotspot` | `network.wifi.mode=hotspot` (the default) and a WiFi device (`network.wifi.interface`, or any for `auto`) exists |
| `tessaro-wifi-client` | `network.wifi.mode=client`, joining `network.wifi.ssid` (`network.wifi.ipv4=dhcp\|static` like Ethernet) |

`tessaro-ctl config set network.ethernet.mode=static network.ethernet.address=192.168.1.50/24
network.ethernet.gateway=192.168.1.1` and `config set network.ethernet.mode=dhcp` switch Ethernet,
`network wifi join SSID` makes WiFi a client (the hotspot goes down), `config set
network.wifi.mode=hotspot` brings the hotspot back, `network.wifi.mode=off` frees the radio.
Profiles made by hand - other ports, anything nmtui saved - are listed by `network
profiles list` and never touched. `network.ethernet.interface` names the managed port;
`auto` leaves the profile unbound, so NetworkManager puts it on the first
Ethernet device that comes up.

**`network.wifi.interface=auto` means whichever WiFi device the device has,
not a fixed name**, because the name depends on the board: the Pi's SDIO
radio is `wlan0`, a PCIe card on x86 gets udev's predictable `wlp1s0`, a USB
dongle `wlx<mac>`. So with `auto`:

* the hotspot and client keyfiles carry no `interface-name`, and
  NetworkManager puts them on the WiFi device there is. This also covers
  boot, where the profiles are rendered before the WiFi driver may have
  loaded and before udev has renamed the device;
* the `network.wifi.nat=0` drop matches `iifname "wl*"`, every WiFi name
  either scheme hands out (`Wifi::nat_match` in `profiles.rs`);
* everything that needs the real device - activating a profile, the
  fallback watch, scanning for `network wifi join` - takes the first
  `wireless` interface in sysfs by name (`profiles::wifi_device`).

With more than one WiFi device, name the one to manage: NetworkManager's
choice and the agent's are then the same.

**A network change is kept only if the device still reaches the network
afterwards, and it is saved only then.** There is no confirm step, on purpose:
a change is one request, and whether it sticks is the device's decision alone,
so a change that takes the operator's own connection away - re-addressing the
link it came in on - is safe by construction. The client explains a lost
connection and `network last` reads the verdict afterwards. `network profiles
list|show`, `network wifi status` and `network wifi scan` read; `network ping HOST`
pings from the device (streamed, like `network speedtest`); `tessaro-ctl device ping`
times the client's own path to the agent - TCP connect, TLS handshake, round
trips - and, like `device id`, needs no token. The logic is `agent/tessaro-agent/src/nm/` (`profiles.rs`
renders, `txn.rs` switches) and `ping.rs`.

* **The profiles are generated, never saved.** The agent renders them as
  keyfiles into `/run/NetworkManager/system-connections` (0600, the in-memory
  directory NetworkManager reads with the highest precedence) from
  `state.json`, `secrets.json` and the node name, and the boot oneshot renders
  them again before NetworkManager starts (`tessaro-config.service` is
  `Before=NetworkManager.service`). So `state.json` is the truth: a reboot at
  any point comes back on the committed configuration, and nothing managed is
  ever written to `/etc`. Only the selected profile of each pair has
  `autoconnect=true`, at priority 100, so it wins over a hand-made profile on
  the same device. The uuids are fixed, the same on every device.
  NetworkManager flags everything under `/run` as unsaved, so `network profiles list`
  lists these as `(managed)` instead; `(not saved)` on any other
  profile means it really is lost at reboot.
* **One change is one transaction** (`nm/txn.rs`): write `txn.json`, take a
  NetworkManager **checkpoint** on the devices involved (with a 150s rollback
  timer of NetworkManager's own, the backstop if the agent dies), write the
  new keyfiles and reload them, bring profiles down and up, set the NAT,
  verify - and only then run the caller's commit, which writes `state.json`
  (and a staged WiFi password to `secrets.json`), and drop the checkpoint. Any
  failure puts the old keyfiles back, then rolls the checkpoint back; nothing
  is saved. The outcome goes to `last.json`. `config set` answers with the checks
  (`Applied.network`); a rolled-back change is an error with the reason.
  Network keys are always applied: `config set --no-apply` refuses them.
* **Verify means, on the device:** what was brought up reaches ACTIVATED
  (failing fast with NetworkManager's reason - `no secrets (wrong
  password?)`), its device gets a global address, a default route is still
  there if there was one, and `--verify` holds: `gateway` (the default, one
  ping from the interface), `HOST` (a ping), `HOST:PORT` (a TCP connect) or
  `none`. About 90s at most; the client waits 180s. Leaving client mode for
  the hotspot or `off` gives up WiFi's route on purpose, so that one change
  does not require the route.
* **A WiFi client that does not connect after boot falls back to the
  hotspot until the next boot**, so a device moved away from its network,
  or whose network changed its password, can still be reached without
  Ethernet. The agent's `watch_wifi` (`control/watchers.rs`) arms while
  `network.wifi.mode=client` has a network, the WiFi interface exists and no
  change runs; if the client is not ACTIVATED within
  `network.wifi.fallback_after` seconds (0 never), it writes
  `/run/tessaro-kiosk/wifi-fallback` (the SSID) and brings up the hotspot
  (`Network::fall_back`, no checkpoint: there was no connection to lose).
  `state.json` still says client. Everything that renders the profiles in
  the agent renders the hotspot while the marker is there, so a claim or an
  agent restart keeps it; the boot oneshot never sees it, since `/run` is
  gone at boot, so every boot tries the client again. Boot only, on
  purpose: once the client has been up (`/run/tessaro-kiosk/wifi-client-seen`)
  the watcher is done, because a kiosk whose router reboots must not end up
  on its hotspot for the rest of the day. A change that leaves the client
  alone (Ethernet, `device.name`) keeps the fallback; `network wifi join` or
  a change to the mode, the interface or the client network ends it once it
  commits. `network wifi status` shows it.
* **An agent that dies half way is rolled back at its next start** (`recover`,
  from `start_control`, retrying for a minute while NetworkManager comes up),
  onto what `state.json` renders. That covers SIGTERM too: the transaction is
  not waited for at shutdown.
* **The transaction runs on a task of its own**, holding the one-at-a-time
  lock the way a speed test holds its own, so a client that is cut off does
  not stop it - the commit happens anyway. A network change is refused while
  an update waits for its reboot, and the `update-commit` step of `update
  send` is refused during one.
  `device.name` is a network key too: it renames the hotspot.
* **The hotspot is `tessaro-<node name>` (read-only
  `network.wifi.hotspot_ssid`), open while the device is unclaimed.**
  `access claim` gives it a random 16-character WPA2 password, stored in
  `/data/tessaro/secrets.json` (0600, never in `state.json`, never shown by
  `config get` or `config keys`) and shown once with the root password; the profiles are
  re-rendered only after the answer is out (`After::Network`), so a claimer on
  the hotspot gets the password before it drops them. `network wifi
  hotspot-password` makes a new one. Unclaim, revoking the last token, a
  factory reset and the boot oneshot's claim invariant open it again. It is
  WPA2 with CCMP and `pmf=1` (disabled): the Pi's brcmfmac refuses clients
  with PMF on in AP mode, and its WPA3 AP support is broken.
* **Hotspot clients get DHCP and DNS from NetworkManager's own dnsmasq**
  (`ipv4.method=shared`, the device at a pinned `address1=10.42.0.1/24`,
  `HOTSPOT_ADDRESS` in `profiles.rs`) and, with `network.wifi.nat=1`, NAT through
  NetworkManager's nftables table (`firewall-backend=nftables` in
  `10-tessaro.conf`). `network.wifi.nat=0` is a table of the agent's, `inet
  tessaro-hotspot`, dropping forwarded traffic from the WiFi interface - NM
  1.46 has no per-connection switch - so clients reach the device itself and
  nothing past it. dnsmasq and nftables are `RDEPENDS` of `tessaro-network`;
  the dnsmasq bbappend removes its resolved drop-in (`DNSStubListener=no`,
  which would break every lookup on the device) and never enables its own
  unit (which would hold port 53). The NAT modules are recommended, since
  linux-yocto builds them as modules.
* **The hotspot's dnsmasq answers the phones' captive portal probes with the
  device itself.** `tessaro-captive.conf` (tessaro-portal) in
  `/etc/NetworkManager/dnsmasq-shared.d/` maps only the probe host names to
  10.42.0.1, which is why the address is pinned; every other name resolves
  as before. How that opens the setup portal is in
  [setup-portal.md](setup-portal.md).
* **Scanning from the hotspot goes through a station interface beside it**
  (`nm/sidescan.rs`), because an access point cannot scan on most radios:
  mac80211 refuses it unless the driver sets `NL80211_FEATURE_AP_SCAN`
  (`ieee80211_scan` in `net/mac80211/cfg.c`), which iwlwifi does not, and
  wpa_supplicant logs `CTRL-EVENT-SCAN-FAILED ret=-95`. So `network wifi
  scan` of an interface whose `iw dev` type is `AP` does not ask
  NetworkManager; it adds `tessaro-scan` on the same phy (a locally
  administered copy of the hotspot's MAC), scans from it with `iw`, deletes
  it, and lists what it found beside NetworkManager's own list while that
  interface stays the hotspot. The hotspot stays up and its clients stay
  connected. A join's security lookup uses the same results. NetworkManager
  never manages the interface (`unmanaged-devices` in `10-tessaro.conf`),
  `net.rs` hides it, so `auto` never picks it, and its name must not start
  with `wl`, which the `wl*` matches above would catch. A radio whose
  interface combinations do not allow a station next to an AP (`iw phy
  <phy> info`) lists nothing new, and the agent's journal says why. `iw` is
  an `RDEPENDS` of `tessaro-network`.
* **WiFi joins are Open, WPA2-PSK and WPA3-SAE.** The security comes from a
  scan, or `--hidden --security`; enterprise (802.1X) and WEP are refused.
  Rejoining the same network keeps its saved password if none is given. The
  password is prompted or read from stdin, never argv, travels as
  `protocol::Secret` (whose `Debug` prints `***`), and is saved only if the
  join holds.
* **nmrs is for reading only.** Saved profiles, access points and WiFi
  devices come from it; checkpoints, activation and reloading are our own
  proxies (`nm/proxy.rs`) on nmrs's connection. It turns on zbus's default
  features, which compile in async-io and friends; they stay idle, since zbus
  still runs on tokio. Do not call the nmrs functions that start
  futures-timer's thread.
* **`network ping` falls back to a raw socket.** It prefers the kernel's ICMP
  datagram sockets, but `net.ipv4.ping_group_range` does not exempt root: the
  kernel's own `1 0` refuses even uid 0 (systemd's default opens it). Refused,
  the agent opens a raw socket, which root may, sets the identifier and the
  IPv4 checksum itself and strips the IP header from replies. The e2e runs
  both.
* **qemu cannot exercise WiFi** - no emulated wireless NIC - so the e2e checks
  the hotspot's keyfile and NAT table, and joins, scans and the hotspot
  itself are tested on real hardware by hand: the Pi (brcmfmac) and an x86
  box with an Intel card (iwlwifi), whose drivers differ in what an access
  point may do.

## Speed test

**`tessaro-ctl network speedtest` measures the device's link, not the client's.** The
agent runs it against speed.cloudflare.com and streams one line per step:
where Cloudflare sees the device from (`/cdn-cgi/trace`), latency (25 empty
requests, less the server's own `Server-Timing`), then download and upload at
100k, 1m, 10m, 25m, 100m up to `--max-size` (default 25m), `--tests` samples
each (default 10), and a result - the median at the largest size that
produced samples, because small payloads never leave slow start. No Ookla,
on purpose: its only client is a closed binary.

* **It is the `cfspeedtest` crate, and that is the one place reqwest and
  rustls are allowed.** cfspeedtest is blocking reqwest on rustls/ring with
  webpki-roots - a large dependency tree and a root store compiled in, so this test (only this test) ignores the device's `/etc/ssl/certs`
  and the extra certificate authorities (see **Certificates**).
  Everything else stays on hyper + native-tls, as the TLS bullet in
  [kiosk-browser.md](kiosk-browser.md) says.
  Fenced into `agent/tessaro-agent/src/speedtest.rs`; do not reach for
  reqwest elsewhere just because it is in the lock.
* **It runs on one `spawn_blocking` thread**, which sends a step per payload
  size down a channel that `server.rs` forwards as events. The reqwest client
  is built and dropped on that thread - a blocking client dropped on the
  runtime thread panics. Every request has a 30s timeout, the thread stops at
  the next step once nobody is listening, and the server gives the whole test
  5 minutes, so the agent never waits on Cloudflare and the watchdog never
  notices. A size that took over 5s is the last one tried (cfspeedtest's own
  rule), which keeps a slow link from spending minutes on 25 MB samples.
* **One at a time.** The lock is held by the thread, not the request, so a
  second test is refused even while an abandoned one is still finishing.
* **It moves a few hundred MB.** Uploads stop at 25m whatever `--max-size`
  says, because cfspeedtest builds the body in memory. On a metered link use
  `--max-size 1m`. The start and the result go to the journal at info.
* **It goes through the proxy while one is set**, and `--no-proxy`
  (`Speedtest.direct`) goes straight out instead, to measure the link rather
  than the proxy. Either way the reqwest client is told explicitly
  (`.proxy()` or `.no_proxy()`), so an `HTTP_PROXY` in the environment never
  decides. See **Proxy**.

## Proxy

**Everything the device fetches from the internet goes through one local
proxy, tinyproxy on `127.0.0.1:3128`, and that proxy forwards to
`network.proxy.url`.** `tessaro-ctl network proxy set URL [--bypass ...]`,
`network proxy off`, `network proxy show` and `network proxy test`; the URL
is `http://host:port` or `socks5://host:port`, with `user:password@` for a
proxy that wants a login. The keys are applied by `Consumer::Proxy`: the agent
renders `/run/tessaro-proxy/tinyproxy.conf` (`render::proxy_config`) and
restarts `tessaro-proxy.service`, or stops it when the URL is emptied.

* **Why a local proxy at all: Chromium cannot log in to one.** Its proxy
  settings take no credentials - an HTTP proxy's 407 becomes a login dialog
  nobody can answer on a kiosk - and it has no SOCKS5 authentication at all.
  tinyproxy (1.11.1 from meta-networking, built `--enable-upstream`) takes
  `Upstream http user:pass@host:port` and `Upstream socks5 ...`, so Chromium,
  the probe, the public address lookup and the speed test all speak plain
  HTTP to loopback and only tinyproxy knows the upstream, its scheme and its
  credentials.
* **What the URL can hold is what tinyproxy's `Upstream` can carry**
  (`conf.c` in 1.11.1): a host name or an IPv4 address - no IPv6 - and a
  port; a user without `:`, a password without `@`, neither with spaces, and
  under 255 bytes together (its Basic auth buffer). `keys::parse_proxy`
  checks exactly that. A password with `$ " ' \` or a backtick has to be
  percent-encoded (`%24`), because no setting value may hold them raw (the
  env-file rule); it is decoded for tinyproxy.
* **The password is stored as typed, in `state.json`.** That is a deliberate
  exception to the rule that secrets live in `secrets.json`: the operator
  chose one URL over a separate password command. So `config get
  network.proxy.url` shows it; `network show`, `network proxy show` and the
  GUI mask it (`keys::masked_proxy`). It never reaches `generated.env`
  (`render::env_file` skips every `Consumer::Proxy` key), and the tinyproxy
  config that holds it decoded is 0600 root in `/run`.
* **Chromium gets a fixed address through its policy**: `ProxyMode
  fixed_servers`, `ProxyServer http://127.0.0.1:3128`, `ProxyBypassList
  <-loopback>`, added only while a proxy is set (`render::policy`). The
  address never changes, so only switching the proxy on or off changes the
  policy - and restarts the browser; a new upstream or password restarts
  tinyproxy alone. Loopback is always direct, so the self-test page,
  `/files/` and CDP are unaffected.
* **The agent tunnels, and does no DNS of its own through a proxy.** The
  probe and the public address lookup (`HyperHttp::with_proxy`, never the
  CDP client) send `CONNECT host:port` to the local proxy for http and https
  alike, then run TLS inside the tunnel; the name is resolved by the proxy,
  which on a network that allows only the proxy is the only thing that can.
  A 407, or the 401 tinyproxy itself answers wrong credentials with
  (`reqs.c`), reaches the journal as "check the user and password", a dead local
  proxy as "see `tessaro-ctl network proxy show`". The proxy keys also carry
  `Consumer::Agent`, so the agent restarts and picks the proxy up.
* **Bypass is tinyproxy's, not Chromium's**: `network.proxy.bypass` becomes
  `Upstream none` lines - a host name matched exactly, a `.domain` as a
  suffix, an address or network under its mask (`hostspec.c`) - next to the
  built-in `localhost` and `127.0.0.0/8`. Everything else goes upstream, so
  the bypass rules are the same for every client.
* **`network proxy test`** fetches Cloudflare's trace through the local
  proxy from the device and says the address the internet sees it at, or
  why not. It uses the proxy even before the agent restarted onto it.
* **Left direct on purpose:** `network ping` (ICMP), the `--verify
  HOST:PORT` check of a network change (it tests the link itself), NTP and
  mDNS.
* **tinyproxy's own unit stays off** (`SYSTEMD_AUTO_ENABLE:pn-tinyproxy =
  "disable"`): it would read `/etc/tinyproxy.conf`, on the `/etc` overlay.
  `tessaro-proxy.service` has `ConditionPathExists=` on the rendered config,
  so without a proxy nothing runs.

## Certificates

**Extra certificate authorities are trusted by the browser and the agent,
on top of the image's `ca-certificates` bundle, never in place of it**: for
an intranet site on an internal CA, or a proxy that inspects TLS.
`tessaro-ctl network certs add FILE | list | revoke CERT`, and the Network
page of `tessaro-gui`. The store is `/data/tessaro/ca-certs`, one
`<sha256>.pem` per certificate (`agent/tessaro-agent/src/certs.rs`); they
are not a setting, since PEM text is exactly what `config set` refuses.

* **Chromium gets them as the `CACertificates` policy**, base64 DER, in the
  rendered `10-tessaro.json` (`render::policy`). The policy is
  `dynamic_refresh` (`CACertificates.yaml` in Chromium's policy templates,
  supported since 132), so an add or a revoke changes what the browser
  trusts without restarting it. A page that already failed is not
  reloaded; the next navigation or probe recovery loads it.
* **The agent's openssl client adds them as roots once per process**
  (`http::trust`, from `main.rs` before the runtime starts), which covers
  the reachability probe, the public address and `network proxy test`.
  An add or a revoke restarts the agent (`After::Restart`) once the answer
  is out; an add that only repeats known certificates restarts nothing.
* **`/etc/ssl/certs` is never touched.** `update-ca-certificates` would copy
  the whole bundle up into the `/etc` overlay, where it would shadow the
  image's forever, and an image update's new roots would never arrive.
  `/usr/local/share/ca-certificates` is on the read-only root anyway.
* **Nothing else on the device sees them**: curl, NetworkManager's
  connectivity check, and the speed test, whose rustls root store is
  compiled in (see **Speed test**).
* **What is accepted**: PEM or DER (the client wraps DER into PEM), one
  certificate or a chain, each stored separately, at most 32 and 64 KiB of
  text per add. A file holding a private key is refused by the client and
  again by the device. A self-signed server certificate works as well as a
  root: Chromium and openssl both treat what is added as a trust anchor.
* **They are configuration, not credentials**: an unclaim keeps them, a
  factory reset (either path) removes the store.
