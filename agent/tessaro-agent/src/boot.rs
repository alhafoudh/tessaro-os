//! `tessaro-agent boot`: the oneshot that runs before Weston, the browser
//! and the agent, as `tessaro-config.service`.
//!
//! It is what lets the browser unit keep `WantedBy=` and still never start on
//! stale configuration: by the time anything reads `generated.env` or the
//! policy, this has rendered them. It is also the only thing that runs when
//! the agent itself will not, which is why the escape hatch lives here.
//!
//! In order:
//!
//! 0. **The last update**: what the initramfs did with a pending image, put
//!    in the journal once (`updates::report`).
//! 1. **Factory reset**, if `/data/tessaro/factory-reset` exists or the
//!    kernel command line says `tessaro.factory_reset`: settings, tokens, ssh
//!    keys, root password and stored files cleared - the fresh-install state.
//!    What a reset left of the file store to delete is finished every boot.
//!    The marker is removed
//!    afterwards; the command-line flag is meant to be typed at the boot
//!    loader for one boot, not written into its config.
//! 2. **Migration** of a leftover `/etc/default/tessaro-kiosk`, once, and of
//!    settings saved under a key's old name (`keys::RENAMED`).
//! 3. **Probation**: a guarded change still pending at boot was never
//!    confirmed - the device was rebooted instead - so it reverts.
//! 4. **The claim invariant**: unclaimed means an empty root password, no
//!    ssh keys and an open hotspot. This is also what heals a power cut in
//!    the middle of a claim or an unclaim.
//! 5. **The TLS identity**, made if missing, so its fingerprint is in the
//!    journal from the first boot.
//! 6. **Render.**
//! 7. **The network**: the managed NetworkManager profiles, rendered
//!    from the saved settings into `/run/NetworkManager/system-connections`
//!    before NetworkManager starts (the unit is ordered before it), and the
//!    hotspot's NAT table. Whatever a change that never committed left
//!    there is gone with `/run`.
//!
//! Every step logs and carries on. A failure here must never keep the kiosk
//! from booting: the worst outcome is the image's defaults.

use std::fs;

use crate::auth::{self, Auth};
use crate::config::Env;
use crate::files;
use crate::identity;
use crate::log::Log;
use crate::nm::{nat, profiles};
use crate::paths::Paths;
use crate::render;
use crate::secrets::{self, Secrets};
use crate::shadow;
use crate::ssh;
use crate::state::{self, State};
use crate::store::Store;
use crate::updates;

pub fn run(env: &dyn Env, log: &Log) {
    let paths = Paths::load(env);
    let defaults = state::defaults(env);
    let state_store = Store::new(&paths.state_dir, state::FILE);
    let auth_store = Store::new(&paths.state_dir, auth::FILE);
    let secrets_store = Store::new(&paths.state_dir, secrets::FILE);

    updates::report(&paths, log);

    if factory_reset_requested(&paths) {
        factory_reset(&paths, &state_store, &auth_store, &secrets_store, log);
    }
    if let Err(err) = files::clean(&paths) {
        log.info(format!(
            "could not finish removing the old file store: {err}"
        ));
    }

    migrate(&paths, &state_store, log);
    rename_keys(&state_store, log);

    match state_store.update(log, |state: &mut State| Ok(state.revert_pending())) {
        Ok(Some(pending)) => log.info(format!(
            "{}={} was never confirmed; back to {}",
            pending.key,
            pending.value,
            pending.previous_or_default()
        )),
        Ok(None) => {}
        Err(err) => log.info(format!("could not check for an unconfirmed change: {err}")),
    }

    reconcile(&paths, &auth_store, &secrets_store, log);

    match identity::tls(&paths.tls_dir()) {
        Ok((tls, made)) => log.info(format!(
            "TLS identity {}: fingerprint {}",
            if made { "created" } else { "present" },
            tls.fingerprint
        )),
        Err(err) => log.info(format!("no TLS identity: {err}")),
    }

    let state: State = state_store.read(log);
    match render::all(&paths, &defaults, &state.settings, log) {
        Ok(rendered) => {
            log.info(format!(
                "rendered settings revision {} (env {}, policy {})",
                state.revision,
                if rendered.env_changed {
                    "updated"
                } else {
                    "unchanged"
                },
                if rendered.policy_changed {
                    "updated"
                } else {
                    "unchanged"
                },
            ));
            // The firmware has already read its files for this boot: a
            // factory reset, or the include just added, reaches it next time.
            if rendered.firmware_changed {
                log.info("the firmware settings changed; they apply at the next reboot");
            }
        }
        Err(err) => log.info(format!("render failed, the image defaults apply: {err}")),
    }

    network(&paths, &defaults, &state, &secrets_store.read(log), log);
}

