//! Every setting a device has, in one table.
//!
//! A key is what a technician types (`browser.url`); its `env` name is what the
//! units, `tessaro-weston-config` and the agent itself read (`KIOSK_URL`).
//! Defaults are not in here: they stay in `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`,
//! where a later image can still move them. `state.json` only ever holds the
//! keys someone set.
//!
//! Validation happens once, at `set`, so nothing that reaches `state.json` -
//! and from there the env file systemd parses - can be malformed. The rule
//! every kind shares is the one that matters most: no control characters, no
//! quotes, no backslash and no `$`. A newline would let a value write a second
//! variable into the env file, and systemd's env-file parser gives quotes,
//! backslashes and `${...}` meanings of their own.
//!
//! A key starts with the `tessaro-ctl` group that acts on the same thing
//! (`browser.*`, `screen.*`, `network.*`, `device.*`, `access.*`, `time.*`); a key no
//! group acts on is named after the component it tunes (`agent.*`). A key
//! that is renamed goes into `RENAMED`, so devices in the field follow.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

/// What has to happen for a changed value to take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Consumer {
    /// Read by tessaro-agent at start: the agent restarts itself. Invisible
    /// on screen - the browser is left alone.
    Agent,
    /// Read by tessaro-kiosk.service: the browser restarts.
    Browser,
    /// Read by tessaro-weston-config before the compositor starts: Weston
    /// restarts, and with it (`PartOf=`) the browser and the agent.
    Weston,
    /// The device's own NetworkManager profiles: applied as one network
    /// change the device verifies and rolls back by itself, before the
    /// setting is saved at all. Nothing restarts.
    Network,
    /// PipeWire, through WirePlumber: the agent switches the output, the
    /// input and their volumes on the running sound server at once. Nothing
    /// restarts, and a sound that is playing moves over.
    Audio,
    /// The Raspberry Pi firmware, through `tessaro.txt` on the boot
    /// partition, which it reads only when the board powers on. Nothing
    /// restarts: the change takes effect at the next reboot.
    Firmware,
    /// The system clock, through systemd-timedated and systemd-timesyncd:
    /// the agent sets the timezone and the NTP servers on the running
    /// system. Only systemd-timesyncd restarts, and only when its servers
    /// change; the browser follows `/etc/localtime` by itself.
    Time,
    /// The device's local forwarding proxy, `tessaro-proxy.service`: its
    /// config in `/run` is rendered again, and the unit restarts, or stops
    /// when network.proxy.url is empty. The browser restarts only when the
    /// proxy is switched on or off, which changes its policy.
    Proxy,
}

/// Hardware a key needs. A key that names one is left out of `config keys`
/// and `config get`, and refused by `config set`, on a device without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hardware {
    /// The Raspberry Pi firmware and its `config.txt`.
    PiFirmware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An absolute URL the browser can open: http, https, file or data.
    Url,
    /// The same, or empty.
    OptionalUrl,
    /// The same, empty, or `none` (never navigate away from the site).
    OfflineUrl,
    /// `1`/`0`; `on`, `true` and `yes` are accepted and stored as `1`.
    Flag,
    /// One of a fixed set.
    Choice(&'static [&'static str]),
    /// A whole number in a range.
    Int { min: i64, max: i64 },
    /// Chromium command-line flags, split at whitespace by systemd.
    Args,
    /// A comma-separated list of Chromium `base::Feature` names.
    Features,
    /// Web origins (`scheme://host[:port]`), space or comma separated.
    Origins,
    /// Weston output scale: empty or `auto`, `none`, or 1 to 4.
    Scale,
    /// Output mode: `preferred`, or `WIDTHxHEIGHT`.
    Resolution,
    /// A DNS label, or empty for the name derived from the node id.
    Name,
    /// `off`, or an `address:port` to listen on.
    Listen,
    /// A free-form custom value, used in browser.url as `{data.<name>}`.
    Param,
    /// Text for the debug screen: `{key}` placeholders like browser.url, and
    /// `\n` - a literal backslash and `n` - for a line break, the one
    /// backslash any value may carry.
    Template,
    /// An interface name, or `auto`.
    Interface,
    /// An IPv4 address with its prefix: `192.168.1.50/24`, or empty.
    Cidr,
    /// One IPv4 address, or empty.
    Address,
    /// IPv4 addresses, comma separated, or empty.
    Addresses,
    /// A WiFi network name: 1 to 32 bytes.
    Ssid,
    /// A file in the store, `/data/files`, from its root: `inject.js` or
    /// `/inject.js`, stored without the leading `/`. Or empty.
    StoreFile,
    /// Where sound plays: `auto`, `off`, a kind of output (`AUDIO_OUTPUTS`),
    /// or one output's exact PipeWire name from `tessaro-ctl audio outputs`.
    AudioOutput,
    /// Where sound is recorded from: the same, with `AUDIO_INPUTS`.
    AudioInput,
    /// A tz database name, `Europe/Bratislava` or `UTC`, from `tessaro-ctl
    /// time zones`. Whether the device has it is the device's check.
    Timezone,
    /// Host names or IP addresses, comma separated, or empty.
    Hosts,
    /// An upstream proxy: `http://host:port` or `socks5://host:port`, with
    /// an optional `user:password@`, or empty for none. See `parse_proxy`.
    ProxyUrl,
    /// What goes around the proxy, comma separated: host names, `.domain`
    /// suffixes, IP addresses and networks (`10.0.0.0/8`).
    Bypass,
    /// Not a setting: something the device reports - its address, its id.
    /// Listed with `config keys`, readable with `config get`, usable in
    /// browser.url, and refused by `config set`.
    ReadOnly,
}

