//! Every setting a device has, in one table.
//!
//! A key is what a technician types (`kiosk.url`); its `env` name is what the
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

use std::net::{Ipv4Addr, SocketAddr};

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
    /// A free-form custom value, used in kiosk.url as `{data.<name>}`.
    Param,
    /// Text for the debug screen: `{key}` placeholders like kiosk.url, and
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
    /// Not a setting: something the device reports - its address, its id.
    /// Listed with `keys`, readable with `get`, usable in kiosk.url, and
    /// refused by `set`.
    ReadOnly,
}

impl Kind {
    /// What a value of this kind may be, for `tessaro-ctl keys`.
    pub fn describe(&self) -> String {
        match self {
            Kind::Url => {
                "an http, https, file or data URL; may contain {key} placeholders - any setting's key, e.g. {data.table} or {node.name}"
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
            Kind::Resolution => "preferred, or WIDTHxHEIGHT from `tessaro-ctl modes`".to_string(),
            Kind::Name => "letters, digits and dashes, up to 40; empty derives one".to_string(),
            Kind::Listen => "address:port, or off".to_string(),
            Kind::Param => "any text; percent-encoded where kiosk.url uses it".to_string(),
            Kind::Template => {
                "text; \\n breaks a line; {key} placeholders as in kiosk.url, plus {kiosk.url}"
                    .to_string()
            }
            Kind::Interface => "an interface name (eth0, enp1s0, wlan0), or auto".to_string(),
            Kind::Cidr => "ADDRESS/PREFIX, e.g. 192.168.1.50/24, or empty".to_string(),
            Kind::Address => "an IPv4 address, or empty".to_string(),
            Kind::Addresses => "IPv4 addresses, comma separated, or empty".to_string(),
            Kind::Ssid => "a WiFi network name, 1 to 32 bytes".to_string(),
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
    pub doc: &'static str,
}

const AGENT: &[Consumer] = &[Consumer::Agent];
const NOBODY: &[Consumer] = &[];
const BROWSER: &[Consumer] = &[Consumer::Browser];
const WESTON: &[Consumer] = &[Consumer::Weston];
const NETWORK: &[Consumer] = &[Consumer::Network];
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
    key("kiosk.url", "KIOSK_URL", Kind::Url, AGENT,
        "The page the kiosk shows (default: the self-test page, http://127.0.0.1/). A new origin also re-grants the device APIs to it."),
    key("kiosk.probe_url", "KIOSK_PROBE_URL", Kind::OptionalUrl, AGENT,
        "Health endpoint to probe instead of kiosk.url; empty probes kiosk.url. Needed for a file: or data: kiosk."),
    key("kiosk.offline_url", "KIOSK_OFFLINE_URL", Kind::OfflineUrl, AGENT,
        "Page shown while the site is down; empty stages /data/kiosk/offline.html or the shipped page; none stays on the site."),
    key("kiosk.enforce_origin", "KIOSK_ENFORCE_ORIGIN", Kind::Flag, AGENT,
        "Bring the browser back when it leaves the kiosk origin. 0 for a site that hands visitors to another host."),
    key("maintenance.enable", "KIOSK_MAINTENANCE", Kind::Flag, AGENT,
        "Maintenance mode: show maintenance.url instead of kiosk.url, which is kept as it is. `tessaro-ctl maintenance on|off`."),
    key("maintenance.url", "KIOSK_MAINTENANCE_URL", Kind::Url, AGENT,
        "The page shown in maintenance mode (default: http://127.0.0.1/maintenance.html, which takes ?title= and ?message=)."),
    key("browser.args_extra", "KIOSK_CHROMIUM_ARGS_EXTRA", Kind::Args, BROWSER,
        "Extra Chromium flags after the fixed set, e.g. --disable-pinch. Features go in browser.*_features."),
    key("browser.touch", "KIOSK_TOUCH", Kind::Choice(&["auto", "enabled", "disabled"]), BROWSER,
        "Touch event feature detection."),
    key("browser.enable_features", "KIOSK_ENABLE_FEATURES", Kind::Features, BROWSER,
        "Chromium features to enable, comma separated, e.g. WebBluetooth,WebBluetoothNewPermissionsBackend."),
    key("browser.disable_features", "KIOSK_DISABLE_FEATURES", Kind::Features, BROWSER,
        "Chromium features to disable. Replaces the default: keep FallbackToSWIfGLES3NotSupported (Pi 3 GPU)."),
    key("browser.fps_counter", "KIOSK_FPS_COUNTER", Kind::Flag, BROWSER,
        "Show Chromium's FPS counter in the corner of the screen (--show-fps-counter)."),
    key("browser.device_origins", "KIOSK_DEVICE_ORIGINS", Kind::Origins, BROWSER,
        "Origins granted WebSerial and WebHID besides the kiosk and self-test origins."),
    key("display.scale", "KIOSK_SCALE", Kind::Scale, WESTON,
        "Weston output scale: auto (2 above 3400px wide), none, or 1-4."),
    Key {
        guarded: true,
        ..key("display.resolution", "KIOSK_RESOLUTION", Kind::Resolution, WESTON,
            "Output mode, WIDTHxHEIGHT from `tessaro-ctl modes`, or preferred. Reverts unless confirmed.")
    },
    key("display.osk", "KIOSK_OSK", Kind::Choice(&["auto", "always", "never"]), WESTON,
        "On-screen keyboard: auto shows it only without a USB/Bluetooth keyboard."),
    key("display.vnc", "KIOSK_VNC", Kind::Choice(&["on", "off"]), WESTON,
        "Mirror the screen to VNC on 127.0.0.1:5900."),
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
    key("node.name", "KIOSK_NODE_NAME", Kind::Name, AGENT_AND_NETWORK,
        "The device's name on the network (NAME.local, and the hotspot tessaro-NAME); empty derives one from the node id."),
    // The device's own network: four NetworkManager profiles the agent
    // generates (tessaro-ethernet-dhcp/-static, tessaro-wifi-hotspot/-client)
    // and switches between. A change is kept only if the device still
    // reaches the network afterwards; see `tessaro-ctl set --verify`.
    key("ethernet.interface", "KIOSK_ETHERNET_INTERFACE", Kind::Interface, NETWORK,
        "The Ethernet port the device manages; auto is the first one that comes up. Other ports are left to hand-made profiles."),
    key("ethernet.mode", "KIOSK_ETHERNET_MODE", Kind::Choice(&["dhcp", "static"]), NETWORK,
        "dhcp, or static with ethernet.address, ethernet.gateway and ethernet.dns."),
    key("ethernet.address", "KIOSK_ETHERNET_ADDRESS", Kind::Cidr, NETWORK,
        "The static address with ethernet.mode=static, e.g. 192.168.1.50/24."),
    key("ethernet.gateway", "KIOSK_ETHERNET_GATEWAY", Kind::Address, NETWORK,
        "The default gateway with ethernet.mode=static; inside ethernet.address. Empty for none."),
    key("ethernet.dns", "KIOSK_ETHERNET_DNS", Kind::Addresses, NETWORK,
        "DNS servers with ethernet.mode=static, comma separated."),
    key("wifi.interface", "KIOSK_WIFI_INTERFACE", Kind::Interface, NETWORK,
        "The WiFi device the device manages; auto is wlan0. Without it, nothing WiFi ever comes up."),
    key("wifi.mode", "KIOSK_WIFI_MODE", Kind::Choice(&["hotspot", "client", "off"]), NETWORK,
        "hotspot (tessaro-NAME, for installation and management), client (joins wifi.ssid; `tessaro-ctl net wifi join`), or off."),
    key("wifi.nat", "KIOSK_WIFI_NAT", Kind::Flag, NETWORK,
        "Let hotspot clients reach the internet and the LAN through the device; 0 lets them reach the device only."),
    key("wifi.ssid", "KIOSK_WIFI_SSID", Kind::Ssid, NETWORK,
        "The network wifi.mode=client joins. Its password is set by `tessaro-ctl net wifi join` and never shown."),
    key("wifi.security", "KIOSK_WIFI_SECURITY", Kind::Choice(&["psk", "sae", "open"]), NETWORK,
        "The client network's security: psk (WPA2), sae (WPA3) or open. `net wifi join` finds it by scanning."),
    key("wifi.hidden", "KIOSK_WIFI_HIDDEN", Kind::Flag, NETWORK,
        "The client network does not broadcast its name."),
    key("wifi.ipv4", "KIOSK_WIFI_IPV4", Kind::Choice(&["dhcp", "static"]), NETWORK,
        "Client addressing: dhcp, or static with wifi.address, wifi.gateway and wifi.dns."),
    key("wifi.address", "KIOSK_WIFI_ADDRESS", Kind::Cidr, NETWORK,
        "The static client address with wifi.ipv4=static, e.g. 192.168.1.51/24."),
    key("wifi.gateway", "KIOSK_WIFI_GATEWAY", Kind::Address, NETWORK,
        "The default gateway with wifi.ipv4=static; inside wifi.address. Empty for none."),
    key("wifi.dns", "KIOSK_WIFI_DNS", Kind::Addresses, NETWORK,
        "DNS servers with wifi.ipv4=static, comma separated."),
    key("api.listen", "KIOSK_API_LISTEN", Kind::Listen, AGENT,
        "Where the TLS control API listens, address:port, or off."),
    key("api.mdns", "KIOSK_MDNS", Kind::Choice(&["on", "off"]), AGENT,
        "Advertise the device as NAME.local and _tessaro._tcp."),
    key("debug.enable", "KIOSK_DEBUG_SCREEN", Kind::Flag, AGENT,
        "Show debug.template full screen instead of the kiosk page. Not agent.debug, which is journal verbosity."),
    key("debug.template", "KIOSK_DEBUG_TEMPLATE", Kind::Template, AGENT,
        "What the debug screen shows: text with {key} placeholders, \\n for a new line, e.g. IP {net.ip}\\nGW {net.gateway}."),
    // Read-only: what the device reports right now. `tessaro-ctl net` shows
    // the same in full, per interface.
    live("node.id", "The node id: systemd's app-specific machine id, never the machine id itself."),
    live("net.hostname", "The kernel hostname."),
    live("net.interface", "The interface carrying the IPv4 default route."),
    live("net.mac", "MAC address of net.interface."),
    live("net.ip", "The first IPv4 address of net.interface."),
    live("net.netmask", "Netmask of net.ip, dotted (255.255.255.0)."),
    live("net.cidr", "net.ip with its prefix length (192.168.1.20/24)."),
    live("net.gateway", "The IPv4 default gateway."),
    live("net.dns", "DNS servers in use, comma separated."),
    live("net.ipv4", "Every IPv4 address on every interface but loopback, comma separated."),
    live("net.ipv6", "Every IPv6 address on every interface but loopback, comma separated."),
    live("net.public_ip", "The address the internet sees, from Cloudflare's trace; looked up by `net` and `get net.public_ip`, and every 5 minutes while kiosk.url uses it."),
    live("net.interfaces", "Every interface but loopback with its state and addresses, as eth0 up 10.0.0.20/24; wlan0 down."),
    live("wifi.hotspot_ssid", "The hotspot's network name, tessaro-NAME. Open while the device is unclaimed; claiming it sets a password, shown once."),
];

/// Custom values: `data.<name>`, named by whoever sets them. The kiosk gives
/// them no meaning; they exist to be put into kiosk.url as `{<name>}`.
pub const DATA_PREFIX: &str = "data.";

/// Every custom `data.<name>` setting shares this entry. It has no env
/// variable of its own: its value only exists inside the expanded kiosk.url.
pub static DATA: Key = Key {
    name: "data.<name>",
    env: "",
    kind: Kind::Param,
    consumers: AGENT,
    guarded: false,
    doc: "Custom values with names you choose, for kiosk.url: data.table=12 fills {data.table}, as in \
          https://menu.test/?table={data.table}. The kiosk gives them no meaning of its own. Set them \
          before or together with a kiosk.url that uses them. A placeholder is always a full key, so \
          built-in settings work the same way: {node.name}, {display.scale}, ...",
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

/// What a `{name}` in kiosk.url stands for.
#[derive(Debug, Clone, Copy)]
pub enum Placeholder<'a> {
    /// `{data.table}`: the custom value `data.table`; holds `table`.
    Param(&'a str),
    /// `{node.name}`: the effective value of a registry key.
    Key(&'static Key),
    /// Neither - including `{kiosk.url}`, `{maintenance.url}` and
    /// `{debug.template}`: a template cannot contain itself or another one.
    /// The debug template resolves `{kiosk.url}` on its own.
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

/// The keys whose values are URL templates, expanded before anyone sees them.
pub const TEMPLATES: [&str; 2] = ["kiosk.url", "maintenance.url"];

/// A placeholder is always a setting's full key: `{data.table}` for the
/// custom `data.table`, `{node.name}` for `node.name`. One rule, no short
/// forms, so a template reads exactly like the `set` that fills it.
pub fn placeholder(name: &str) -> Placeholder<'_> {
    if let Some(custom) = param_name(name) {
        return Placeholder::Param(custom);
    }
    match KEYS.iter().find(|key| key.name == name) {
        Some(key) if !TEMPLATES.contains(&key.name) && key.kind != Kind::Template => {
            Placeholder::Key(key)
        }
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
    }
}

/// A name the kernel could give an interface: `IFNAMSIZ` less the NUL.
pub fn is_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
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
        ("ethernet.mode", "static", "ethernet"),
        ("wifi.ipv4", "static", "wifi"),
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
    if value("wifi.mode") == "client" {
        if value("wifi.ssid").is_empty() {
            return Err(
                "wifi.mode=client needs wifi.ssid; `tessaro-ctl net wifi join SSID` sets both"
                    .to_string(),
            );
        }
        if value("wifi.security") != "open" && !has_psk {
            return Err(format!(
                "{} needs a password; join it with `tessaro-ctl net wifi join {}`",
                value("wifi.ssid"),
                value("wifi.ssid")
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
            check("kiosk.url", " https://example.com/x ").unwrap(),
            "https://example.com/x"
        );
        assert!(check("kiosk.url", "").is_err());
        assert!(check("kiosk.url", "ftp://example.com/").is_err());
        assert!(check("kiosk.url", "https:///nohost").is_err());
        assert!(check("kiosk.url", "https://a.test/a b").is_err());
        assert_eq!(check("kiosk.probe_url", "").unwrap(), "");
        assert!(check("kiosk.probe_url", "none").is_err());
        assert_eq!(check("kiosk.offline_url", "none").unwrap(), "none");
        assert!(check("kiosk.url", "data:text/html,<h1>hi</h1>").is_ok());
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
        assert_eq!(check("display.scale", "auto").unwrap(), "");
        assert_eq!(check("display.scale", "2").unwrap(), "2");
        assert!(check("display.scale", "1.5").is_err());
        assert_eq!(
            check("display.resolution", "1920X1080").unwrap(),
            "1920x1080"
        );
        assert_eq!(check("display.resolution", "").unwrap(), "preferred");
        assert!(check("display.resolution", "1920x1080@60").is_err());
        assert!(check("display.resolution", "10x10").is_err());
        assert!(find("display.resolution").unwrap().guarded);
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
        assert_eq!(check("node.name", "Lobby-1").unwrap(), "lobby-1");
        assert!(check("node.name", "-x").is_err());
        assert!(check("node.name", "a.b").is_err());
        assert_eq!(check("node.name", "").unwrap(), "");
        assert_eq!(check("api.listen", "0.0.0.0:7400").unwrap(), "0.0.0.0:7400");
        assert_eq!(check("api.listen", "OFF").unwrap(), "off");
        assert!(check("api.listen", "7400").is_err());
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
                "https://{data.shop}.test/{data.lang}/x?s={data.shop}&j={not-one}&k={}&n={node.name}&z={.x}"
            ),
            ["data.shop", "data.lang", "node.name"]
        );
    }

    #[test]
    fn a_placeholder_is_always_a_full_key_but_never_the_url_itself() {
        assert_eq!(placeholder("data.store"), Placeholder::Param("store"));
        // No short form: {store} is not data.store.
        assert_eq!(placeholder("store"), Placeholder::Unknown);
        assert_eq!(
            placeholder("node.name"),
            Placeholder::Key(find("node.name").unwrap())
        );
        assert_eq!(
            placeholder("display.scale"),
            Placeholder::Key(find("display.scale").unwrap())
        );
        assert_eq!(placeholder("kiosk.url"), Placeholder::Unknown);
        assert_eq!(placeholder("maintenance.url"), Placeholder::Unknown);
        assert_eq!(
            placeholder("maintenance.enable"),
            Placeholder::Key(find("maintenance.enable").unwrap())
        );
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
            check("kiosk.url", "https://{data.shop}.test/?lang={data.lang}").unwrap(),
            "https://{data.shop}.test/?lang={data.lang}"
        );
        assert!(check("kiosk.url", "{data.scheme}://x.test/").is_err());
    }

    #[test]
    fn read_only_keys_are_placeholders_but_not_settings() {
        for name in ["node.id", "net.ip", "net.gateway", "net.ipv4"] {
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
            check("debug.template", "IP {net.ip}\\nGW {net.gateway}").unwrap(),
            "IP {net.ip}\\nGW {net.gateway}"
        );
        for bad in ["a\\tb", "a\\\\b", "a\\", "it's", "a\"b", "${HOME}", "a\nb"] {
            assert!(check("debug.template", bad).is_err(), "{bad}");
        }
        // Everywhere else a backslash is still refused.
        assert!(check("data.x", "a\\nb").is_err());
    }

    #[test]
    fn templates_cannot_contain_themselves() {
        assert_eq!(placeholder("debug.template"), Placeholder::Unknown);
        assert_eq!(placeholder("kiosk.url"), Placeholder::Unknown);
        assert_eq!(
            placeholder("debug.enable"),
            Placeholder::Key(find("debug.enable").unwrap())
        );
    }

    #[test]
    fn expansion_can_leave_values_raw() {
        let (text, missing) = expand_with(
            "ip {net.ip} q {data.q}",
            |name| (name == "net.ip").then(|| "10.0.0.2/24 x&y".to_string()),
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
        assert_eq!(check("ethernet.interface", "").unwrap(), "auto");
        assert_eq!(check("ethernet.interface", "enp1s0").unwrap(), "enp1s0");
        assert!(check("ethernet.interface", "eth 0").is_err());
        assert_eq!(
            check("ethernet.address", "192.168.1.50/24").unwrap(),
            "192.168.1.50/24"
        );
        assert!(check("ethernet.address", "192.168.1.50").is_err());
        assert!(check("ethernet.address", "192.168.1.50/33").is_err());
        assert_eq!(check("ethernet.gateway", "").unwrap(), "");
        assert!(check("ethernet.gateway", "2001:db8::1").is_err());
        assert_eq!(
            check("ethernet.dns", "1.1.1.1, 9.9.9.9").unwrap(),
            "1.1.1.1,9.9.9.9"
        );
        assert_eq!(check("wifi.mode", "Hotspot").unwrap(), "hotspot");
        assert!(check("wifi.ssid", "").is_err());
        assert!(check("wifi.ssid", &"x".repeat(33)).is_err());
        assert_eq!(check("wifi.ssid", "Office 2").unwrap(), "Office 2");
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
        assert!(check_network(with(&[("ethernet.mode", "dhcp")]), false).is_ok());
        let alone = check_network(with(&[("ethernet.mode", "static")]), false);
        assert!(alone.unwrap_err().contains("needs ethernet.address"));
        let outside = check_network(
            with(&[
                ("ethernet.mode", "static"),
                ("ethernet.address", "10.99.0.5/24"),
                ("ethernet.gateway", "192.168.1.1"),
            ]),
            false,
        );
        assert!(outside.unwrap_err().contains("outside"));
        assert!(check_network(
            with(&[
                ("ethernet.mode", "static"),
                ("ethernet.address", "192.168.1.50/24"),
                ("ethernet.gateway", "192.168.1.1"),
            ]),
            false
        )
        .is_ok());

        let client = [
            ("wifi.mode", "client"),
            ("wifi.ssid", "Office"),
            ("wifi.security", "psk"),
        ];
        assert!(check_network(with(&client), false)
            .unwrap_err()
            .contains("net wifi join Office"));
        assert!(check_network(with(&client), true).is_ok());
        let open = [
            ("wifi.mode", "client"),
            ("wifi.ssid", "Cafe"),
            ("wifi.security", "open"),
        ];
        assert!(check_network(with(&open), false).is_ok());
        assert!(check_network(with(&[("wifi.mode", "client")]), true).is_err());
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
