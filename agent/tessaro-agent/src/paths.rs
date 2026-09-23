//! Where things live on a device.
//!
//! None of these are settings - `tessaro-ctl` cannot change them and they are
//! not in the key registry. They are environment variables only so the same
//! binary runs on a development host, where `/data`, `/etc/shadow` and
//! `/run` are not ours to write (`mise run agent-integration` points them all
//! into a temporary directory).

use std::path::PathBuf;

use crate::config::Env;

#[derive(Debug, Clone)]
pub struct Paths {
    /// `state.json`, `auth.json`, the TLS identity, the factory-reset marker.
    pub state_dir: PathBuf,
    /// Where `generated.env` is rendered. Shared with the offline page.
    pub run_dir: PathBuf,
    /// The Chromium policy file the agent renders. Its path is compiled into
    /// Chromium (policy_paths.cc) and cannot move.
    pub policy: PathBuf,
    /// The image's copy of the policy, which the rendered one starts from.
    pub policy_base: PathBuf,
    pub socket: PathBuf,
    pub shadow: PathBuf,
    pub machine_id: PathBuf,
    /// The runtime override file this replaced; imported once, then renamed.
    pub legacy_override: PathBuf,
    pub drm: PathBuf,
    pub cmdline: PathBuf,
    /// `/sys/class/net`: every interface, its MAC, state, MTU and kind.
    pub sys_net: PathBuf,
    /// `/proc/net/route`: the IPv4 routing table, for the default route.
    pub proc_route: PathBuf,
    /// The upstream DNS servers systemd-resolved uses. `/etc/resolv.conf` is
    /// its 127.0.0.53 stub, which says nothing about the network.
    pub resolv: PathBuf,
    pub hostname: PathBuf,
    /// The MACHINE the image was built for, for `id` and the mDNS TXT record.
    pub machine: String,
    /// The self-test page's origin, always granted the device APIs.
    pub selftest_origin: String,
    pub kiosk_unit: String,
    pub weston_unit: String,
    pub agent_unit: String,
}

impl Paths {
    pub fn load(env: &dyn Env) -> Self {
        let path = |name: &str, default: &str| {
            PathBuf::from(env.get(name).unwrap_or_else(|| default.to_string()))
        };
        let text = |name: &str, default: &str| env.get(name).unwrap_or_else(|| default.to_string());

        Self {
            state_dir: path("KIOSK_STATE_DIR", "/data/tessaro"),
            run_dir: path("KIOSK_RUN_DIR", "/run/tessaro-kiosk"),
            policy: path(
                "KIOSK_POLICY",
                "/etc/chromium/policies/managed/10-tessaro.json",
            ),
            policy_base: path("KIOSK_POLICY_BASE", "/usr/lib/tessaro-kiosk/policy.json"),
            socket: path("KIOSK_SOCKET", protocol::DEFAULT_SOCKET),
            shadow: path("KIOSK_SHADOW", "/etc/shadow"),
            machine_id: path("KIOSK_MACHINE_ID", "/etc/machine-id"),
            legacy_override: path("KIOSK_LEGACY_OVERRIDE", "/etc/default/tessaro-kiosk"),
            drm: path("KIOSK_DRM", "/sys/class/drm"),
            cmdline: path("KIOSK_CMDLINE", "/proc/cmdline"),
            sys_net: path("KIOSK_SYS_NET", "/sys/class/net"),
            proc_route: path("KIOSK_PROC_ROUTE", "/proc/net/route"),
            resolv: path("KIOSK_RESOLV", "/run/systemd/resolve/resolv.conf"),
            hostname: path("KIOSK_HOSTNAME", "/proc/sys/kernel/hostname"),
            machine: text("KIOSK_MACHINE", "unknown"),
            selftest_origin: text("KIOSK_SELFTEST_ORIGIN", "http://127.0.0.1"),
            kiosk_unit: text("KIOSK_UNIT", "tessaro-kiosk.service"),
            weston_unit: text("KIOSK_WESTON_UNIT", "weston.service"),
            agent_unit: text("KIOSK_AGENT_UNIT", "tessaro-agent.service"),
        }
    }

    pub fn generated_env(&self) -> PathBuf {
        self.run_dir.join("generated.env")
    }

    pub fn tls_dir(&self) -> PathBuf {
        self.state_dir.join("tls")
    }

    pub fn factory_reset_marker(&self) -> PathBuf {
        self.state_dir.join("factory-reset")
    }
}