impl Kind {
    /// What a value of this kind may be, for `tessaro-ctl config keys`.
    pub fn describe(&self) -> String {
        match self {
            Kind::Url => {
                "an http, https, file or data URL; may contain {key} placeholders - any setting's key, e.g. {data.table} or {device.name}"
                    .to_string()
            }
            Kind::OptionalUrl => "an http, https, file or data URL, or empty".to_string(),
            Kind::OfflineUrl => "a URL, empty, or none".to_string(),
            Kind::Flag => "1 or 0 (on/off, true/false, yes/no)".to_string(),
            Kind::Choice(choices) => format!("one of: {}", choices.join(", ")),
            Kind::Int { min, max } => format!("a whole number, {min} to {max}"),
            Kind::Args => "Chromium command-line flags, space separated".to_string(),
            Kind::Features => "Chromium feature names, comma separated, no spaces".to_string(),
            Kind::Origins => "origins (scheme://host[:port]), space or comma separated".to_string(),
            Kind::Scale => "auto, none, or 1 to 4".to_string(),
            Kind::Resolution => {
                "preferred, or WIDTHxHEIGHT from `tessaro-ctl screen modes`".to_string()
            }
            Kind::Name => "letters, digits and dashes, up to 40; empty derives one".to_string(),
            Kind::Listen => "address:port, or off".to_string(),
            Kind::Param => "any text; percent-encoded where browser.url uses it".to_string(),
            Kind::Template => {
                "text; \\n breaks a line; {key} placeholders as in browser.url, plus {browser.url}"
                    .to_string()
            }
            Kind::Interface => "an interface name (eth0, enp1s0, wlan0), or auto".to_string(),
            Kind::Cidr => "ADDRESS/PREFIX, e.g. 192.168.1.50/24, or empty".to_string(),
            Kind::Address => "an IPv4 address, or empty".to_string(),
            Kind::Addresses => "IPv4 addresses, comma separated, or empty".to_string(),
            Kind::Ssid => "a WiFi network name, 1 to 32 bytes".to_string(),
            Kind::StoreFile => {
                "a file in the store from its root, e.g. inject.js, or empty".to_string()
            }
            Kind::AudioOutput => format!(
                "one of: {}, or an output's name from `tessaro-ctl audio outputs`",
                AUDIO_OUTPUTS.join(", ")
            ),
            Kind::AudioInput => format!(
                "one of: {}, or an input's name from `tessaro-ctl audio inputs`",
                AUDIO_INPUTS.join(", ")
            ),
            Kind::Timezone => {
                "a timezone, e.g. Europe/Bratislava or UTC, from `tessaro-ctl time zones`".to_string()
            }
            Kind::Hosts => "host names or IP addresses, comma separated, or empty".to_string(),
            Kind::ProxyUrl => {
                "http://[user:password@]host:port or socks5://[user:password@]host:port, or empty; percent-encode $ \" ' \\ ` @ in a password"
                    .to_string()
            }
            Kind::Bypass => {
                "host names, .domain suffixes, IP addresses or networks (10.0.0.0/8), comma separated"
                    .to_string()
            }
            Kind::ReadOnly => "read-only: reported by the device, cannot be set".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub name: &'static str,
    pub env: &'static str,
    pub kind: Kind,
    pub consumers: &'static [Consumer],
    /// A change to this key is applied on probation: it reverts on its own
    /// unless confirmed, because a wrong value can leave nobody able to see
    /// the screen to put it right.
    pub guarded: bool,
    /// The hardware the key needs, if it is not every device's.
    pub only: Option<Hardware>,
    pub doc: &'static str,
}

const AGENT: &[Consumer] = &[Consumer::Agent];
const NOBODY: &[Consumer] = &[];
const BROWSER: &[Consumer] = &[Consumer::Browser];
const WESTON: &[Consumer] = &[Consumer::Weston];
const NETWORK: &[Consumer] = &[Consumer::Network];
const AUDIO: &[Consumer] = &[Consumer::Audio];
const FIRMWARE: &[Consumer] = &[Consumer::Firmware];
const TIME: &[Consumer] = &[Consumer::Time];
/// The proxy keys: the local proxy, and the agent, whose probe and public
/// address lookup go through it.
const PROXY: &[Consumer] = &[Consumer::Proxy, Consumer::Agent];
/// The node name: the agent's mDNS name, and the hotspot's SSID.
const AGENT_AND_NETWORK: &[Consumer] = &[Consumer::Agent, Consumer::Network];

const fn key(
    name: &'static str,
    env: &'static str,
    kind: Kind,
    consumers: &'static [Consumer],
    doc: &'static str,
) -> Key {
    Key {
        name,
        env,
        kind,
        consumers,
        guarded: false,
        only: None,
        doc,
    }
}

/// A read-only key: no env variable, nothing reads it, `set` refuses it.
const fn live(name: &'static str, doc: &'static str) -> Key {
    key(name, "", Kind::ReadOnly, NOBODY, doc)
}

const fn seconds(
    name: &'static str,
    env: &'static str,
    min: i64,
    max: i64,
    doc: &'static str,
) -> Key {
    key(name, env, Kind::Int { min, max }, AGENT, doc)
}

/// The registry. The VNC credential (`KIOSK_VNC_USER`/`KIOSK_VNC_PASSWORD`) is
/// deliberately absent: it is static, an image property, not a setting.
pub static KEYS: &[Key] = &[
    key(URL, "KIOSK_URL", Kind::Url, AGENT,
        "The page the kiosk shows (default: the welcome page, http://127.0.0.1/; the self-test is http://127.0.0.1/selftest.html). A new origin also re-grants the device APIs to it."),
    key(PROBE_URL, "KIOSK_PROBE_URL", Kind::OptionalUrl, AGENT,
        "Health endpoint to probe instead of browser.url; empty probes browser.url. Needed for a file: or data: kiosk."),
    key("browser.offline_url", "KIOSK_OFFLINE_URL", Kind::OfflineUrl, AGENT,
        "Page shown while the site is down; empty stages /data/kiosk/offline.html or the shipped page; none stays on the site."),
    key("browser.enforce_origin", "KIOSK_ENFORCE_ORIGIN", Kind::Flag, AGENT,
        "Bring the browser back when it leaves the kiosk origin. 0 for a site that hands visitors to another host."),
    key(MAINTENANCE_ENABLE, "KIOSK_MAINTENANCE", Kind::Flag, AGENT,
        "Maintenance mode: show browser.maintenance.url instead of browser.url, which is kept as it is. `tessaro-ctl browser maintenance on|off`."),
    key(MAINTENANCE_URL, "KIOSK_MAINTENANCE_URL", Kind::Url, AGENT,
        "The page shown in maintenance mode (default: http://127.0.0.1/maintenance.html, which takes ?title= and ?message=)."),
    key(DEBUG_ENABLE, "KIOSK_DEBUG_SCREEN", Kind::Flag, AGENT,
        "Show browser.debug.template full screen instead of the kiosk page. Not agent.debug, which is journal verbosity. `tessaro-ctl browser debug on|off`."),
    key(DEBUG_TEMPLATE, "KIOSK_DEBUG_TEMPLATE", Kind::Template, AGENT,
        "What the debug screen shows: text with {key} placeholders, \\n for a new line, e.g. IP {network.ip}\\nGW {network.gateway}."),
    key(INJECT_SCRIPT, "KIOSK_INJECT_SCRIPT", Kind::StoreFile, AGENT,
        "A script from the file store run in every page before the page's own, e.g. inject.js for /data/files/inject.js. Empty for none. `tessaro-ctl browser inject on|off`."),
    key(BRIDGE_MODE, "KIOSK_BRIDGE_MODE", Kind::Choice(BRIDGE_MODES), AGENT,
        "What the page gets as window.tessaro: off, config (the settings, read-only) or actions (the settings and device actions). `tessaro-ctl browser bridge`."),
    key(ZOOM, "KIOSK_ZOOM", Kind::Int { min: 25, max: 500 }, BROWSER,
        "Page zoom in percent, Chrome's Ctrl+/- zoom for every site, on top of screen.scale. `tessaro-ctl browser zoom`."),
    key("browser.args_extra", "KIOSK_CHROMIUM_ARGS_EXTRA", Kind::Args, BROWSER,
        "Extra Chromium flags after the fixed set, e.g. --disable-pinch. Features go in browser.*_features."),
    key("browser.touch", "KIOSK_TOUCH", Kind::Choice(&["auto", "enabled", "disabled"]), BROWSER,
        "Touch event feature detection."),
    key("browser.enable_features", "KIOSK_ENABLE_FEATURES", Kind::Features, BROWSER,
        "Chromium features to enable, comma separated, e.g. WebBluetooth,WebBluetoothNewPermissionsBackend. Replaces the default: keep the AcceleratedVideoDecode* features (hardware video decode)."),
    key("browser.disable_features", "KIOSK_DISABLE_FEATURES", Kind::Features, BROWSER,
        "Chromium features to disable. Replaces the default: keep FallbackToSWIfGLES3NotSupported (Pi 3 GPU)."),
    key("browser.fps_counter", "KIOSK_FPS_COUNTER", Kind::Flag, BROWSER,
        "Show Chromium's FPS counter in the corner of the screen (--show-fps-counter)."),
    key("browser.device_origins", "KIOSK_DEVICE_ORIGINS", Kind::Origins, BROWSER,
        "Origins granted WebSerial and WebHID besides the kiosk and self-test origins."),
    key("screen.scale", "KIOSK_SCALE", Kind::Scale, WESTON,
        "Weston output scale: auto (2 above 3400px wide), none, or 1-4."),
    Key {
        guarded: true,
        ..key(RESOLUTION, "KIOSK_RESOLUTION", Kind::Resolution, WESTON,
            "Output mode, WIDTHxHEIGHT from `tessaro-ctl screen modes`, or preferred. Reverts unless confirmed with `tessaro-ctl screen confirm`.")
    },
    key(OSK, "KIOSK_OSK", Kind::Choice(&["auto", "always", "never"]), WESTON,
        "On-screen keyboard: auto shows it only without a USB/Bluetooth keyboard."),
    key("screen.vnc", "KIOSK_VNC", Kind::Choice(&["on", "off"]), WESTON,
        "Mirror the screen to VNC on 127.0.0.1:5900."),
    // Sound, on PipeWire. Applied to the running sound server at once; see
    // `tessaro-ctl audio show`.
    key(AUDIO_OUTPUT, "KIOSK_AUDIO_OUTPUT", Kind::AudioOutput, AUDIO,
        "Where sound plays: auto (the latest USB or Bluetooth output, else HDMI with a screen, else the jack), hdmi, jack, usb, bluetooth, off, or one output from `tessaro-ctl audio outputs`. One that is not plugged in plays on auto until it is back."),
    key(AUDIO_VOLUME, "KIOSK_AUDIO_VOLUME", Kind::Int { min: 0, max: 100 }, AUDIO,
        "Output volume in percent, on whichever output is playing."),
    key(AUDIO_MUTE, "KIOSK_AUDIO_MUTE", Kind::Flag, AUDIO,
        "Mute the output; audio.volume is kept for when it is unmuted."),
    key(AUDIO_INPUT, "KIOSK_AUDIO_INPUT", Kind::AudioInput, AUDIO,
        "Where sound is recorded from, for pages that use the microphone: auto (the latest USB or Bluetooth input, else the jack), usb, jack, bluetooth, off (the page records silence), or one input from `tessaro-ctl audio inputs`."),
    key(AUDIO_INPUT_VOLUME, "KIOSK_AUDIO_INPUT_VOLUME", Kind::Int { min: 0, max: 100 }, AUDIO,
        "Input (microphone) level in percent."),
    // The clock, through timedated and timesyncd. Applied to the running
    // system at once; see `tessaro-ctl time show`.
    key(TIMEZONE, "KIOSK_TIMEZONE", Kind::Timezone, TIME,
        "The device's timezone, e.g. Europe/Bratislava, from `tessaro-ctl time zones`. Pages and the journal show local time in it; the browser follows without a restart. `tessaro-ctl time timezone`."),
    key(NTP_ENABLE, "KIOSK_NTP", Kind::Flag, TIME,
        "Keep the clock in sync over NTP. 0 for a network without any time server; then `tessaro-ctl time set` sets the clock by hand. `tessaro-ctl time ntp on|off`."),
    key(NTP_SERVERS, "KIOSK_NTP_SERVERS", Kind::Hosts, TIME,
        "NTP servers, comma separated. Empty uses the servers the network's DHCP offers, else the image's fallback servers."),
    key("agent.enable", "KIOSK_AGENT_ENABLE", Kind::Flag, AGENT,
        "Supervise the browser at all; 0 parks the agent."),
    key("agent.debug", "KIOSK_DEBUG", Kind::Flag, AGENT,
        "Debug lines in the agent's journal."),
    key("agent.watchdog", "KIOSK_WATCHDOG", Kind::Flag, AGENT,
        "Judge the agent's pledges before pinging the systemd watchdog."),
    key("agent.device_access", "KIOSK_DEVICE_ACCESS", Kind::Flag, AGENT,
        "Enable CDP DeviceAccess on the page session."),
    seconds("agent.probe_interval", "KIOSK_PROBE_INTERVAL", 1, 3600,
        "Seconds between probes while the site is up."),
    seconds("agent.probe_interval_fail", "KIOSK_PROBE_INTERVAL_FAIL", 1, 3600,
        "Seconds between probes while the site is down."),
    seconds("agent.probe_connect_timeout", "KIOSK_PROBE_CONNECT_TIMEOUT", 1, 120,
        "Probe connect timeout, seconds."),
    seconds("agent.probe_timeout", "KIOSK_PROBE_TIMEOUT", 1, 120,
        "Probe response timeout, seconds."),
    seconds("agent.fail_threshold", "KIOSK_FAIL_THRESHOLD", 1, 1000,
        "Failed probes before the offline page."),
    seconds("agent.refresh_interval", "KIOSK_REFRESH_INTERVAL", 0, 86400,
        "Seconds between reloads of the kiosk page; 0 never reloads. Set 0 for a manual self-test pass."),
    seconds("agent.offline_refresh", "KIOSK_OFFLINE_REFRESH", 0, 86400,
        "Seconds between reloads of the offline page."),
    seconds("agent.ping_fails", "KIOSK_PING_FAILS", 1, 1000,
        "Failed browser checks before the browser is restarted."),
    seconds("agent.restart_after", "KIOSK_RESTART_AFTER", 0, 100_000,
        "Failed probes while down before the browser is restarted; 0 never."),
    seconds("agent.restart_backoff", "KIOSK_RESTART_BACKOFF", 0, 86400,
        "Minimum seconds between browser restarts."),
    seconds("agent.cdp_timeout", "KIOSK_CDP_TIMEOUT", 1, 120,
        "Budget for one DevTools command, seconds."),
    seconds("agent.cdp_ping", "KIOSK_CDP_PING", 1, 3600,
        "DevTools websocket keepalive, seconds."),
    seconds("agent.cdp_reconnect_max", "KIOSK_CDP_RECONNECT_MAX", 1, 3600,
        "Ceiling on the DevTools reconnect backoff, seconds."),
    key(NAME, "KIOSK_NODE_NAME", Kind::Name, AGENT_AND_NETWORK,
        "The device's name on the network (NAME.local, and the hotspot tessaro-NAME); empty derives one from the node id."),
    Key {
        only: Some(Hardware::PiFirmware),
        ..key(GPU_MEM, "KIOSK_GPU_MEM", Kind::Int { min: 16, max: 512 }, FIRMWARE,
            "Raspberry Pi only: megabytes of RAM the firmware keeps for the GPU, which its hardware video decoder draws from; unset keeps the firmware's default. Taken from the browser's RAM. Takes effect at the next reboot, `tessaro-ctl device reboot`.")
    },
    key("access.listen", "KIOSK_API_LISTEN", Kind::Listen, AGENT,
        "Where the TLS control API listens, address:port, or off."),
    key("access.mdns", "KIOSK_MDNS", Kind::Choice(&["on", "off"]), AGENT,
        "Advertise the device as NAME.local and _tessaro._tcp."),
    // The device's own network: the NetworkManager profiles the agent
    // generates (tessaro-ethernet-dhcp/-static, tessaro-wifi-hotspot/-client)
    // and switches between. A change is kept only if the device still
    // reaches the network afterwards; see `tessaro-ctl config set --verify`.
    key("network.ethernet.interface", "KIOSK_ETHERNET_INTERFACE", Kind::Interface, NETWORK,
        "The Ethernet port the device manages; auto is the first one that comes up. Other ports are left to hand-made profiles."),
    key("network.ethernet.mode", "KIOSK_ETHERNET_MODE", Kind::Choice(&["dhcp", "static"]), NETWORK,
        "dhcp, or static with network.ethernet.address, .gateway and .dns."),
    key("network.ethernet.address", "KIOSK_ETHERNET_ADDRESS", Kind::Cidr, NETWORK,
        "The static address with network.ethernet.mode=static, e.g. 192.168.1.50/24."),
    key("network.ethernet.gateway", "KIOSK_ETHERNET_GATEWAY", Kind::Address, NETWORK,
        "The default gateway with network.ethernet.mode=static; inside network.ethernet.address. Empty for none."),
    key("network.ethernet.dns", "KIOSK_ETHERNET_DNS", Kind::Addresses, NETWORK,
        "DNS servers with network.ethernet.mode=static, comma separated."),
    key("network.wifi.interface", "KIOSK_WIFI_INTERFACE", Kind::Interface, NETWORK,
        "The WiFi device the device manages; auto is whichever there is (wlan0, wlp1s0), so name it when there is more than one. Without one, nothing WiFi ever comes up."),
    key("network.wifi.mode", "KIOSK_WIFI_MODE", Kind::Choice(&["hotspot", "client", "off"]), NETWORK,
        "hotspot (tessaro-NAME, for installation and management), client (joins network.wifi.ssid; `tessaro-ctl network wifi join`; falls back to the hotspot when it does not connect after boot, see network.wifi.fallback_after), or off."),
    key("network.wifi.nat", "KIOSK_WIFI_NAT", Kind::Flag, NETWORK,
        "Let hotspot clients reach the internet and the LAN through the device; 0 lets them reach the device only."),
    key("network.wifi.ssid", "KIOSK_WIFI_SSID", Kind::Ssid, NETWORK,
        "The network network.wifi.mode=client joins. Its password is set by `tessaro-ctl network wifi join` and never shown."),
    key("network.wifi.security", "KIOSK_WIFI_SECURITY", Kind::Choice(&["psk", "sae", "open"]), NETWORK,
        "The client network's security: psk (WPA2), sae (WPA3) or open. `network wifi join` finds it by scanning."),
    key("network.wifi.hidden", "KIOSK_WIFI_HIDDEN", Kind::Flag, NETWORK,
        "The client network does not broadcast its name."),
    key("network.wifi.ipv4", "KIOSK_WIFI_IPV4", Kind::Choice(&["dhcp", "static"]), NETWORK,
        "Client addressing: dhcp, or static with network.wifi.address, .gateway and .dns."),
    key("network.wifi.address", "KIOSK_WIFI_ADDRESS", Kind::Cidr, NETWORK,
        "The static client address with network.wifi.ipv4=static, e.g. 192.168.1.51/24."),
    key("network.wifi.gateway", "KIOSK_WIFI_GATEWAY", Kind::Address, NETWORK,
        "The default gateway with network.wifi.ipv4=static; inside network.wifi.address. Empty for none."),
    key("network.wifi.dns", "KIOSK_WIFI_DNS", Kind::Addresses, NETWORK,
        "DNS servers with network.wifi.ipv4=static, comma separated."),
    // Read by the agent's watcher, not rendered into a profile: the
    // fallback is runtime state in /run, so the saved mode stays client.
    seconds(WIFI_FALLBACK_AFTER, "KIOSK_WIFI_FALLBACK_AFTER", 0, 86400,
        "Seconds network.wifi.mode=client may go without connecting after boot before the device falls back to its hotspot until the next boot; 0 never."),
    // The upstream proxy, through the device's local tinyproxy. The one
    // setting that holds a password as typed: it is stored verbatim in
    // state.json and shown by `config get`, by the operator's choice; the
    // human-readable views mask it. Never written to generated.env.
    key(PROXY_URL, "KIOSK_PROXY_URL", Kind::ProxyUrl, PROXY,
        "The proxy everything reaches the internet through: http://host:port or socks5://host:port, optionally with user:password@; empty for none. `tessaro-ctl network proxy set`."),
    key(PROXY_BYPASS, "KIOSK_PROXY_BYPASS", Kind::Bypass, PROXY,
        "What goes around the proxy, comma separated: host names, .domain suffixes, IP addresses, networks like 192.168.0.0/16. Loopback always does."),
    // Read-only: what the device reports right now. `tessaro-ctl network
    // show` shows the same in full, per interface.
    live(ID, "The node id: systemd's app-specific machine id, never the machine id itself."),
    live("network.hostname", "The kernel hostname."),
    live("network.interface", "The interface carrying the IPv4 default route."),
    live("network.mac", "MAC address of network.interface."),
    live("network.ip", "The first IPv4 address of network.interface."),
    live("network.netmask", "Netmask of network.ip, dotted (255.255.255.0)."),
    live("network.cidr", "network.ip with its prefix length (192.168.1.20/24)."),
    live("network.gateway", "The IPv4 default gateway."),
    live("network.dns", "DNS servers in use, comma separated."),
    live("network.ipv4", "Every IPv4 address on every interface but loopback, comma separated."),
    live("network.ipv6", "Every IPv6 address on every interface but loopback, comma separated."),
    live(PUBLIC_IP, "The address the internet sees, from Cloudflare's trace; looked up by `network show` and `config get network.public_ip`, and every 5 minutes while browser.url uses it."),
    live("network.interfaces", "Every interface but loopback with its state and addresses, as eth0 up 10.0.0.20/24; wlan0 down."),
    live("network.wifi.hotspot_ssid", "The hotspot's network name, tessaro-NAME. Open while the device is unclaimed; claiming it sets a password, shown once."),
    // Read-only, and deliberately never a reason to re-render: free space
    // changes all the time (`state::Live::moves`). `tessaro-ctl storage
    // show` shows the same.
    live("storage.size", "The size of the disk the device runs from, as 64.0 GB."),
    live("storage.unallocated", "Space after the last partition, which `storage grow` gives to /data; 0 B once it has."),
    live("storage.data_size", "The size of the /data filesystem."),
    live("storage.data_free", "Space still free on /data."),
    live("storage.data_used", "How full /data is, in percent (42%)."),
    live("storage.root_free", "Space free on the root filesystem, which is read-only and changes only with an update."),
];

// The names code refers to on its own, not only through the table.
pub const URL: &str = "browser.url";
pub const PROBE_URL: &str = "browser.probe_url";
pub const MAINTENANCE_ENABLE: &str = "browser.maintenance.enable";
pub const MAINTENANCE_URL: &str = "browser.maintenance.url";
pub const DEBUG_ENABLE: &str = "browser.debug.enable";
pub const DEBUG_TEMPLATE: &str = "browser.debug.template";
pub const ZOOM: &str = "browser.zoom";
pub const INJECT_SCRIPT: &str = "browser.inject.script";
pub const BRIDGE_MODE: &str = "browser.bridge.mode";
pub const OSK: &str = "screen.osk";
pub const RESOLUTION: &str = "screen.resolution";
pub const NAME: &str = "device.name";
pub const ID: &str = "device.id";
pub const GPU_MEM: &str = "device.gpu_mem";
pub const PUBLIC_IP: &str = "network.public_ip";
pub const WIFI_FALLBACK_AFTER: &str = "network.wifi.fallback_after";
pub const PROXY_URL: &str = "network.proxy.url";
pub const PROXY_BYPASS: &str = "network.proxy.bypass";
pub const AUDIO_OUTPUT: &str = "audio.output";
pub const AUDIO_VOLUME: &str = "audio.volume";
pub const AUDIO_MUTE: &str = "audio.mute";
pub const AUDIO_INPUT: &str = "audio.input";
pub const AUDIO_INPUT_VOLUME: &str = "audio.input_volume";
pub const TIMEZONE: &str = "time.timezone";
pub const NTP_ENABLE: &str = "time.ntp.enable";
pub const NTP_SERVERS: &str = "time.ntp.servers";

/// The timezone of a device where time.timezone was never set.
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// browser.bridge.mode, from nothing to everything: each mode includes the
/// one before it.
pub const BRIDGE_MODES: &[&str] = &["off", "config", "actions"];

/// The kinds of output audio.output names instead of one output. Besides
/// these, `auto` and `off`.
pub const AUDIO_OUTPUTS: &[&str] = &["auto", "hdmi", "jack", "usb", "bluetooth", "off"];
/// The same for audio.input: there is no HDMI input.
pub const AUDIO_INPUTS: &[&str] = &["auto", "jack", "usb", "bluetooth", "off"];

/// A PipeWire node name as `tessaro-ctl audio outputs` lists it:
/// `alsa_output.platform-bcm2835_audio.stereo-fallback`, or a card and the
/// profile that would bring an output up,
/// `alsa_card.pci-0000_00_1f.3:output:hdmi-stereo`.
pub fn is_audio_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '+' | '@'))
}