/// The managed profiles and the NAT table, from the saved settings.
fn network(
    paths: &Paths,
    defaults: &std::collections::HashMap<String, String>,
    state: &State,
    secrets: &Secrets,
    log: &Log,
) {
    let value = profiles::value_of(&state.settings, defaults);
    let derived = identity::read_node_id(&paths.machine_id)
        .map(|id| identity::friendly_name(&id))
        .unwrap_or_else(|_| "kiosk".to_string());
    let config = profiles::NetConfig::from_settings(
        &value,
        secrets.hotspot_psk.clone(),
        secrets.wifi_psk.clone(),
        &profiles::node_name(&value, &derived),
    );
    match profiles::write(&paths.nm_run_dir, &profiles::render(&config)) {
        Ok(_) => log.info(format!(
            "network profiles: ethernet {}, wifi {} ({}, {})",
            config.ethernet_profile().id,
            config
                .wifi_profile()
                .map(|profile| profile.id)
                .unwrap_or("off"),
            config.wifi.hotspot_ssid,
            if config.wifi.hotspot_psk.is_some() {
                "WPA2"
            } else {
                "open"
            }
        )),
        Err(err) => log.info(format!(
            "network profiles: {}: {err}",
            paths.nm_run_dir.display()
        )),
    }
    if let Err(err) = nat::apply_blocking(config.wifi.nat, config.wifi.nat_match()) {
        log.info(format!("hotspot NAT: {err}"));
    }
}

fn factory_reset_requested(paths: &Paths) -> bool {
    paths.factory_reset_marker().exists()
        || fs::read_to_string(&paths.cmdline).is_ok_and(|cmdline| {
            cmdline
                .split_whitespace()
                .any(|arg| arg == "tessaro.factory_reset")
        })
}

fn factory_reset(
    paths: &Paths,
    state_store: &Store,
    auth_store: &Store,
    secrets_store: &Store,
    log: &Log,
) {
    // Tokens before the password, as everywhere else.
    if let Err(err) = auth_store.remove() {
        log.info(format!("factory reset: auth.json: {err}"));
    }
    if let Err(err) = ssh::clear(&paths.authorized_keys) {
        log.info(format!("factory reset: authorized_keys: {err}"));
    }
    if let Err(err) = shadow::set_root(&paths.shadow, None) {
        log.info(format!("factory reset: root password: {err}"));
    }
    if let Err(err) = state_store.remove() {
        log.info(format!("factory reset: state.json: {err}"));
    }
    if let Err(err) = secrets_store.remove() {
        log.info(format!("factory reset: secrets.json: {err}"));
    }
    if let Err(err) = files::wipe(paths) {
        log.info(format!("factory reset: the file store: {err}"));
    }
    match fs::remove_file(paths.factory_reset_marker()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log.info(format!("factory reset: cannot remove the marker: {err}")),
    }
    log.info(
        "factory reset: settings, tokens, ssh keys, root and network passwords, stored files cleared",
    );
}

/// Import the old runtime override file into `state.json`, once. Only the
/// assignments that were not commented out, only keys the registry knows,
/// and only values that validate; everything skipped is named.
fn migrate(paths: &Paths, state_store: &Store, log: &Log) {
    let Ok(text) = fs::read_to_string(&paths.legacy_override) else {
        return;
    };

    let (imported, skipped) = parse_legacy(&text);
    for line in &skipped {
        log.info(format!(
            "{}: not imported: {line}",
            paths.legacy_override.display()
        ));
    }

    if !imported.is_empty() {
        let names: Vec<String> = imported.iter().map(|(key, _)| key.clone()).collect();
        let outcome = state_store.update(log, |state: &mut State| {
            for (key, value) in &imported {
                state.settings.insert(key.clone(), value.clone());
            }
            state.revision += 1;
            Ok(())
        });
        match outcome {
            Ok(()) => log.info(format!(
                "imported {} from {}",
                names.join(", "),
                paths.legacy_override.display()
            )),
            Err(err) => {
                log.info(format!(
                    "could not import {}: {err}",
                    paths.legacy_override.display()
                ));
                return;
            }
        }
    }

    let migrated = paths.legacy_override.with_extension("migrated");
    if let Err(err) = fs::rename(&paths.legacy_override, &migrated) {
        log.info(format!(
            "cannot rename {}: {err}",
            paths.legacy_override.display()
        ));
    }
}

