# Tessaro

**A web kiosk that looks after itself.** Tessaro turns a Raspberry Pi or an
x86 PC into a screen that boots straight into your site, fullscreen, and keeps
it there - through crashed tabs, dead networks and power cuts - while you
manage it from your laptop with one command.

![The self-test page, at http://127.0.0.1/selftest.html](docs/images/selftest.jpg)

```sh
tessaro-ctl -n golden-thistle-5731 config set browser.url=https://menu.example.com/
```

That is the whole deployment step. The device restarts only what that setting
touches, and the URL survives reboots and image updates.

## What it does for you

- **The page stays up.** An agent watches Chromium over the DevTools protocol.
  A crashed tab is reloaded in seconds, a wedged browser is restarted, and
  while your site is unreachable a local offline page is shown until it comes
  back. The agent is itself under a systemd watchdog.
- **It stays on your site.** If a link takes the browser to another origin,
  it is brought back. Pages within your origin and redirects you control are
  left alone.
- **It cannot be broken by accident.** The system is read-only; settings,
  your files and the browser profile live on a separate `/data` partition. A
  factory reset is one command, or one word typed at the boot loader.
- **Updates over the network that keep everything.** Send a new image and the
  device checks it, writes it from its initramfs, verifies every block and
  reboots into it, with settings, ownership and browser storage intact. A
  power cut at any point leaves either the old system or a retry, never half
  of one.
- **Network changes that cannot lock you out.** A new static address or WiFi
  network is kept only if the device still reaches the network afterwards.
  Otherwise the device rolls it back by itself, even if the change cut you off.
- **Built for real kiosk hardware.** Touch screens, an on-screen keyboard that
  appears only when no keyboard is plugged in, screens plugged in after boot,
  sound on HDMI, the jack or USB, and WebSerial and WebHID granted to your site
  with no permission prompt, so the page can talk to serial and HID devices
  on a screen nobody is standing at.
- **Content that works offline.** Sync a directory of videos, images and JSON
  to the device and your page loads them from `http://127.0.0.1/files/`, with
  or without a network.
- **Owned by whoever claims it first.** A fresh device has no password, and
  until it is claimed anyone on its network can manage it with `tessaro-ctl`
  or `tessaro-gui`. Claiming gives you a token, pins the device's certificate
  and sets a random root password; from then on only token holders get in.
  Everything goes over TLS.

## Quick start

You build the image yourself, on a Linux machine with Docker,
[kas](https://kas.readthedocs.io/) (`pipx install kas`, for `kas-container`)
and [mise](https://mise.jdx.dev/):

```sh
git clone git@github.com:alhafoudh/tessaro-os.git && cd tessaro-os
mise trust && mise install
mise run image:build:rpi      # Raspberry Pi 3B/3B+; image:build:x86 for a UEFI PC
```

The first build fetches and compiles everything, Chromium included, and takes
hours; later ones reuse the cache. Write the image to a card or disk:

```sh
bmaptool copy build/raspberrypi3-64/tmp/deploy/images/raspberrypi3-64/tessaro-os-raspberrypi3-64.rootfs.wic.bz2 /dev/sdX
```

(From a separate workstation, `mise run image:pull` and `mise run
image:flash` do the same - see [DEVELOPMENT.md](DEVELOPMENT.md).)

Boot it with a network cable in. It comes up on its welcome page, which shows
its name, its address and the command that claims it, and announces itself on
the local network. Build the client, find the device and
claim it:

```sh
mise run ctl:build                  # build/cargo-target/release/tessaro-ctl; put it on your PATH
tessaro-ctl nodes list              # devices answering on this network
tessaro-ctl -n golden-thistle-5731 access claim
```

```
...
Pin it and continue? [y/N] y
claimed golden-thistle-5731 (d77857317a77452baadbbde45de78ba7)
token 4cd4cf8a saved in ~/.config/tessaro/nodes.json

root password - shown this once, store it now:

    ********************
```

Then point it at your site:

```sh
tessaro-ctl -n golden-thistle-5731 config set browser.url=https://menu.example.com/
```

```
revision 3: browser.url
restarting tessaro-kiosk.service, tessaro-agent.service
```

Set `TESSARO_NODE=golden-thistle-5731` and the `-n` can go. The examples
below leave it out.

## Using it

### See what a device is doing

```
$ tessaro-ctl device status
name         golden-thistle-5731
node id      d77857317a77452baadbbde45de78ba7
machine      qemux86-64
agent        1.0.0
fingerprint  d93f5f87bc4b45d9dfb36092abcf3cbe0547a3fe898092a7b0b66598bb075360
claimed      no
os           Tessaro OS 0.1 (main), image 0
revision     2
data         7.8 GB free of 8.2 GB (1% used)
browser url  http://127.0.0.1/
showing      http://127.0.0.1/
browser      answering
  tessaro-agent.service    active
  tessaro-kiosk.service    active
  weston.service           active
audio        usb 80%
time         UTC, in sync
```

```sh
tessaro-ctl device logs -f -u tessaro-agent.service   # the agent's journal, live
tessaro-ctl device ping                               # latency from here to the device
```

### One image, a different page per screen

Any setting can be a placeholder in the URL, and `data.*` keys are yours to
define. Give each device its own value and they all run the same image:

```sh
tessaro-ctl config set 'browser.url=https://menu.example.com/?table={data.table}' data.table=12
tessaro-ctl config set 'browser.url=https://{device.name}.signage.example.com/'
```

Values are percent-encoded into the URL, so a value can never change where
the URL points.

### Every setting documents itself

```
$ tessaro-ctl config keys browser.url
browser.url
    The page the kiosk shows (default: the welcome page, http://127.0.0.1/; the self-test is http://127.0.0.1/selftest.html). A new origin also re-grants the device APIs to it.
    value     http://127.0.0.1/  (default)
    accepts   an http, https, file or data URL; may contain {key} placeholders - any setting's key, e.g. {data.table} or {device.name}
    restarts  the agent (invisible on screen)
    env       KIOSK_URL
```

`tessaro-ctl config keys` lists them all, `config get` shows what a device is
using, and `config unset KEY` goes back to the image default.

### Maintenance and debug screens

```sh
tessaro-ctl config set 'data.msg=We are restocking the shelves. Back at 14:00.' \
  'browser.maintenance.url=http://127.0.0.1/maintenance.html?message={data.msg}'
tessaro-ctl browser maintenance on     # `off` goes straight back to your site
```

![The maintenance page with a custom message](docs/images/maintenance.jpg)

`browser debug on` replaces the page with the device's name and addresses in
large type - the thing a technician in front of the screen needs. The
template is yours too:

```sh
tessaro-ctl browser debug on --template '{device.name}\n\nip     {network.cidr} via {network.gateway}\nmac    {network.mac}\ndata   {storage.data_free} free\n\nurl    {browser.url}'
```

![The debug screen with that template](docs/images/debug-screen.jpg)

Neither restarts the browser, and your site's device permissions stay where
they are.

### Page zoom

```sh
tessaro-ctl browser zoom 125     # like Ctrl+/- in Chrome, 25 to 500; 100 is no zoom
```

It is the same zoom Ctrl+/- sets in a desktop Chrome, for every site and on
top of `screen.scale`. The browser restarts to take it, so the page reloads.

### Files for offline use

```
$ tessaro-ctl files sync ./site-assets
sent       media/promo.mp4  397.1 kB
sent       menu.json  20 B
done: 2 sent (0.4 MB), 0 unchanged

$ tessaro-ctl files list -R
2026-09-24 18:40             media/
2026-09-24 18:40   397.1 kB  media/promo.mp4
2026-09-24 18:40       20 B  menu.json
total              397.1 kB
```

`sync` works like `rsync -r --delete` and asks before removing anything. Your
page reads `http://127.0.0.1/files/media/promo.mp4`, and an https site can
fetch it without a mixed-content or local-network prompt. Uploads are resumed
if the connection drops, and a file only appears once it is complete.

### The screen

```sh
tessaro-ctl screen modes                                  # what the panel offers
tessaro-ctl config set screen.resolution=1920x1080
tessaro-ctl screen confirm                                # within 60 s, or it reverts
tessaro-ctl config set screen.osk=always                  # on-screen keyboard even with a keyboard attached
```

```
screen.resolution=1920x1080 is on probation. Check the screen, then run

    tessaro-ctl screen confirm

within 59s, or it goes back to the default on its own.
```

A mode nobody can see never sticks: without the confirm, and after a reboot,
the device goes back to what worked. High-resolution panels are scaled
automatically.

### The network

```
$ tessaro-ctl network show
hostname     tessaro
interface    enp0s1
address      10.0.2.15/24
gateway      10.0.2.2
public ip    203.0.113.7
dns          10.0.2.3
mac          52:54:00:12:35:02

interfaces:
  enp0s1       ethernet  up       10.0.2.15/24 fec0::5054:ff:fe12:3502/64 fe80::5054:ff:fe12:3502/64 *
  lo           loopback  unknown  127.0.0.1/8 ::1/128
  sit0         virtual   down     -

  * carries the default route. `tessaro-ctl network interfaces` for details.
```

```sh
tessaro-ctl config set network.ethernet.mode=static \
  network.ethernet.address=192.168.1.50/24 network.ethernet.gateway=192.168.1.1
tessaro-ctl network wifi scan
tessaro-ctl network wifi join Office        # prompts for the password
tessaro-ctl network speedtest               # the device's link, not yours
tessaro-ctl network proxy set 'http://jan:s3cret@proxy.corp.test:8080' --bypass .corp.test
tessaro-ctl network proxy test              # the address the internet sees through it
tessaro-ctl network speedtest --no-proxy    # the link itself, around the proxy
```

Behind a corporate proxy everything goes through it - the browser, the
device's own checks and the speed test - over `http://` or `socks5://`, with
a login in the URL if the proxy wants one.

Out of the box the WiFi radio is a hotspot, `tessaro-<device name>`: open
until the device is claimed, then protected by a password shown with the root
password.

### Sound

```
$ tessaro-ctl audio show
output   auto -> QEMU USB Audio Analog Stereo (usb)  80%
input    auto -> (none)  100%
```

```sh
tessaro-ctl audio output hdmi && tessaro-ctl audio volume 60 && tessaro-ctl audio test
```

`auto` picks the USB or Bluetooth device plugged in last, then HDMI with a
screen on it, then the jack. Changes apply to sound already playing, and
nothing restarts.

### Time

```sh
tessaro-ctl time show                                  # timezone, in sync or not, server, offset, drift
tessaro-ctl time timezone Europe/Bratislava            # `time zones` lists them; no restart
tessaro-ctl time ntp on --server ntp1.corp.test --server ntp2.corp.test
tessaro-ctl time ntp off && tessaro-ctl time set       # no time server: this computer's clock
```

Devices start on UTC and take their NTP servers from the network's DHCP,
else a public fallback. A network that blocks outside NTP needs its own
servers named, or TLS fails once the clock drifts.

### Updating

```sh
mise run image:update golden-thistle-5731                  # build host to device; settings stay
tessaro-ctl update send tessaro-os-raspberrypi3-64.rootfs.wic.bz2   # the same, by hand
tessaro-ctl update status
```

`--wipe-data` also starts `/data` over, and `--repartition` rewrites the whole
disk for a device on an older layout. A dropped upload resumes where it
stopped.

### Getting in when you need to

```sh
tessaro-ctl ssh connect                     # root shell with your ~/.ssh key, host key pinned
                                            # (unclaimed: no key, empty password)
tessaro-ctl ssh connect -- journalctl -fu tessaro-kiosk
tessaro-ctl access token create phone       # a token for a second client
tessaro-ctl device factory-reset -y         # settings, owners and files gone
```

Tab completion: `source <(tessaro-ctl completion bash)` (also zsh and
powershell). On the device it is already on.

### The desktop client

**Everything `tessaro-ctl` does, in windows and tables.** `tessaro-gui` shares
the command line's device list, pins and tokens (`~/.config/tessaro/nodes.json`),
so a device claimed with one is open in the other.

```sh
mise run gui:build              # build/gui-target/release/tessaro-gui, for this machine
```

- The device list shows every device answering on the network next to the
  ones you know. Log in to or claim one from there; the certificate is shown
  before it is pinned.
- Each device opens in a window of its own inside the app. It has an overview,
  every setting in tables (double-click to edit), and a page for each command
  group:
  - the screen and its modes, network and WiFi, storage, sound
  - tokens and passwords, SSH keys
  - a file manager for `/data/files`
  - image updates with progress
- The Log page follows the journal live. The VNC panel shows the screen live
  (view only) through an SSH tunnel it sets up itself.
- Keys: Enter confirms, Esc closes, Up and Down move through a table,
  Cmd + and Cmd - zoom.

## Hardware

| Hardware | Machine | State |
| --- | --- | --- |
| Raspberry Pi 3 Model B and B+ | `raspberrypi3-64` | builds, boots and runs the kiosk, GPU rendering |
| x86_64 PCs and mini PCs, UEFI | `genericx86-64` | configured, not yet built on real hardware |
| QEMU x86_64 | `qemux86-64` | for development and the end-to-end tests |

Every machine runs the same image. On the Pi 3, memory is the limit to plan
around: 1 GB, shared with the GPU.

## How it works

Tessaro is a Yocto Linux distribution derived from
[Moonforge](https://moonforgelinux.org/): a read-only root filesystem with
`/etc` as an overlay on a persistent `/data` partition, systemd, Weston as the
compositor and Chromium 147 as the browser. `tessaro-agent`, a Rust service,
supervises the browser over CDP and is the device's control plane;
`tessaro-ctl` talks to it over a local socket on the device or over pinned TLS
from anywhere else, and `tessaro-gui` does the same from a desktop.
[docs/](docs/) explains each part and the reasons behind it.

## More

- [DEVELOPMENT.md](DEVELOPMENT.md) - building, running in QEMU, flashing from
  a workstation, testing the agent.
- [docs/](docs/) - how each subsystem works: the browser, settings and
  claiming, display, networking, updates, sound, remote access, the desktop
  client.
- `tessaro-ctl --help` - worked examples for every command group.