/// Every key that was renamed, old name first. Only for devices that still
/// carry the old names in state.json: the boot oneshot rewrites them once
/// (`tessaro-agent boot`), and a `set` or `get` of an old name is refused
/// with the new one. Nothing else accepts them - one spelling per setting.
pub static RENAMED: &[(&str, &str)] = &[
    ("kiosk.url", URL),
    ("kiosk.probe_url", PROBE_URL),
    ("kiosk.offline_url", "browser.offline_url"),
    ("kiosk.enforce_origin", "browser.enforce_origin"),
    ("maintenance.enable", MAINTENANCE_ENABLE),
    ("maintenance.url", MAINTENANCE_URL),
    ("debug.enable", DEBUG_ENABLE),
    ("debug.template", DEBUG_TEMPLATE),
    ("display.scale", "screen.scale"),
    ("display.resolution", RESOLUTION),
    ("display.osk", "screen.osk"),
    ("display.vnc", "screen.vnc"),
    ("node.name", NAME),
    ("node.id", ID),
    ("api.listen", "access.listen"),
    ("api.mdns", "access.mdns"),
    ("ethernet.interface", "network.ethernet.interface"),
    ("ethernet.mode", "network.ethernet.mode"),
    ("ethernet.address", "network.ethernet.address"),
    ("ethernet.gateway", "network.ethernet.gateway"),
    ("ethernet.dns", "network.ethernet.dns"),
    ("wifi.interface", "network.wifi.interface"),
    ("wifi.mode", "network.wifi.mode"),
    ("wifi.nat", "network.wifi.nat"),
    ("wifi.ssid", "network.wifi.ssid"),
    ("wifi.security", "network.wifi.security"),
    ("wifi.hidden", "network.wifi.hidden"),
    ("wifi.ipv4", "network.wifi.ipv4"),
    ("wifi.address", "network.wifi.address"),
    ("wifi.gateway", "network.wifi.gateway"),
    ("wifi.dns", "network.wifi.dns"),
    ("wifi.hotspot_ssid", "network.wifi.hotspot_ssid"),
    ("net.hostname", "network.hostname"),
    ("net.interface", "network.interface"),
    ("net.mac", "network.mac"),
    ("net.ip", "network.ip"),
    ("net.netmask", "network.netmask"),
    ("net.cidr", "network.cidr"),
    ("net.gateway", "network.gateway"),
    ("net.dns", "network.dns"),
    ("net.ipv4", "network.ipv4"),
    ("net.ipv6", "network.ipv6"),
    ("net.public_ip", PUBLIC_IP),
    ("net.interfaces", "network.interfaces"),
];