/// Settings saved under a key's old name, moved to the new one. Read first,
/// so a device with nothing to rename never rewrites its state.json.
fn rename_keys(state_store: &Store, log: &Log) {
    let mut state: State = state_store.read(log);
    if state.rename_keys().is_empty() {
        return;
    }
    match state_store.update(log, |state: &mut State| Ok(state.rename_keys())) {
        Ok(done) => {
            for line in done {
                log.info(format!("renamed setting: {line}"));
            }
        }
        Err(err) => log.info(format!("could not rename old settings: {err}")),
    }
}

fn parse_legacy(text: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut imported = Vec::new();
    let mut skipped = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            skipped.push(line.to_string());
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);

        match protocol::keys::find_env(name.trim()) {
            Some(key) => match protocol::keys::validate(key, value) {
                Ok(value) => imported.push((key.name.to_string(), value)),
                Err(err) => skipped.push(err),
            },
            None => skipped.push(format!("{} is not a setting any more", name.trim())),
        }
    }
    (imported, skipped)
}

fn reconcile(paths: &Paths, auth_store: &Store, secrets_store: &Store, log: &Log) {
    let auth: Auth = auth_store.read(log);
    if auth.claimed() {
        return;
    }
    let secrets: Secrets = secrets_store.read(log);
    if secrets.hotspot_psk.is_some() {
        match secrets_store.update(log, |secrets: &mut Secrets| {
            secrets.hotspot_psk = None;
            Ok(())
        }) {
            Ok(()) => log.info("unclaimed but the hotspot had a password: opened it"),
            Err(err) => log.info(format!("could not open the hotspot: {err}")),
        }
    }
    match ssh::clear(&paths.authorized_keys) {
        Ok(true) => log.info("unclaimed but root had ssh keys: removed them"),
        Ok(false) => {}
        Err(err) => log.info(format!(
            "could not empty {}: {err}",
            paths.authorized_keys.display()
        )),
    }
    match shadow::root_has_password(&paths.shadow) {
        Ok(true) => match shadow::set_root(&paths.shadow, None) {
            Ok(()) => log.info("unclaimed but root had a password: emptied it"),
            Err(err) => log.info(format!("could not empty the root password: {err}")),
        },
        Ok(false) => {}
        Err(err) => log.info(format!("cannot read {}: {err}", paths.shadow.display())),
    }
}

