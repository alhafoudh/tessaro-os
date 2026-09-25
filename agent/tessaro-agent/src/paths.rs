//! Where things live on a device.
//!
//! None of these are settings - `tessaro-ctl` cannot change them and they are
//! not in the key registry. They are environment variables only so the same
//! binary runs on a development host, where `/data`, `/etc/shadow` and
//! `/run` are not ours to write (`mise run agent:integration` points them all
//! into a temporary directory).

use std::path::PathBuf;

use crate::config::Env;

#[derive(Debug, Clone)]
pub struct Paths {
    /// `state.json`, `auth.json`, the TLS identity, the factory-reset marker.
    pub state_dir: PathBuf,
    /// The file store `tessaro-ctl files` fills, served by nginx at
    /// `http://127.0.0.1/files/`. On the same filesystem as `state_dir`,
    /// where uploads are staged, so putting one in place is a rename.
    pub files_dir: PathBuf,
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
    /// `/sys/class/input`: what the on-screen keyboard's `auto` looks at.
    pub input: PathBuf,
    /// The config Weston is running with, as `tessaro-weston-config` wrote it
    /// at the compositor's start.
    pub weston_config: PathBuf,
    /// The generator itself, which the agent runs again after a hotplug to
    /// see whether that config still holds.
    pub weston_generator: PathBuf,
    /// The socket of Weston's `tessaro-power.so`, which switches the outputs
    /// off and on.
    pub power_socket: PathBuf,
    pub cmdline: PathBuf,
    /// `/sys/class/net`: every interface, its MAC, state, MTU and kind.
    pub sys_net: PathBuf,
    /// `/proc/net/route`: the IPv4 routing table, for the default route.
    pub proc_route: PathBuf,
    /// The upstream DNS servers systemd-resolved uses. `/etc/resolv.conf` is
    /// its 127.0.0.53 stub, which says nothing about the network.
    pub resolv: PathBuf,
    pub hostname: PathBuf,
    /// Where the managed NetworkManager profiles are rendered: the
    /// in-memory keyfile directory, highest precedence, gone at every boot.
    pub nm_run_dir: PathBuf,
    /// Root's `authorized_keys`, in root's real home (`/etc/passwd` says
    /// `/root`, which is where dropbear looks). `/root` is a bind of
    /// `/data/overlay-root`, so it persists; the claim model owns it like
    /// the root password.
    pub authorized_keys: PathBuf,
    /// Where dropbear keeps its host keys, first match wins. `/etc/dropbear`
    /// is on the `/etc` overlay, so a key made at first boot stays.
    /// `/var/lib/dropbear` is where oe-core's read-only-rootfs hook would put
    /// them (tmpfs, a new key every boot) if the image ever lost
    /// `overlayfs-etc`; the hook skips images that have it.
    pub ssh_host_key_dirs: Vec<PathBuf>,
    /// `/sys/class/block` and `/dev/disk/by-partuuid`: which partition is
    /// root, for checking that an update was built for this disk.
    pub sys_block: PathBuf,
    pub by_partuuid: PathBuf,
    /// `/dev/disk/by-label`, `/proc/self/mountinfo` and `/dev`: the
    /// partitions' labels, what is mounted where, and the device nodes
    /// `storage grow` works on.
    pub by_label: PathBuf,
    pub mountinfo: PathBuf,
    pub dev: PathBuf,
    /// `/proc/meminfo`: whether a disk update's upload fits in RAM.
    pub meminfo: PathBuf,
    /// The image's os-release, for `status`.
    pub os_release: PathBuf,
    /// Where PipeWire, WirePlumber and the Pulse server put their sockets:
    /// the runtime directory their units share with nothing else.
    pub audio_runtime: PathBuf,
    /// `/proc/asound/cards`: the sound cards the kernel has, which is how the
    /// agent notices a USB speaker without asking PipeWire every few seconds.
    pub asound_cards: PathBuf,
    /// The kernel's file name on the boot partition: `bzImage` on x86,
    /// `Image` on the Pi. An update replaces exactly that file.
    pub kernel_file: String,
    /// The MACHINE the image was built for, for `id` and the mDNS TXT record.
    pub machine: String,
    /// The Raspberry Pi firmware's boot partition, with its `config.txt`,
    /// where the agent writes `tessaro.txt`. `None` on a device without that
    /// firmware. Set by the build per machine, never probed.
    pub boot_config_dir: Option<PathBuf>,
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
            files_dir: path("KIOSK_FILES_DIR", "/data/files"),
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
            input: path("KIOSK_INPUT", "/sys/class/input"),
            weston_config: path("KIOSK_WESTON_CONFIG", "/run/weston/weston.ini"),
            weston_generator: path(
                "KIOSK_WESTON_GENERATOR",
                "/usr/libexec/tessaro-weston-config",
            ),
            power_socket: path("KIOSK_POWER_SOCKET", "/run/weston/power.sock"),
            cmdline: path("KIOSK_CMDLINE", "/proc/cmdline"),
            sys_net: path("KIOSK_SYS_NET", "/sys/class/net"),
            proc_route: path("KIOSK_PROC_ROUTE", "/proc/net/route"),
            resolv: path("KIOSK_RESOLV", "/run/systemd/resolve/resolv.conf"),
            hostname: path("KIOSK_HOSTNAME", "/proc/sys/kernel/hostname"),
            nm_run_dir: path("KIOSK_NM_RUN_DIR", "/run/NetworkManager/system-connections"),
            authorized_keys: path("KIOSK_AUTHORIZED_KEYS", "/root/.ssh/authorized_keys"),
            ssh_host_key_dirs: text("KIOSK_SSH_HOST_KEY_DIRS", "/etc/dropbear:/var/lib/dropbear")
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .collect(),
            sys_block: path("KIOSK_SYS_BLOCK", "/sys/class/block"),
            by_partuuid: path("KIOSK_BY_PARTUUID", "/dev/disk/by-partuuid"),
            by_label: path("KIOSK_BY_LABEL", "/dev/disk/by-label"),
            mountinfo: path("KIOSK_MOUNTINFO", "/proc/self/mountinfo"),
            dev: path("KIOSK_DEV", "/dev"),
            meminfo: path("KIOSK_MEMINFO", "/proc/meminfo"),
            os_release: path("KIOSK_OS_RELEASE", "/usr/lib/os-release"),
            audio_runtime: path("KIOSK_AUDIO_RUNTIME_DIR", "/run/tessaro-audio"),
            asound_cards: path("KIOSK_ASOUND_CARDS", "/proc/asound/cards"),
            kernel_file: text("KIOSK_KERNEL_FILE", "bzImage"),
            machine: text("KIOSK_MACHINE", "unknown"),
            boot_config_dir: env
                .get("KIOSK_BOOT_CONFIG_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            selftest_origin: text("KIOSK_SELFTEST_ORIGIN", "http://127.0.0.1"),
            kiosk_unit: text("KIOSK_UNIT", "tessaro-kiosk.service"),
            weston_unit: text("KIOSK_WESTON_UNIT", "weston.service"),
            agent_unit: text("KIOSK_AGENT_UNIT", "tessaro-agent.service"),
        }
    }

    /// Whether this device has `hardware`.
    pub fn has(&self, hardware: protocol::keys::Hardware) -> bool {
        match hardware {
            protocol::keys::Hardware::PiFirmware => self.boot_config_dir.is_some(),
        }
    }

    /// Whether `key` exists on this device: every key does, but one that
    /// needs hardware the device lacks.
    pub fn offers(&self, key: &protocol::keys::Key) -> bool {
        key.only.is_none_or(|hardware| self.has(hardware))
    }

    pub fn generated_env(&self) -> PathBuf {
        self.run_dir.join("generated.env")
    }

    /// The last public address found, written by the agent's refresher.
    pub fn public_ip_file(&self) -> PathBuf {
        self.run_dir.join("public-ip")
    }

    /// What the welcome page at http://127.0.0.1/ shows, kept current by the
    /// agent and served by nginx as /welcome.json.
    pub fn welcome_file(&self) -> PathBuf {
        self.run_dir.join("welcome.json")
    }

    /// Whether the screen should be off, kept to put it back after Weston
    /// restarts. Runtime only: a reboot turns the screen on.
    pub fn screen_power_file(&self) -> PathBuf {
        self.run_dir.join("screen-off")
    }

    pub fn tls_dir(&self) -> PathBuf {
        self.state_dir.join("tls")
    }

    pub fn factory_reset_marker(&self) -> PathBuf {
        self.state_dir.join("factory-reset")
    }

    /// Where the agent has the Weston config generator write its answer after
    /// a hotplug, to compare with the running config.
    pub fn weston_candidate(&self) -> PathBuf {
        self.run_dir.join("weston-candidate.ini")
    }

    /// The hardware snapshot Weston was last restarted for by a hotplug, so
    /// a config that still differs afterwards is not restarted again.
    pub fn display_reconciled(&self) -> PathBuf {
        self.run_dir.join("display-reconciled")
    }

    /// The WiFi client gave way to the hotspot, until the next boot; holds
    /// the SSID it stands in for. In `/run`, so every boot tries the client
    /// again, and an agent restart keeps it.
    pub fn wifi_fallback_marker(&self) -> PathBuf {
        self.run_dir.join("wifi-fallback")
    }

    /// The WiFi client was up once this boot, which disarms the fallback
    /// until the next one.
    pub fn wifi_client_seen_marker(&self) -> PathBuf {
        self.run_dir.join("wifi-client-seen")
    }

    /// Image updates: the upload, the staging, the marker, the last result.
    pub fn update_dir(&self) -> PathBuf {
        self.state_dir.join("update")
    }

    /// The file being uploaded into the store, until it is complete.
    pub fn files_upload_dir(&self) -> PathBuf {
        self.state_dir.join("files-upload")
    }

    /// Where a factory reset moves the store to delete it.
    pub fn files_trash_dir(&self) -> PathBuf {
        self.state_dir.join("files-trash")
    }

    /// Network changes: the one in progress, and what the last one did. No
    /// secrets; those are in `secrets.json`.
    pub fn network_dir(&self) -> PathBuf {
        self.state_dir.join("network")
    }
}