/// The current name of a key that was renamed.
pub fn renamed(old: &str) -> Option<&'static str> {
    RENAMED
        .iter()
        .find(|(from, _)| *from == old)
        .map(|(_, to)| *to)
}

/// `name is not a setting`, or - for an old name - what it is called now.
pub fn unknown(name: &str) -> String {
    match renamed(name) {
        Some(new) => format!("{name} is now {new}"),
        None => format!("{name} is not a setting; `tessaro-ctl config keys` lists them"),
    }
}

/// `template` with every placeholder that names a renamed key rewritten to
/// the new name; anything else is left exactly as it was.
pub fn rename_placeholders(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open + 1]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder(&after[..close]) => {
                let name = &after[..close];
                out.push_str(renamed(name).unwrap_or(name));
                out.push('}');
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out.push_str(rest);
    out
}

/// Custom values: `data.<name>`, named by whoever sets them. The kiosk gives
/// them no meaning; they exist to be put into browser.url as `{data.<name>}`.
pub const DATA_PREFIX: &str = "data.";

/// Every custom `data.<name>` setting shares this entry. It has no env
/// variable of its own: its value only exists inside the expanded browser.url.
pub static DATA: Key = Key {
    name: "data.<name>",
    env: "",
    kind: Kind::Param,
    consumers: AGENT,
    guarded: false,
    only: None,
    doc: "Custom values with names you choose, for browser.url: data.table=12 fills {data.table}, as in \
          https://menu.test/?table={data.table}. The kiosk gives them no meaning of its own. Set them \
          before or together with a browser.url that uses them. A placeholder is always a full key, so \
          built-in settings work the same way: {device.name}, {screen.scale}, ...",
};

pub fn find(name: &str) -> Option<&'static Key> {
    KEYS.iter()
        .find(|key| key.name == name)
        .or_else(|| param_name(name).map(|_| &DATA))
}

/// `table` for `data.table`, if the rest is a valid placeholder name.
pub fn param_name(key: &str) -> Option<&str> {
    key.strip_prefix(DATA_PREFIX).filter(|name| is_param(name))
}

/// Placeholder names: lower-case letters, digits and `_`, up to 32.
pub fn is_param(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// What a `{name}` in browser.url stands for.
#[derive(Debug, Clone, Copy)]
pub enum Placeholder<'a> {
    /// `{data.table}`: the custom value `data.table`; holds `table`.
    Param(&'a str),
    /// `{device.name}`: the effective value of a registry key.
    Key(&'static Key),
    /// Neither - including `{browser.url}`, `{browser.maintenance.url}` and
    /// `{browser.debug.template}`: a template cannot contain itself or
    /// another one. The debug template resolves `{browser.url}` on its own.
    Unknown,
}

/// Registry entries are statics, one per name, so a key is equal by name.
impl PartialEq for Placeholder<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Placeholder::Param(a), Placeholder::Param(b)) => a == b,
            (Placeholder::Key(a), Placeholder::Key(b)) => a.name == b.name,
            (Placeholder::Unknown, Placeholder::Unknown) => true,
            _ => false,
        }
    }
}

/// How a template's placeholders are filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expansion {
    /// Percent-encoded into a URL.
    Url,
    /// Raw, into text the debug screen escapes for HTML.
    Text,
}

/// The keys whose values are templates, expanded before anyone sees them.
/// No template may name another one as a placeholder.
pub const TEMPLATES: [(&str, Expansion); 3] = [
    (URL, Expansion::Url),
    (MAINTENANCE_URL, Expansion::Url),
    (DEBUG_TEMPLATE, Expansion::Text),
];

/// Is `name` one of the `TEMPLATES`?
pub fn is_template(name: &str) -> bool {
    TEMPLATES.iter().any(|(template, _)| *template == name)
}

/// A placeholder is always a setting's full key: `{data.table}` for the
/// custom `data.table`, `{device.name}` for `device.name`. One rule, no short
/// forms, so a template reads exactly like the `set` that fills it.
pub fn placeholder(name: &str) -> Placeholder<'_> {
    if let Some(custom) = param_name(name) {
        return Placeholder::Param(custom);
    }
    match KEYS.iter().find(|key| key.name == name) {
        Some(key) if !is_template(key.name) && key.kind != Kind::Template => Placeholder::Key(key),
        _ => Placeholder::Unknown,
    }
}

/// What may stand between braces: a parameter name, or a dotted key name.
/// Anything else - `{}`, `{a-b}`, JSON in a data: URL - is left as it is.
fn is_placeholder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.')
}

/// The `{name}` placeholders in a URL template, in order, without repeats.
/// Braces around anything that is not a valid name are left alone.
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut found: Vec<&str> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder(&after[..close]) => {
                let name = &after[..close];
                if !found.contains(&name) {
                    found.push(name);
                }
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    found
}

/// Fill every placeholder from `value`, percent-encoded so a value can never
/// change the URL's structure. Returns the names that had no value; those
/// placeholders expand to nothing.
pub fn expand(template: &str, value: impl Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    expand_with(template, value, percent_encode)
}

/// `expand`, with each value put in through `encode` - raw text for the
/// debug screen, which escapes for HTML itself.
pub fn expand_with(
    template: &str,
    value: impl Fn(&str) -> Option<String>,
    encode: fn(&str) -> String,
) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len());
    let mut missing = Vec::new();
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder(&after[..close]) => {
                let name = &after[..close];
                match value(name) {
                    Some(value) => out.push_str(&encode(&value)),
                    None => {
                        if !missing.iter().any(|known| known == name) {
                            missing.push(name.to_string());
                        }
                    }
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, missing)
}

/// RFC 3986 unreserved characters pass; every other byte is `%XX`.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// What breaks a line in the debug template: a backslash and an `n`, as
/// typed - a real newline could never survive the env file.
pub const LINE_BREAK: &str = "\\n";

pub fn find_env(env: &str) -> Option<&'static Key> {
    KEYS.iter()
        .find(|key| !key.env.is_empty() && key.env == env)
}

