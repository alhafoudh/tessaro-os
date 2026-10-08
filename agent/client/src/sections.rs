//! The named sections the clients list their subjects under: the groups in
//! `tessaro-ctl --help`, the pages in the GUI's nav and in Webconfig's menu
//! (`webconfig/src/pages/registry.tsx` writes the same titles). The first
//! subjects - the device itself, Quick Setup - come before any section.

pub const KIOSK: &str = "Kiosk";
pub const PLAYER: &str = "Player";
pub const PERIPHERALS: &str = "Peripherals";
pub const NETWORK: &str = "Network";
pub const AUTOMATION: &str = "Automation";
pub const SECURITY: &str = "Security";
pub const SYSTEM: &str = "System";
/// The setting groups no page shows, and `config` in the ctl.
pub const SETTINGS: &str = "Settings";
/// What acts on this client and needs no device. The ctl only.
pub const CLIENT: &str = "Client";

/// The ctl's command groups by section, in the order its help lists them.
/// The first is under clap's own heading, as a plain command list would be.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Commands", &["device"]),
    (KIOSK, &["screen", "browser", "files"]),
    (PLAYER, &["playlist"]),
    (PERIPHERALS, &["audio", "camera", "printer", "scanner"]),
    (NETWORK, &["network"]),
    (AUTOMATION, &["script", "schedule"]),
    (SECURITY, &["access", "ssh"]),
    (SYSTEM, &["time", "storage", "update"]),
    (SETTINGS, &["config"]),
    (CLIENT, &["nodes", "completion", "help"]),
];