/// For tests: does `path` hold an unclaimed-looking root entry?
#[cfg(test)]
fn root_is_empty(path: &std::path::Path) -> bool {
    !shadow::root_has_password(path).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Device {
        _dir: tempfile::TempDir,
        env: HashMap<String, String>,
    }

    impl Device {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let at = |name: &str| dir.path().join(name).display().to_string();
            let env: HashMap<String, String> = [
                ("KIOSK_STATE_DIR", at("data")),
                ("KIOSK_FILES_DIR", at("files")),
                ("KIOSK_RUN_DIR", at("run")),
                ("KIOSK_POLICY", at("policy.json")),
                ("KIOSK_POLICY_BASE", at("policy-base.json")),
                ("KIOSK_SHADOW", at("etc/shadow")),
                ("KIOSK_AUTHORIZED_KEYS", at("root/.ssh/authorized_keys")),
                ("KIOSK_LEGACY_OVERRIDE", at("etc/default-tessaro-kiosk")),
                ("KIOSK_CMDLINE", at("cmdline")),
                ("KIOSK_URL", "http://127.0.0.1/".to_string()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

            fs::create_dir_all(dir.path().join("etc")).unwrap();
            fs::write(dir.path().join("etc/shadow"), "root::1:0:99999:7:::\n").unwrap();
            fs::write(
                dir.path().join("policy-base.json"),
                "{\"TranslateEnabled\": false}",
            )
            .unwrap();
            fs::write(dir.path().join("cmdline"), "console=ttyS0 quiet\n").unwrap();
            Self { _dir: dir, env }
        }

        fn paths(&self) -> Paths {
            Paths::load(&self.env)
        }

        fn state(&self) -> State {
            Store::new(self.paths().state_dir, state::FILE).read(&Log::buffered(true))
        }
    }

    #[test]
    fn a_fresh_device_renders_and_gets_an_identity() {
        let device = Device::new();
        let log = Log::buffered(true);

        run(&device.env, &log);

        let paths = device.paths();
        assert!(paths.generated_env().exists());
        assert!(paths.policy.exists());
        assert!(paths.tls_dir().join("cert.pem").exists());
        assert!(log
            .lines()
            .iter()
            .any(|line| line.contains("TLS identity created")));
    }

    #[test]
    fn unclaimed_means_an_empty_root_password() {
        let device = Device::new();
        let shadow = device.paths().shadow;
        shadow::set_root(&shadow, Some(&shadow::hash("left over").unwrap())).unwrap();

        run(&device.env, &Log::buffered(true));

        assert!(root_is_empty(&shadow));
    }

    #[test]
    fn unclaimed_means_no_ssh_keys() {
        let device = Device::new();
        let keys = device.paths().authorized_keys;
        let key = protocol::sshkey::PublicKey::parse(SSH_KEY).unwrap();
        ssh::add(&keys, &key).unwrap();

        let log = Log::buffered(true);
        run(&device.env, &log);

        assert!(ssh::list(&keys).unwrap().is_empty());
        assert!(log
            .lines()
            .iter()
            .any(|line| line.contains("root had ssh keys")));
    }

    const SSH_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO7gEEtj0g4zaawVIwrP4wxLZQ2TqgASR86NTHDJ66jj a@laptop";

    #[test]
    fn a_claimed_device_keeps_its_password() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, auth::FILE)
            .update(&log, |auth: &mut Auth| {
                auth.issue("laptop", "claim").map(|_| ())
            })
            .unwrap();
        shadow::set_root(&paths.shadow, Some(&shadow::hash("kept").unwrap())).unwrap();
        let key = protocol::sshkey::PublicKey::parse(SSH_KEY).unwrap();
        ssh::add(&paths.authorized_keys, &key).unwrap();

        run(&device.env, &log);

        assert!(!root_is_empty(&paths.shadow));
        assert_eq!(ssh::list(&paths.authorized_keys).unwrap(), vec![key]);
    }

    #[test]
    fn the_marker_factory_resets_and_is_removed() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                state
                    .settings
                    .insert("browser.url".into(), "https://a.test/".into());
                Ok(())
            })
            .unwrap();
        Store::new(&paths.state_dir, auth::FILE)
            .update(&log, |auth: &mut Auth| {
                auth.issue("laptop", "claim").map(|_| ())
            })
            .unwrap();
        shadow::set_root(&paths.shadow, Some(&shadow::hash("x").unwrap())).unwrap();
        fs::create_dir_all(paths.files_dir.join("media")).unwrap();
        fs::write(paths.files_dir.join("media/clip.mp4"), "video").unwrap();
        fs::write(paths.factory_reset_marker(), "").unwrap();

        run(&device.env, &log);

        assert!(device.state().settings.is_empty());
        let auth: Auth = Store::new(&paths.state_dir, auth::FILE).read(&log);
        assert!(!auth.claimed());
        assert!(root_is_empty(&paths.shadow));
        assert!(!paths.factory_reset_marker().exists());
        assert_eq!(fs::read_dir(&paths.files_dir).unwrap().count(), 0);
    }

    #[test]
    fn the_kernel_command_line_factory_resets_too() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                state.settings.insert("screen.osk".into(), "never".into());
                Ok(())
            })
            .unwrap();
        fs::write(&paths.cmdline, "quiet tessaro.factory_reset\n").unwrap();

        run(&device.env, &log);

        assert!(device.state().settings.is_empty());
    }

    #[test]
    fn a_pending_change_at_boot_reverts() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                state
                    .settings
                    .insert("screen.resolution".into(), "640x480".into());
                state.pending = Some(state::PendingChange {
                    key: "screen.resolution".into(),
                    value: "640x480".into(),
                    previous: None,
                });
                Ok(())
            })
            .unwrap();

        run(&device.env, &log);

        let state = device.state();
        assert!(state.pending.is_none());
        assert!(!state.settings.contains_key("screen.resolution"));
    }

    #[test]
    fn the_old_override_file_is_imported_once() {
        let device = Device::new();
        let paths = device.paths();
        fs::write(
            &paths.legacy_override,
            "# a comment\n#KIOSK_URL=https://commented.test/\nKIOSK_URL=\"https://shop.test/\"\n\
             KIOSK_OSK=sometimes\nKIOSK_NOPE=1\nKIOSK_SCALE=2\n",
        )
        .unwrap();
        let log = Log::buffered(true);

        run(&device.env, &log);

        let state = device.state();
        assert_eq!(state.settings["browser.url"], "https://shop.test/");
        assert_eq!(state.settings["screen.scale"], "2");
        assert!(!state.settings.contains_key("screen.osk"));
        assert!(!paths.legacy_override.exists());
        assert!(paths.legacy_override.with_extension("migrated").exists());
        assert!(log.lines().iter().any(|line| line.contains("KIOSK_NOPE")));
        assert!(log.lines().iter().any(|line| line.contains("screen.osk")));
    }

    #[test]
    fn settings_saved_under_old_names_move_to_the_new_ones_once() {
        let mut device = Device::new();
        let nm = device._dir.path().join("nm");
        device
            .env
            .insert("KIOSK_NM_RUN_DIR".into(), nm.display().to_string());
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                for (key, value) in [
                    ("kiosk.url", "https://{node.name}.shop.test/?ip={net.ip}"),
                    ("debug.template", "{node.name}\\n{kiosk.url}"),
                    ("display.osk", "never"),
                    ("ethernet.mode", "static"),
                    ("ethernet.address", "192.168.1.50/24"),
                    ("ethernet.gateway", "192.168.1.1"),
                    ("node.name", "lobby"),
                    ("screen.scale", "2"),
                    ("display.scale", "1"),
                    ("data.table", "{node.name}"),
                ] {
                    state.settings.insert(key.into(), value.into());
                }
                Ok(())
            })
            .unwrap();

        run(&device.env, &log);

        let state = device.state();
        let settings: Vec<(&str, &str)> = state
            .settings
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            settings,
            [
                ("browser.debug.template", "{device.name}\\n{browser.url}"),
                (
                    "browser.url",
                    "https://{device.name}.shop.test/?ip={network.ip}"
                ),
                // A custom value is not a template: its text is its own.
                ("data.table", "{node.name}"),
                ("device.name", "lobby"),
                ("network.ethernet.address", "192.168.1.50/24"),
                ("network.ethernet.gateway", "192.168.1.1"),
                ("network.ethernet.mode", "static"),
                ("screen.osk", "never"),
                // Set under both names: the new one wins.
                ("screen.scale", "2"),
            ]
        );
        let lines = log.lines();
        assert!(lines
            .iter()
            .any(|line| line.contains("renamed setting: kiosk.url is now browser.url")));
        assert!(lines
            .iter()
            .any(|line| line.contains("display.scale dropped: screen.scale is set already")));
        // The same static profile as before the rename.
        assert!(lines
            .iter()
            .any(|line| line.contains("ethernet tessaro-ethernet-static")));
        let keyfiles: String = fs::read_dir(&nm)
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        assert!(keyfiles.contains("192.168.1.50/24"), "{keyfiles}");

        // Nothing left to do: the second boot does not touch the file.
        let revision = state.revision;
        let again = Log::buffered(true);
        run(&device.env, &again);
        assert_eq!(device.state().revision, revision);
        assert!(!again
            .lines()
            .iter()
            .any(|line| line.contains("renamed setting")));
    }

    #[test]
    fn a_pending_change_under_an_old_name_still_reverts() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                state
                    .settings
                    .insert("display.resolution".into(), "640x480".into());
                state.pending = Some(state::PendingChange {
                    key: "display.resolution".into(),
                    value: "640x480".into(),
                    previous: Some("1920x1080".into()),
                });
                Ok(())
            })
            .unwrap();

        run(&device.env, &log);

        let state = device.state();
        assert_eq!(state.settings["screen.resolution"], "1920x1080");
        assert!(!state.settings.contains_key("display.resolution"));
        assert!(state.pending.is_none());
    }
}