/// The value as it will be stored, or why it cannot be.
pub fn validate(key: &Key, value: &str) -> Result<String, String> {
    // The debug template's `\n` is the one backslash allowed anywhere. It
    // never reaches an env file systemd parses unquoted: only the agent reads
    // it, from state.json.
    let checked = if key.kind == Kind::Template {
        value.replace(LINE_BREAK, "")
    } else {
        value.to_string()
    };
    if let Some(bad) = checked
        .chars()
        .find(|ch| ch.is_control() || matches!(ch, '"' | '\'' | '\\' | '$' | '`'))
    {
        return Err(format!("{}: {:?} is not allowed in a value", key.name, bad));
    }

    let value = value.trim();
    let fail = |why: &str| Err(format!("{}: {why}", key.name));

    match key.kind {
        Kind::Url => {
            if value.is_empty() {
                return fail("must not be empty");
            }
            // Checked with every placeholder filled, so a template is held to
            // the same rules as the URL it becomes. Whether each one has a
            // value is the caller's check: it knows the other settings.
            let (sample, _) = expand(value, |_| Some("x".to_string()));
            url(&sample)
                .map(|_| value.to_string())
                .or_else(|why| fail(&why))
        }
        Kind::Param | Kind::Template => Ok(value.to_string()),
        Kind::ReadOnly => fail("is read-only: the device reports it, it cannot be set"),
        Kind::OptionalUrl => {
            if value.is_empty() {
                return Ok(String::new());
            }
            url(value).map(str::to_string).or_else(|why| fail(&why))
        }
        Kind::OfflineUrl => {
            if value.is_empty() || value == "none" {
                return Ok(value.to_string());
            }
            url(value).map(str::to_string).or_else(|why| fail(&why))
        }
        Kind::Flag => match value.to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Ok("1".to_string()),
            "0" | "off" | "false" | "no" => Ok("0".to_string()),
            _ => fail("must be 1 or 0"),
        },
        Kind::Choice(choices) => {
            let lower = value.to_ascii_lowercase();
            if choices.contains(&lower.as_str()) {
                Ok(lower)
            } else {
                fail(&format!("must be one of {}", choices.join(", ")))
            }
        }
        Kind::Int { min, max } => match value.parse::<i64>() {
            Ok(number) if (min..=max).contains(&number) => Ok(number.to_string()),
            _ => fail(&format!("must be a whole number from {min} to {max}")),
        },
        Kind::Args => Ok(value.split_whitespace().collect::<Vec<_>>().join(" ")),
        Kind::Features => {
            if value.chars().any(char::is_whitespace) {
                return fail("feature names are comma separated, without spaces");
            }
            Ok(value
                .split(',')
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(","))
        }
        Kind::Origins => {
            let mut origins: Vec<&str> = Vec::new();
            for candidate in value.split(|ch: char| ch == ',' || ch.is_whitespace()) {
                if candidate.is_empty() {
                    continue;
                }
                if !is_origin(candidate) {
                    return fail(&format!(
                        "{candidate} is not an origin (scheme://host[:port], no path)"
                    ));
                }
                if !origins.contains(&candidate) {
                    origins.push(candidate);
                }
            }
            Ok(origins.join(" "))
        }
        Kind::Scale => match value.to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(String::new()),
            "none" => Ok("none".to_string()),
            number => match number.parse::<u8>() {
                Ok(scale @ 1..=4) => Ok(scale.to_string()),
                _ => fail("must be auto, none, or 1 to 4"),
            },
        },
        Kind::Resolution => match value.to_ascii_lowercase().as_str() {
            "" | "preferred" => Ok("preferred".to_string()),
            mode => match parse_mode(mode) {
                Some((width, height)) => Ok(format!("{width}x{height}")),
                None => fail("must be preferred or WIDTHxHEIGHT, e.g. 1920x1080"),
            },
        },
        Kind::Name => {
            let lower = value.to_ascii_lowercase();
            if lower.is_empty() || is_label(&lower) {
                Ok(lower)
            } else {
                fail("must be letters, digits and dashes, at most 40, not starting or ending with a dash")
            }
        }
        Kind::Listen => {
            if value.eq_ignore_ascii_case("off") {
                return Ok("off".to_string());
            }
            value
                .parse::<SocketAddr>()
                .map(|addr| addr.to_string())
                .or_else(|_| fail("must be off or address:port, e.g. 0.0.0.0:7400"))
        }
        Kind::Interface => {
            if value.is_empty() || value.eq_ignore_ascii_case("auto") {
                return Ok("auto".to_string());
            }
            if is_interface(value) {
                Ok(value.to_string())
            } else {
                fail("must be auto or an interface name (letters, digits, - _ .; up to 15)")
            }
        }
        Kind::Cidr => {
            if value.is_empty() {
                return Ok(String::new());
            }
            parse_cidr(value)
                .map(|(address, prefix)| format!("{address}/{prefix}"))
                .or_else(|why| fail(&why))
        }
        Kind::Address => {
            if value.is_empty() {
                return Ok(String::new());
            }
            value
                .parse::<Ipv4Addr>()
                .map(|address| address.to_string())
                .or_else(|_| fail(&format!("{value} is not an IPv4 address")))
        }
        Kind::Addresses => parse_addresses(value)
            .map(|list| {
                list.iter()
                    .map(Ipv4Addr::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .or_else(|why| fail(&why)),
        Kind::Ssid => {
            if value.is_empty() || value.len() > 32 {
                fail("must be 1 to 32 bytes")
            } else {
                Ok(value.to_string())
            }
        }
        Kind::StoreFile => {
            if value.is_empty() {
                return Ok(String::new());
            }
            match crate::files::normalize(value) {
                Ok(path) if path.is_empty() => fail("names a directory, not a file"),
                Ok(path) => Ok(path),
                Err(why) => fail(&why),
            }
        }
        Kind::AudioOutput | Kind::AudioInput => {
            let (kinds, list) = if key.kind == Kind::AudioOutput {
                (AUDIO_OUTPUTS, "outputs")
            } else {
                (AUDIO_INPUTS, "inputs")
            };
            let lower = value.to_ascii_lowercase();
            if lower.is_empty() {
                Ok("auto".to_string())
            } else if kinds.contains(&lower.as_str()) {
                Ok(lower)
            } else if lower == "hdmi" {
                fail("there is no HDMI input")
            } else if is_audio_name(value) {
                // Whether it exists is the device's check: it knows its
                // outputs.
                Ok(value.to_string())
            } else {
                fail(&format!(
                    "must be one of {}, or a name from `tessaro-ctl audio {list}`",
                    kinds.join(", ")
                ))
            }
        }
        // Whether the zone exists is the device's check: it knows its tz
        // database.
        Kind::Timezone => {
            if is_timezone(value) {
                Ok(value.to_string())
            } else {
                fail("must be a timezone such as Europe/Bratislava or UTC, from `tessaro-ctl time zones`")
            }
        }
        Kind::Hosts => parse_hosts(value)
            .map(|hosts| hosts.join(","))
            .or_else(|why| fail(&why)),
        Kind::ProxyUrl => {
            if value.is_empty() {
                return Ok(String::new());
            }
            parse_proxy(value)
                .map(|proxy| proxy.to_string())
                .or_else(|why| fail(&why))
        }
        Kind::Bypass => parse_bypass(value)
            .map(|entries| entries.join(","))
            .or_else(|why| fail(&why)),
    }
}

/// What a proxy speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyScheme {
    Http,
    Socks5,
}

impl ProxyScheme {
    pub fn word(self) -> &'static str {
        match self {
            ProxyScheme::Http => "http",
            ProxyScheme::Socks5 => "socks5",
        }
    }
}

/// network.proxy.url, taken apart. `user` and `password` are decoded; the
/// `Display` form puts them back percent-encoded, as they are stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
}

impl ProxyUrl {
    /// The URL with the password as `***`, for anything a person reads.
    pub fn masked(&self) -> String {
        self.render(true)
    }

    /// `http user:pass@host:port`: the proxy as tinyproxy's `Upstream`
    /// directive takes it, credentials decoded.
    pub fn upstream(&self) -> String {
        let credentials = match (&self.user, &self.password) {
            (Some(user), Some(password)) => format!("{user}:{password}@"),
            (Some(user), None) => format!("{user}:@"),
            _ => String::new(),
        };
        format!(
            "{} {credentials}{}:{}",
            self.scheme.word(),
            self.host,
            self.port
        )
    }

    fn render(&self, mask: bool) -> String {
        let credentials = match (&self.user, &self.password) {
            (Some(user), Some(_)) if mask => format!("{}:***@", userinfo_encode(user)),
            (Some(user), Some(password)) => {
                format!("{}:{}@", userinfo_encode(user), userinfo_encode(password))
            }
            (Some(user), None) => format!("{}@", userinfo_encode(user)),
            _ => String::new(),
        };
        format!(
            "{}://{credentials}{}:{}",
            self.scheme.word(),
            self.host,
            self.port
        )
    }
}

impl std::fmt::Display for ProxyUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(false))
    }
}

/// The URL as a person should see it: the password masked. Anything that
/// does not parse is shown as it is, since it has no password to hide.
pub fn masked_proxy(value: &str) -> String {
    parse_proxy(value)
        .map(|proxy| proxy.masked())
        .unwrap_or_else(|_| value.to_string())
}

/// `http://[user[:password]@]host:port` or `socks5://...`. The host is a
/// name or an IPv4 address and the port is required: that is what
/// tinyproxy's `Upstream` directive takes (`conf.c`: no IPv6 there). The
/// user and password are percent-decoded; the user cannot hold `:` and
/// neither can hold `@` or whitespace, which that directive cannot carry,
/// and together they stay under the 255 bytes its Basic auth buffer holds.
pub fn parse_proxy(value: &str) -> Result<ProxyUrl, String> {
    let value = value.trim();
    let (scheme, rest) = if let Some(rest) = value.strip_prefix("http://") {
        (ProxyScheme::Http, rest)
    } else if let Some(rest) = value.strip_prefix("socks5://") {
        (ProxyScheme::Socks5, rest)
    } else {
        return Err("must start with http:// or socks5://".to_string());
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#']) {
        return Err("a proxy URL has no path, only host:port".to_string());
    }
    let (userinfo, hostport) = match rest.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, rest),
    };
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| "needs a port, as in http://10.0.0.5:3128".to_string())?;
    let host = host.to_ascii_lowercase();
    if host.starts_with('[') || host.contains(':') {
        return Err("an IPv6 proxy address is not supported; use its host name".to_string());
    }
    if host.parse::<Ipv4Addr>().is_err() && !is_hostname(&host) {
        return Err(format!("{host} is not a host name or an IPv4 address"));
    }
    let port = match port.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(format!("{port} is not a port")),
    };
    let (user, password) = match userinfo {
        None => (None, None),
        Some(userinfo) => {
            let (user, password) = match userinfo.split_once(':') {
                Some((user, password)) => (user, Some(password)),
                None => (userinfo, None),
            };
            let user = percent_decode(user)?;
            let password = password.map(percent_decode).transpose()?;
            if user.is_empty() {
                return Err("the user name is empty".to_string());
            }
            let bad = |text: &str| {
                text.chars()
                    .any(|ch| ch == '@' || ch.is_whitespace() || ch.is_control())
            };
            if user.contains(':') || bad(&user) {
                return Err("the user name cannot contain : @ or spaces".to_string());
            }
            if password.as_deref().is_some_and(bad) {
                return Err("the password cannot contain @ or spaces".to_string());
            }
            if user.len() + password.as_deref().map_or(0, str::len) > 250 {
                return Err(
                    "the user name and password are too long together (250 bytes)".to_string(),
                );
            }
            (Some(user), password)
        }
    };
    Ok(ProxyUrl {
        scheme,
        host,
        port,
        user,
        password,
    })
}

/// `%XX` escapes to the bytes they stand for, as UTF-8.
fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| format!("{text}: a % must be followed by two hex digits"))?;
            out.push(hex);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("{text}: the %-escapes are not UTF-8"))
}

