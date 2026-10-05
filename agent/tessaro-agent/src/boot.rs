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
//! 0. **The store**: `tessaro.db` opened and migrated (`db.rs`), a broken
//!    one set aside. The agent starts after this, on the migrated schema.
//! 1. **The last update**: what the initramfs did with a pending image, put
//!    in the journal once (`updates::report`).
//! 2. **Factory reset**, if `/data/tessaro/factory-reset` exists or the
//!    kernel command line says `tessaro.factory_reset`: settings, tokens, ssh
//!    keys, root password, extra certificate authorities, browser policies,
//!    schedules and stored files cleared - the fresh-install state.
//!    What a reset left of the file store to delete is finished every boot.
//!    The marker is removed
//!    afterwards; the command-line flag is meant to be typed at the boot
//!    loader for one boot, not written into its config.
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

use crate::auth::Auth;
use crate::config::Env;
use crate::db::Db;
use crate::files;
use crate::identity;
use crate::log::Log;
use crate::nm::{nat, profiles};
use crate::paths::Paths;
use crate::render;
use crate::secrets::Secrets;
use crate::shadow;
use crate::ssh;
use crate::state::{self, State};
use crate::updates;

pub fn run(env: &dyn Env, log: &Log) {
    let paths = Paths::load(env);
    let defaults = state::defaults(env);
    let db = Db::open(&paths.state_dir, log);

    updates::report(&paths, log);

    if factory_reset_requested(&paths) {
        factory_reset(&paths, &db, log);
    }
    if let Err(err) = files::clean(&paths) {
        log.info(format!(
            "could not finish removing the old file store: {err}"
        ));
    }

    match db.update(|state: &mut State| Ok(state.revert_pending())) {
        Ok(pending) if pending.is_empty() => {}
        Ok(pending) => log.info(format!(
            "{} never confirmed; {}",
            state::listed(&pending),
            state::listed_back(&pending)
        )),
        Err(err) => log.info(format!("could not check for an unconfirmed change: {err}")),
    }

    reconcile(&paths, &db, log);

    match identity::tls(&paths.tls_dir()) {
        Ok((tls, made)) => log.info(format!(
            "TLS identity {}: fingerprint {}",
            if made { "created" } else { "present" },
            tls.fingerprint
        )),
        Err(err) => log.info(format!("no TLS identity: {err}")),
    }

    let state: State = db.read(log);
    match render::state(&paths, &defaults, &state, log) {
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

    network(&paths, &defaults, &state, &db.read(log), log);
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

fn factory_reset(paths: &Paths, db: &Db, log: &Log) {
    // Tokens before the password, as everywhere else.
    if let Err(err) = db.clear::<Auth>() {
        log.info(format!("factory reset: {err}"));
    }
    if let Err(err) = ssh::clear(&paths.authorized_keys) {
        log.info(format!("factory reset: authorized_keys: {err}"));
    }
    if let Err(err) = shadow::set_root(&paths.shadow, None) {
        log.info(format!("factory reset: root password: {err}"));
    }
    if let Err(err) = db.clear::<State>() {
        log.info(format!("factory reset: {err}"));
    }
    if let Err(err) = db.clear::<Secrets>() {
        log.info(format!("factory reset: {err}"));
    }
    if let Err(err) = files::wipe(paths) {
        log.info(format!("factory reset: the file store: {err}"));
    }
    if let Err(err) = crate::certs::clear(&paths.ca_certs_dir()) {
        log.info(format!("factory reset: the certificate authorities: {err}"));
    }
    if let Err(err) = crate::policies::clear(db) {
        log.info(format!("factory reset: {err}"));
    }
    if let Err(err) = crate::schedules::clear(db) {
        log.info(format!("factory reset: the schedules: {err}"));
    }
    if let Err(err) = crate::scripts::clear(db, &paths.script_runs_dir()) {
        log.info(format!("factory reset: the scripts: {err}"));
    }
    match fs::remove_file(paths.factory_reset_marker()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log.info(format!("factory reset: cannot remove the marker: {err}")),
    }
    log.info(
        "factory reset: settings, tokens, ssh keys, root and network passwords, \
         certificate authorities, browser policies, scripts, schedules, stored files cleared",
    );
}

fn reconcile(paths: &Paths, db: &Db, log: &Log) {
    let auth: Auth = db.read(log);
    if auth.claimed() {
        return;
    }
    let secrets: Secrets = db.read(log);
    if secrets.hotspot_psk.is_some() {
        match db.update(|secrets: &mut Secrets| {
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
                ("KIOSK_CMDLINE", at("cmdline")),
                ("KIOSK_PROXY_CONFIG", at("tinyproxy.conf")),
                ("KIOSK_CAMERA_DIR", at("camera")),
                ("KIOSK_CAMERA_ENV", at("camera/camera.env")),
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

        fn db(&self) -> Db {
            Db::open(&self.paths().state_dir, &Log::buffered(true))
        }

        fn state(&self) -> State {
            self.db().read(&Log::buffered(true))
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
        device
            .db()
            .update(|auth: &mut Auth| auth.issue("laptop", "claim").map(|_| ()))
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
        let db = device.db();
        db.update(|state: &mut State| {
            state
                .settings
                .insert("browser.url".into(), "https://a.test/".into());
            Ok(())
        })
        .unwrap();
        db.update(|auth: &mut Auth| auth.issue("laptop", "claim").map(|_| ()))
            .unwrap();
        shadow::set_root(&paths.shadow, Some(&shadow::hash("x").unwrap())).unwrap();
        fs::create_dir_all(paths.files_dir.join("media")).unwrap();
        fs::write(paths.files_dir.join("media/clip.mp4"), "video").unwrap();
        fs::write(paths.factory_reset_marker(), "").unwrap();

        run(&device.env, &log);

        assert!(device.state().settings.is_empty());
        let auth: Auth = db.read(&log);
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
        device
            .db()
            .update(|state: &mut State| {
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
        let log = Log::buffered(true);
        device
            .db()
            .update(|state: &mut State| {
                state
                    .settings
                    .insert("screen.resolution".into(), "640x480".into());
                state.settings.insert("screen.rotation".into(), "90".into());
                state.pending = vec![
                    state::PendingChange {
                        key: "screen.resolution".into(),
                        value: "640x480".into(),
                        previous: None,
                    },
                    state::PendingChange {
                        key: "screen.rotation".into(),
                        value: "90".into(),
                        previous: Some("180".into()),
                    },
                ];
                Ok(())
            })
            .unwrap();

        run(&device.env, &log);

        let state = device.state();
        assert!(state.pending.is_empty());
        assert!(!state.settings.contains_key("screen.resolution"));
        assert_eq!(state.settings["screen.rotation"], "180");
        // The splash never saw the turn on probation, and keeps the one before.
        assert_eq!(
            fs::read_to_string(device.paths().splash_env())
                .unwrap()
                .lines()
                .last(),
            Some("PSPLASH_ARGS=--angle 180")
        );
    }

    #[test]
    fn a_broken_store_boots_on_the_defaults() {
        let device = Device::new();
        let paths = device.paths();
        fs::create_dir_all(&paths.state_dir).unwrap();
        fs::write(paths.state_dir.join(crate::db::FILE), vec![0x5a; 8192]).unwrap();
        let log = Log::buffered(true);

        run(&device.env, &log);

        assert!(paths.generated_env().exists());
        assert!(device.state().settings.is_empty());
        assert!(log.lines().iter().any(|line| line.contains("was broken")));
    }
}
