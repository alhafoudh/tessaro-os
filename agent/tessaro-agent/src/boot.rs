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
//! 1. **Factory reset**, if `/data/tessaro/factory-reset` exists or the
//!    kernel command line says `tessaro.factory_reset`: settings, tokens and
//!    root password cleared - the fresh-install state. The marker is removed
//!    afterwards; the command-line flag is meant to be typed at the boot
//!    loader for one boot, not written into its config.
//! 2. **Migration** of a leftover `/etc/default/tessaro-kiosk`, once.
//! 3. **Probation**: a guarded change still pending at boot was never
//!    confirmed - the device was rebooted instead - so it reverts.
//! 4. **The claim invariant**: unclaimed means an empty root password. This
//!    is also what heals a power cut in the middle of a claim.
//! 5. **The TLS identity**, made if missing, so its fingerprint is in the
//!    journal from the first boot.
//! 6. **Render.**
//!
//! Every step logs and carries on. A failure here must never keep the kiosk
//! from booting: the worst outcome is the image's defaults.

use std::fs;

use crate::auth::{self, Auth};
use crate::config::Env;
use crate::identity;
use crate::log::Log;
use crate::paths::Paths;
use crate::render;
use crate::shadow;
use crate::state::{self, State};
use crate::store::Store;

pub fn run(env: &dyn Env, log: &Log) {
    let paths = Paths::load(env);
    let defaults = state::defaults(env);
    let state_store = Store::new(&paths.state_dir, state::FILE);
    let auth_store = Store::new(&paths.state_dir, auth::FILE);

    if factory_reset_requested(&paths) {
        factory_reset(&paths, &state_store, &auth_store, log);
    }

    migrate(&paths, &state_store, log);

    match state_store.update(log, |state: &mut State| Ok(state.revert_pending())) {
        Ok(Some(pending)) => log.info(format!(
            "{}={} was never confirmed; back to {}",
            pending.key,
            pending.value,
            pending.previous.as_deref().unwrap_or("the default")
        )),
        Ok(None) => {}
        Err(err) => log.info(format!("could not check for an unconfirmed change: {err}")),
    }

    reconcile(&paths, &auth_store, log);

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
        Ok(rendered) => log.info(format!(
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
        )),
        Err(err) => log.info(format!("render failed, the image defaults apply: {err}")),
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

fn factory_reset(paths: &Paths, state_store: &Store, auth_store: &Store, log: &Log) {
    // Tokens before the password, as everywhere else.
    if let Err(err) = auth_store.remove() {
        log.info(format!("factory reset: auth.json: {err}"));
    }
    if let Err(err) = shadow::set_root(&paths.shadow, None) {
        log.info(format!("factory reset: root password: {err}"));
    }
    if let Err(err) = state_store.remove() {
        log.info(format!("factory reset: state.json: {err}"));
    }
    match fs::remove_file(paths.factory_reset_marker()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log.info(format!("factory reset: cannot remove the marker: {err}")),
    }
    log.info("factory reset: settings, tokens and root password cleared");
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

fn reconcile(paths: &Paths, auth_store: &Store, log: &Log) {
    let auth: Auth = auth_store.read(log);
    if auth.claimed() {
        return;
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
                ("KIOSK_RUN_DIR", at("run")),
                ("KIOSK_POLICY", at("policy.json")),
                ("KIOSK_POLICY_BASE", at("policy-base.json")),
                ("KIOSK_SHADOW", at("etc/shadow")),
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

        run(&device.env, &log);

        assert!(!root_is_empty(&paths.shadow));
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
                    .insert("kiosk.url".into(), "https://a.test/".into());
                Ok(())
            })
            .unwrap();
        Store::new(&paths.state_dir, auth::FILE)
            .update(&log, |auth: &mut Auth| {
                auth.issue("laptop", "claim").map(|_| ())
            })
            .unwrap();
        shadow::set_root(&paths.shadow, Some(&shadow::hash("x").unwrap())).unwrap();
        fs::write(paths.factory_reset_marker(), "").unwrap();

        run(&device.env, &log);

        assert!(device.state().settings.is_empty());
        let auth: Auth = Store::new(&paths.state_dir, auth::FILE).read(&log);
        assert!(!auth.claimed());
        assert!(root_is_empty(&paths.shadow));
        assert!(!paths.factory_reset_marker().exists());
    }

    #[test]
    fn the_kernel_command_line_factory_resets_too() {
        let device = Device::new();
        let paths = device.paths();
        let log = Log::buffered(true);
        Store::new(&paths.state_dir, state::FILE)
            .update(&log, |state: &mut State| {
                state.settings.insert("display.osk".into(), "never".into());
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
                    .insert("display.resolution".into(), "640x480".into());
                state.pending = Some(state::PendingChange {
                    key: "display.resolution".into(),
                    value: "640x480".into(),
                    previous: None,
                });
                Ok(())
            })
            .unwrap();

        run(&device.env, &log);

        let state = device.state();
        assert!(state.pending.is_none());
        assert!(!state.settings.contains_key("display.resolution"));
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
        assert_eq!(state.settings["kiosk.url"], "https://shop.test/");
        assert_eq!(state.settings["display.scale"], "2");
        assert!(!state.settings.contains_key("display.osk"));
        assert!(!paths.legacy_override.exists());
        assert!(paths.legacy_override.with_extension("migrated").exists());
        assert!(log.lines().iter().any(|line| line.contains("KIOSK_NOPE")));
        assert!(log.lines().iter().any(|line| line.contains("display.osk")));
    }
}