/// Userinfo as it goes back into a URL: unreserved characters and the
/// sub-delims that need no escaping pass; `%`, `:`, `@` and everything a
/// setting value may not hold are escaped.
fn userinfo_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!&()*+,;=".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// network.proxy.bypass: host names, `.domain` suffixes, IP addresses and
/// networks, as tinyproxy's `Upstream none` matches them (`hostspec.c`:
/// a name exactly or, with a leading dot, as a suffix; an address under
/// its mask). Lower-cased, without repeats.
pub fn parse_bypass(value: &str) -> Result<Vec<String>, String> {
    let mut entries: Vec<String> = Vec::new();
    for item in value.split(|ch: char| ch == ',' || ch.is_whitespace()) {
        if item.is_empty() {
            continue;
        }
        let entry = item.to_ascii_lowercase();
        let ok = if let Some(domain) = entry.strip_prefix('.') {
            is_hostname(domain)
        } else if let Some((address, prefix)) = entry.split_once('/') {
            match address.parse::<IpAddr>() {
                Ok(IpAddr::V4(_)) => prefix.parse::<u8>().is_ok_and(|bits| bits <= 32),
                Ok(IpAddr::V6(_)) => prefix.parse::<u8>().is_ok_and(|bits| bits <= 128),
                Err(_) => false,
            }
        } else {
            entry.parse::<IpAddr>().is_ok() || is_hostname(&entry)
        };
        if !ok {
            return Err(format!(
                "{item} is not a host name, a .domain, an IP address or a network"
            ));
        }
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// A name as the tz database spells them: `UTC`, `Europe/Bratislava`,
/// `America/Argentina/Buenos_Aires`, `Etc/GMT+2`. Never a path that could
/// leave `/usr/share/zoneinfo`.
pub fn is_timezone(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '.'))
        })
}

/// Host names and IP addresses, comma or space separated, lower-cased and
/// without repeats. Empty for none.
pub fn parse_hosts(value: &str) -> Result<Vec<String>, String> {
    let mut hosts: Vec<String> = Vec::new();
    for item in value.split(|ch: char| ch == ',' || ch.is_whitespace()) {
        if item.is_empty() {
            continue;
        }
        let host = item.to_ascii_lowercase();
        if host.parse::<IpAddr>().is_err() && !is_hostname(&host) {
            return Err(format!("{item} is not a host name or an IP address"));
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

/// A DNS name: dot-separated labels of letters, digits and dashes, at most
/// 253 characters. A trailing dot is not accepted.
pub fn is_hostname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

/// A value of audio.output or audio.input that names one device rather than
/// a kind of them - which only exists if the device has it now.
pub fn is_audio_device(value: &str) -> bool {
    !AUDIO_OUTPUTS.contains(&value) && !AUDIO_INPUTS.contains(&value)
}

/// A name the kernel could give an interface: `IFNAMSIZ` less the NUL. No
/// `:` - an old-style alias like `eth0:1` is not an interface NetworkManager
/// or `SO_BINDTODEVICE` would take.
pub fn is_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// `is_interface`, as the error a command answers with.
pub fn check_interface(name: &str) -> Result<(), String> {
    if is_interface(name) {
        Ok(())
    } else {
        Err(format!("{name} is not an interface name"))
    }
}

/// `192.168.1.50/24`.
pub fn parse_cidr(value: &str) -> Result<(Ipv4Addr, u8), String> {
    let (address, prefix) = value
        .split_once('/')
        .ok_or_else(|| format!("{value} needs a prefix, as in {value}/24"))?;
    let address = address
        .parse::<Ipv4Addr>()
        .map_err(|_| format!("{address} is not an IPv4 address"))?;
    match prefix.parse::<u8>() {
        Ok(prefix @ 1..=32) => Ok((address, prefix)),
        _ => Err(format!("{value}: the prefix must be 1 to 32")),
    }
}

/// Comma separated, spaces allowed, empty for none.
pub fn parse_addresses(value: &str) -> Result<Vec<Ipv4Addr>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.parse::<Ipv4Addr>()
                .map_err(|_| format!("{item} is not an IPv4 address"))
        })
        .collect()
}

/// Whether `ip` is inside `network/prefix`.
pub fn contains(network: Ipv4Addr, prefix: u8, ip: Ipv4Addr) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    u32::from(network) & mask == u32::from(ip) & mask
}

/// A WPA passphrase: 8 to 63 printable ASCII characters, or the 64 hex
/// digits of a raw key - what wpa_supplicant itself accepts.
pub fn check_psk(psk: &str) -> Result<(), String> {
    if psk.len() == 64 && psk.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(());
    }
    if !(8..=63).contains(&psk.len()) {
        return Err("a WiFi password is 8 to 63 characters, or 64 hex digits".to_string());
    }
    if !psk
        .chars()
        .all(|ch| ch.is_ascii() && !ch.is_ascii_control())
    {
        return Err("a WiFi password is printable ASCII only".to_string());
    }
    Ok(())
}

/// What the network keys must say together, over the effective values (set
/// or image default) that `value` returns. `has_psk` is whether a client
/// password is stored.
pub fn check_network(value: impl Fn(&str) -> String, has_psk: bool) -> Result<(), String> {
    for (mode_key, static_word, prefix) in [
        ("network.ethernet.mode", "static", "network.ethernet"),
        ("network.wifi.ipv4", "static", "network.wifi"),
    ] {
        if value(mode_key) != static_word {
            continue;
        }
        let address = value(&format!("{prefix}.address"));
        let (network, bits) = parse_cidr(&address).map_err(|_| {
            format!("{mode_key}=static needs {prefix}.address, e.g. 192.168.1.50/24")
        })?;
        let gateway = value(&format!("{prefix}.gateway"));
        if !gateway.is_empty() {
            let gateway: Ipv4Addr = gateway
                .parse()
                .map_err(|_| format!("{prefix}.gateway: {gateway} is not an IPv4 address"))?;
            if !contains(network, bits, gateway) {
                return Err(format!(
                    "{prefix}.gateway {gateway} is outside {prefix}.address {address}"
                ));
            }
        }
    }
    if value("network.wifi.mode") == "client" {
        if value("network.wifi.ssid").is_empty() {
            return Err("network.wifi.mode=client needs network.wifi.ssid; \
                 `tessaro-ctl network wifi join SSID` sets both"
                .to_string());
        }
        if value("network.wifi.security") != "open" && !has_psk {
            return Err(format!(
                "{} needs a password; join it with `tessaro-ctl network wifi join {}`",
                value("network.wifi.ssid"),
                value("network.wifi.ssid")
            ));
        }
    }
    Ok(())
}

/// `WIDTHxHEIGHT` with both sides in a plausible range.
pub fn parse_mode(mode: &str) -> Option<(u32, u32)> {
    let (width, height) = mode.split_once('x')?;
    let width: u32 = width.parse().ok()?;
    let height: u32 = height.parse().ok()?;
    ((320..=16384).contains(&width) && (200..=16384).contains(&height)).then_some((width, height))
}

fn url(value: &str) -> Result<&str, String> {
    if value.chars().any(char::is_whitespace) {
        return Err("must not contain whitespace (use %20)".to_string());
    }
    let schemes = ["http://", "https://", "file://", "data:"];
    if !schemes.iter().any(|scheme| value.starts_with(scheme)) {
        return Err("must be an http, https, file or data URL".to_string());
    }
    if (value.starts_with("http://") || value.starts_with("https://")) && origin_of(value).is_none()
    {
        return Err("has no host".to_string());
    }
    Ok(value)
}

/// `scheme://host[:port]` of an http(s) URL, or `None` for anything without
/// one (`file:`, `data:`, or a URL with no host).
pub fn origin_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if authority_end == 0 {
        return None;
    }
    let scheme_len = url.len() - rest.len();
    Some(&url[..scheme_len + authority_end])
}

fn is_origin(candidate: &str) -> bool {
    origin_of(candidate) == Some(candidate)
}

/// A DNS label as mDNS will carry it: lower-case letters, digits, dashes.
pub fn is_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, value: &str) -> Result<String, String> {
        validate(find(name).expect(name), value)
    }

    #[test]
    fn names_and_env_names_are_unique() {
        for (at, key) in KEYS.iter().enumerate() {
            for other in &KEYS[at + 1..] {
                assert_ne!(key.name, other.name);
                // Read-only keys have no variable; every other env is unique.
                if !key.env.is_empty() {
                    assert_ne!(key.env, other.env);
                }
            }
        }
    }

    #[test]
    fn a_newline_cannot_smuggle_a_second_variable_into_the_env_file() {
        for key in KEYS {
            let err = validate(key, "1\nKIOSK_URL=http://evil.test/").unwrap_err();
            assert!(err.contains("not allowed"), "{}: {err}", key.name);
        }
    }

    #[test]
    fn quotes_backslashes_and_dollars_are_refused_everywhere() {
        for bad in ["a\"b", "a'b", "a\\b", "${HOME}", "a`b"] {
            assert!(check("browser.args_extra", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls() {
        assert_eq!(
            check("browser.url", " https://example.com/x ").unwrap(),
            "https://example.com/x"
        );
        assert!(check("browser.url", "").is_err());
        assert!(check("browser.url", "ftp://example.com/").is_err());
        assert!(check("browser.url", "https:///nohost").is_err());
        assert!(check("browser.url", "https://a.test/a b").is_err());
        assert_eq!(check("browser.probe_url", "").unwrap(), "");
        assert!(check("browser.probe_url", "none").is_err());
        assert_eq!(check("browser.offline_url", "none").unwrap(), "none");
        assert!(check("browser.url", "data:text/html,<h1>hi</h1>").is_ok());
    }

    #[test]
    fn flags_are_stored_as_one_and_zero() {
        assert_eq!(check("agent.debug", "on").unwrap(), "1");
        assert_eq!(check("agent.debug", "No").unwrap(), "0");
        assert!(check("agent.debug", "maybe").is_err());
    }

    #[test]
    fn numbers_have_ranges() {
        assert_eq!(check("agent.probe_interval", "15").unwrap(), "15");
        assert!(check("agent.probe_interval", "0").is_err());
        assert!(check("agent.probe_interval", "soon").is_err());
        assert_eq!(check("agent.refresh_interval", "0").unwrap(), "0");
    }

    #[test]
    fn origins_are_deduplicated_and_must_have_no_path() {
        assert_eq!(
            check(
                "browser.device_origins",
                "https://a.test, https://b.test:8443 https://a.test"
            )
            .unwrap(),
            "https://a.test https://b.test:8443"
        );
        assert!(check("browser.device_origins", "https://a.test/path").is_err());
        assert!(check("browser.device_origins", "file:///x").is_err());
        assert_eq!(check("browser.device_origins", "").unwrap(), "");
    }

    #[test]
    fn scale_and_resolution() {
        assert_eq!(check("screen.scale", "auto").unwrap(), "");
        assert_eq!(check("screen.scale", "2").unwrap(), "2");
        assert!(check("screen.scale", "1.5").is_err());
        assert_eq!(
            check("screen.resolution", "1920X1080").unwrap(),
            "1920x1080"
        );
        assert_eq!(check("screen.resolution", "").unwrap(), "preferred");
        assert!(check("screen.resolution", "1920x1080@60").is_err());
        assert!(check("screen.resolution", "10x10").is_err());
        assert!(find("screen.resolution").unwrap().guarded);
    }

    #[test]
    fn features_are_comma_separated() {
        assert_eq!(
            check("browser.enable_features", "WebBluetooth,,Foo").unwrap(),
            "WebBluetooth,Foo"
        );
        assert!(check("browser.enable_features", "A, B").is_err());
    }

    #[test]
    fn names_and_listen_addresses() {
        assert_eq!(check("device.name", "Lobby-1").unwrap(), "lobby-1");
        assert!(check("device.name", "-x").is_err());
        assert!(check("device.name", "a.b").is_err());
        assert_eq!(check("device.name", "").unwrap(), "");
    }

    #[test]
    fn gpu_mem_is_a_pi_firmware_key_in_megabytes() {
        let key = find(GPU_MEM).unwrap();
        assert_eq!(key.only, Some(Hardware::PiFirmware));
        assert_eq!(key.consumers, [Consumer::Firmware]);
        assert_eq!(check(GPU_MEM, "128").unwrap(), "128");
        assert!(check(GPU_MEM, "8").is_err());
        assert!(check(GPU_MEM, "1024").is_err());
        assert!(check(GPU_MEM, "128M").is_err());
        assert_eq!(
            check("access.listen", "0.0.0.0:7400").unwrap(),
            "0.0.0.0:7400"
        );
        assert_eq!(check("access.listen", "OFF").unwrap(), "off");
        assert!(check("access.listen", "7400").is_err());
    }

    #[test]
    fn origin_of_trims_the_path() {
        assert_eq!(origin_of("https://a.test:1/x?y"), Some("https://a.test:1"));
        assert_eq!(origin_of("http://a.test"), Some("http://a.test"));
        assert_eq!(origin_of("file:///x"), None);
    }

    #[test]
    fn the_vnc_credential_is_not_a_setting() {
        assert!(find_env("KIOSK_VNC_USER").is_none());
        assert!(find_env("KIOSK_VNC_PASSWORD").is_none());
    }

    #[test]
    fn any_data_dot_name_is_a_custom_value() {
        assert_eq!(find("data.store").unwrap().kind, Kind::Param);
        assert_eq!(find("data.store_2").unwrap().kind, Kind::Param);
        assert!(find("data.").is_none());
        assert!(find("data.Store").is_none());
        assert!(find("data.a-b").is_none());
        assert!(find("url.store").is_none());
        assert_eq!(param_name("data.lang"), Some("lang"));
        assert_eq!(check("data.store", " 42 ").unwrap(), "42");
        assert!(check("data.store", "a\nb").is_err());
    }

    #[test]
    fn placeholders_are_found_once_and_odd_braces_left_alone() {
        assert_eq!(
            placeholders(
                "https://{data.shop}.test/{data.lang}/x?s={data.shop}&j={not-one}&k={}&n={device.name}&z={.x}"
            ),
            ["data.shop", "data.lang", "device.name"]
        );
    }

    #[test]
    fn a_placeholder_is_always_a_full_key_but_never_the_url_itself() {
        assert_eq!(placeholder("data.store"), Placeholder::Param("store"));
        // No short form: {store} is not data.store.
        assert_eq!(placeholder("store"), Placeholder::Unknown);
        assert_eq!(
            placeholder("device.name"),
            Placeholder::Key(find("device.name").unwrap())
        );
        assert_eq!(
            placeholder("screen.scale"),
            Placeholder::Key(find("screen.scale").unwrap())
        );
        assert_eq!(placeholder("browser.url"), Placeholder::Unknown);
        assert_eq!(placeholder("browser.maintenance.url"), Placeholder::Unknown);
        assert_eq!(
            placeholder("browser.maintenance.enable"),
            Placeholder::Key(find("browser.maintenance.enable").unwrap())
        );
        // An old name is no placeholder; the boot migration rewrites it.
        assert_eq!(placeholder("node.name"), Placeholder::Unknown);
        assert_eq!(placeholder("no.such"), Placeholder::Unknown);
    }

    #[test]
    fn expansion_encodes_values_and_names_what_is_missing() {
        let values = |name: &str| match name {
            "data.shop" => Some("north".to_string()),
            "data.q" => Some("a b&c=d/é".to_string()),
            _ => None,
        };
        let (url, missing) = expand(
            "https://{data.shop}.test/?q={data.q}&x={data.gone}&y={a-b}",
            values,
        );

        assert_eq!(
            url,
            "https://north.test/?q=a%20b%26c%3Dd%2F%C3%A9&x=&y={a-b}"
        );
        assert_eq!(missing, ["data.gone"]);
    }

    #[test]
    fn a_url_template_is_validated_as_the_url_it_becomes() {
        assert_eq!(
            check("browser.url", "https://{data.shop}.test/?lang={data.lang}").unwrap(),
            "https://{data.shop}.test/?lang={data.lang}"
        );
        assert!(check("browser.url", "{data.scheme}://x.test/").is_err());
    }

    #[test]
    fn read_only_keys_are_placeholders_but_not_settings() {
        for name in ["device.id", "network.ip", "network.gateway", "network.ipv4"] {
            let key = find(name).expect(name);
            assert_eq!(key.kind, Kind::ReadOnly);
            assert!(key.env.is_empty() && key.consumers.is_empty(), "{name}");
            assert!(check(name, "1.2.3.4").unwrap_err().contains("read-only"));
            assert_eq!(placeholder(name), Placeholder::Key(key));
        }
    }

    #[test]
    fn the_debug_template_allows_backslash_n_and_no_other_backslash() {
        assert_eq!(
            check(
                "browser.debug.template",
                "IP {network.ip}\\nGW {network.gateway}"
            )
            .unwrap(),
            "IP {network.ip}\\nGW {network.gateway}"
        );
        for bad in ["a\\tb", "a\\\\b", "a\\", "it's", "a\"b", "${HOME}", "a\nb"] {
            assert!(check("browser.debug.template", bad).is_err(), "{bad}");
        }
        // Everywhere else a backslash is still refused.
        assert!(check("data.x", "a\\nb").is_err());
    }

    #[test]
    fn templates_cannot_contain_themselves() {
        assert_eq!(placeholder("browser.debug.template"), Placeholder::Unknown);
        assert_eq!(placeholder("browser.url"), Placeholder::Unknown);
        assert_eq!(
            placeholder("browser.debug.enable"),
            Placeholder::Key(find("browser.debug.enable").unwrap())
        );
    }

    #[test]
    fn every_old_name_leads_to_a_key_that_exists() {
        for (old, new) in RENAMED {
            assert!(find(old).is_none(), "{old} is still a key");
            assert!(find(new).is_some(), "{old} -> {new}, which is no key");
        }
        assert_eq!(unknown("kiosk.url"), "kiosk.url is now browser.url");
        assert!(unknown("no.such").contains("not a setting"));
    }

    #[test]
    fn old_placeholders_are_renamed_and_nothing_else_moves() {
        assert_eq!(
            rename_placeholders(
                "https://{node.name}.test/?ip={net.ip}&t={data.table}&j={\"a\":1}&x={kiosk.url"
            ),
            "https://{device.name}.test/?ip={network.ip}&t={data.table}&j={\"a\":1}&x={kiosk.url"
        );
        let current = "IP {network.ip}\\n{browser.url}";
        assert_eq!(rename_placeholders(current), current);
    }

    #[test]
    fn expansion_can_leave_values_raw() {
        let (text, missing) = expand_with(
            "ip {network.ip} q {data.q}",
            |name| (name == "network.ip").then(|| "10.0.0.2/24 x&y".to_string()),
            str::to_string,
        );
        assert_eq!(text, "ip 10.0.0.2/24 x&y q ");
        assert_eq!(missing, ["data.q"]);
    }

    #[test]
    fn every_kind_describes_itself() {
        for key in KEYS.iter().chain([&DATA]) {
            assert!(!key.kind.describe().is_empty(), "{}", key.name);
        }
    }

    #[test]
    fn network_values_are_normalized() {
        assert_eq!(check("network.ethernet.interface", "").unwrap(), "auto");
        assert_eq!(
            check("network.ethernet.interface", "enp1s0").unwrap(),
            "enp1s0"
        );
        assert!(check("network.ethernet.interface", "eth 0").is_err());
        assert_eq!(
            check("network.ethernet.address", "192.168.1.50/24").unwrap(),
            "192.168.1.50/24"
        );
        assert!(check("network.ethernet.address", "192.168.1.50").is_err());
        assert!(check("network.ethernet.address", "192.168.1.50/33").is_err());
        assert_eq!(check("network.ethernet.gateway", "").unwrap(), "");
        assert!(check("network.ethernet.gateway", "2001:db8::1").is_err());
        assert_eq!(
            check("network.ethernet.dns", "1.1.1.1, 9.9.9.9").unwrap(),
            "1.1.1.1,9.9.9.9"
        );
        assert_eq!(check("network.wifi.mode", "Hotspot").unwrap(), "hotspot");
        assert!(check("network.wifi.ssid", "").is_err());
        assert!(check("network.wifi.ssid", &"x".repeat(33)).is_err());
        assert_eq!(check("network.wifi.ssid", "Office 2").unwrap(), "Office 2");
    }

    #[test]
    fn network_keys_are_checked_together() {
        let with = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
        };
        assert!(check_network(with(&[("network.ethernet.mode", "dhcp")]), false).is_ok());
        let alone = check_network(with(&[("network.ethernet.mode", "static")]), false);
        assert!(alone
            .unwrap_err()
            .contains("needs network.ethernet.address"));
        let outside = check_network(
            with(&[
                ("network.ethernet.mode", "static"),
                ("network.ethernet.address", "10.99.0.5/24"),
                ("network.ethernet.gateway", "192.168.1.1"),
            ]),
            false,
        );
        assert!(outside.unwrap_err().contains("outside"));
        assert!(check_network(
            with(&[
                ("network.ethernet.mode", "static"),
                ("network.ethernet.address", "192.168.1.50/24"),
                ("network.ethernet.gateway", "192.168.1.1"),
            ]),
            false
        )
        .is_ok());

        let client = [
            ("network.wifi.mode", "client"),
            ("network.wifi.ssid", "Office"),
            ("network.wifi.security", "psk"),
        ];
        assert!(check_network(with(&client), false)
            .unwrap_err()
            .contains("network wifi join Office"));
        assert!(check_network(with(&client), true).is_ok());
        let open = [
            ("network.wifi.mode", "client"),
            ("network.wifi.ssid", "Cafe"),
            ("network.wifi.security", "open"),
        ];
        assert!(check_network(with(&open), false).is_ok());
        assert!(check_network(with(&[("network.wifi.mode", "client")]), true).is_err());
    }

    #[test]
    fn audio_values_are_a_kind_of_device_or_one_device() {
        assert_eq!(check("audio.output", "HDMI").unwrap(), "hdmi");
        assert_eq!(check("audio.output", "").unwrap(), "auto");
        assert_eq!(check("audio.output", "off").unwrap(), "off");
        assert_eq!(
            check(
                "audio.output",
                "alsa_output.platform-bcm2835_audio.stereo-fallback"
            )
            .unwrap(),
            "alsa_output.platform-bcm2835_audio.stereo-fallback"
        );
        assert_eq!(
            check(
                "audio.output",
                "alsa_card.pci-0000_00_1f.3:output:hdmi-stereo"
            )
            .unwrap(),
            "alsa_card.pci-0000_00_1f.3:output:hdmi-stereo"
        );
        assert!(check("audio.output", "a b").is_err());
        assert!(check("audio.output", "x/y").is_err());
        assert_eq!(check("audio.input", "USB").unwrap(), "usb");
        assert!(check("audio.input", "hdmi")
            .unwrap_err()
            .contains("no HDMI input"));
        assert!(is_audio_device("alsa_input.usb-mic"));
        assert!(!is_audio_device("jack"));
        assert!(!is_audio_device("auto"));
    }

    #[test]
    fn audio_levels_are_percentages_applied_live() {
        assert_eq!(check("audio.volume", "70").unwrap(), "70");
        assert!(check("audio.volume", "101").is_err());
        assert!(check("audio.input_volume", "-1").is_err());
        assert_eq!(check("audio.mute", "on").unwrap(), "1");
        for name in [
            AUDIO_OUTPUT,
            AUDIO_VOLUME,
            AUDIO_MUTE,
            AUDIO_INPUT,
            AUDIO_INPUT_VOLUME,
        ] {
            assert_eq!(find(name).unwrap().consumers, [Consumer::Audio], "{name}");
        }
    }

    #[test]
    fn timezones_are_tz_names_never_paths() {
        assert_eq!(
            check(TIMEZONE, " Europe/Bratislava ").unwrap(),
            "Europe/Bratislava"
        );
        assert_eq!(check(TIMEZONE, "UTC").unwrap(), "UTC");
        assert_eq!(check(TIMEZONE, "Etc/GMT+2").unwrap(), "Etc/GMT+2");
        assert_eq!(
            check(TIMEZONE, "America/Argentina/Buenos_Aires").unwrap(),
            "America/Argentina/Buenos_Aires"
        );
        for bad in [
            "",
            "/etc/passwd",
            "../../etc/passwd",
            "Europe/../UTC",
            "Europe//Paris",
            "Europe/Bra tislava",
        ] {
            assert!(check(TIMEZONE, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ntp_servers_are_hosts_or_addresses() {
        assert_eq!(
            check(
                NTP_SERVERS,
                "Time.Example.com, 10.0.0.1 2001:db8::1,10.0.0.1"
            )
            .unwrap(),
            "time.example.com,10.0.0.1,2001:db8::1"
        );
        assert_eq!(check(NTP_SERVERS, "").unwrap(), "");
        for bad in ["-bad.test", "a..b", "ntp_1.test", "host.test."] {
            assert!(check(NTP_SERVERS, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn proxy_urls_are_http_or_socks5_with_optional_credentials() {
        assert_eq!(check(PROXY_URL, "").unwrap(), "");
        assert_eq!(
            check(PROXY_URL, " http://10.0.0.5:3128/ ").unwrap(),
            "http://10.0.0.5:3128"
        );
        assert_eq!(
            check(PROXY_URL, "socks5://Proxy.Corp.test:1080").unwrap(),
            "socks5://proxy.corp.test:1080"
        );
        let proxy = parse_proxy("http://jan:pa%24%24w0rd@proxy.test:8080").unwrap();
        assert_eq!(proxy.scheme, ProxyScheme::Http);
        assert_eq!(proxy.user.as_deref(), Some("jan"));
        assert_eq!(proxy.password.as_deref(), Some("pa$$w0rd"));
        assert_eq!(proxy.upstream(), "http jan:pa$$w0rd@proxy.test:8080");
        assert_eq!(proxy.masked(), "http://jan:***@proxy.test:8080");
        // Stored percent-encoded again, so the value itself never holds a $.
        assert_eq!(
            check(PROXY_URL, "http://jan:pa%24%24w0rd@proxy.test:8080").unwrap(),
            "http://jan:pa%24%24w0rd@proxy.test:8080"
        );
        assert_eq!(
            parse_proxy("socks5://u@h.test:1").unwrap().upstream(),
            "socks5 u:@h.test:1"
        );
        assert_eq!(masked_proxy("http://h.test:1"), "http://h.test:1");
        for bad in [
            "https://h.test:3128",
            "h.test:3128",
            "http://h.test",
            "http://h.test:0",
            "http://h.test:3128/path",
            "http://[::1]:3128",
            "http://u%3Ax:p@h.test:1",
            "http://u:p%40q@h.test:1",
            "http://u:p%20q@h.test:1",
            "http://:p@h.test:1",
            "http://u:p%zz@h.test:1",
            "http://u:pa$s@h.test:1",
        ] {
            assert!(check(PROXY_URL, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_bypass_list_is_what_tinyproxy_matches() {
        assert_eq!(
            check(
                PROXY_BYPASS,
                "Intranet.test, .corp.test 10.0.0.0/8,192.168.1.5,fd00::/8"
            )
            .unwrap(),
            "intranet.test,.corp.test,10.0.0.0/8,192.168.1.5,fd00::/8"
        );
        for bad in ["10.0.0.0/33", "host/8", ".", "a..b", "*.corp.test"] {
            assert!(check(PROXY_BYPASS, bad).is_err(), "{bad}");
        }
        for name in [PROXY_URL, PROXY_BYPASS] {
            assert_eq!(
                find(name).unwrap().consumers,
                [Consumer::Proxy, Consumer::Agent],
                "{name}"
            );
        }
    }

    #[test]
    fn the_time_keys_are_applied_to_the_running_clock() {
        assert_eq!(check(NTP_ENABLE, "off").unwrap(), "0");
        for name in [TIMEZONE, NTP_ENABLE, NTP_SERVERS] {
            assert_eq!(find(name).unwrap().consumers, [Consumer::Time], "{name}");
        }
    }

    #[test]
    fn zoom_has_chromes_range_and_restarts_the_browser() {
        assert_eq!(check(ZOOM, "25").unwrap(), "25");
        assert_eq!(check(ZOOM, "500").unwrap(), "500");
        assert!(check(ZOOM, "24").is_err());
        assert!(check(ZOOM, "501").is_err());
        assert!(check(ZOOM, "1.5").is_err());
        assert_eq!(find(ZOOM).unwrap().consumers, [Consumer::Browser]);
    }

    #[test]
    fn the_injected_script_is_a_file_in_the_store_from_its_root() {
        assert_eq!(check(INJECT_SCRIPT, "inject.js").unwrap(), "inject.js");
        assert_eq!(check(INJECT_SCRIPT, "/inject.js").unwrap(), "inject.js");
        assert_eq!(check(INJECT_SCRIPT, "/js/site.js").unwrap(), "js/site.js");
        assert_eq!(check(INJECT_SCRIPT, "").unwrap(), "");
        assert!(check(INJECT_SCRIPT, "/").is_err(), "the store itself");
        assert!(check(INJECT_SCRIPT, "../tessaro/auth.json").is_err());
        assert!(check(INJECT_SCRIPT, "a//b.js").is_err());
        assert_eq!(find(INJECT_SCRIPT).unwrap().consumers, [Consumer::Agent]);
    }

    #[test]
    fn the_bridge_mode_is_off_config_or_actions() {
        for mode in BRIDGE_MODES {
            assert_eq!(check(BRIDGE_MODE, mode).unwrap(), *mode);
        }
        assert_eq!(check(BRIDGE_MODE, "Actions").unwrap(), "actions");
        assert!(check(BRIDGE_MODE, "on").is_err());
        assert_eq!(find(BRIDGE_MODE).unwrap().consumers, [Consumer::Agent]);
    }

    #[test]
    fn the_wifi_fallback_is_seconds_the_agent_reads_not_a_network_change() {
        assert_eq!(check(WIFI_FALLBACK_AFTER, "120").unwrap(), "120");
        assert_eq!(check(WIFI_FALLBACK_AFTER, "0").unwrap(), "0");
        assert!(check(WIFI_FALLBACK_AFTER, "-1").is_err());
        assert!(check(WIFI_FALLBACK_AFTER, "86401").is_err());
        assert_eq!(
            find(WIFI_FALLBACK_AFTER).unwrap().consumers,
            [Consumer::Agent]
        );
    }

    #[test]
    fn subnets_and_passphrases() {
        let net = Ipv4Addr::new(192, 168, 1, 50);
        assert!(contains(net, 24, Ipv4Addr::new(192, 168, 1, 1)));
        assert!(!contains(net, 24, Ipv4Addr::new(192, 168, 2, 1)));
        assert!(check_psk("correct horse").is_ok());
        assert!(check_psk("short").is_err());
        assert!(check_psk(&"a".repeat(64)).is_ok());
        assert!(check_psk("pässwort123").is_err());
    }
}
