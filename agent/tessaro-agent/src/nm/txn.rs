//! A change to the device's network as one transaction it keeps or undoes by
//! itself.
//!
//! The operator's client sends one request and may never hear back - the
//! change can take its own connection away - so nothing here waits for it.
//! What makes that safe is the order:
//!
//! 1. **Record** what is under way in `/data/tessaro/network/txn.json`.
//! 2. **Checkpoint** the devices involved in NetworkManager, with a rollback
//!    timer of its own: if this process dies, NetworkManager undoes the
//!    runtime half by itself.
//! 3. **Switch**: write the new keyfiles under `/run` (never saved anywhere),
//!    reload, bring profiles down and up, set the hotspot's NAT.
//! 4. **Verify on the device**: what was brought up comes up and gets an
//!    address, a default route is still there if there was one, and the
//!    `--verify` target answers.
//! 5. Only then **commit** - the caller's future, which writes `state.json` -
//!    and drop the checkpoint. On any failure, **roll back**: the old
//!    keyfiles, the old NAT, the checkpoint.
//!
//! The settings in `state.json` are the truth throughout: the boot oneshot
//! renders the profiles from them before NetworkManager starts, so a reboot
//! at any point comes back on the committed configuration, and an agent that
//! stops half way leaves the record behind for the next one to roll back
//! (`recover`).

use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use protocol::{ChangeOutcome, NetChange, NetCheck, Verify};
use serde::{Deserialize, Serialize};

use crate::deadline::blocking;
use crate::log::Log;
use crate::nm::profiles::{Keyfile, Profile};

/// How long a connection has to come up: association, the handshake, DHCP.
pub const ACTIVATE: Duration = Duration::from_secs(45);
/// After it is up, how long the route and the addresses have to settle.
const SETTLE: Duration = Duration::from_secs(20);
/// Tries at the `--verify` target, a second or two apart.
const REACH_TRIES: u32 = 5;
/// NetworkManager's own timer on the checkpoint, the backstop if the agent
/// dies: longer than anything above, so it never fires on a live change.
pub const BACKSTOP: Duration = Duration::from_secs(150);

const RECORD: &str = "txn.json";
const LAST: &str = "last.json";

/// What is under way. No secrets: those never leave `secrets.json` and the
/// keyfiles under `/run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub action: String,
    pub checkpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Up {
    pub profile: Profile,
    /// Where: the managed interface, or `None` for NetworkManager to choose.
    pub device: Option<String>,
}

/// The hotspot's NAT, before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nat {
    pub on: bool,
    pub was: bool,
    /// The `iifname` it matches: `Wifi::nat_match`.
    pub interface: String,
}

/// One change, resolved to keyfiles and profiles.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub action: String,
    /// Interfaces the checkpoint covers.
    pub devices: Vec<String>,
    /// The keyfiles as they will be, and as they are - for a rollback.
    pub new: Vec<Keyfile>,
    pub old: Vec<Keyfile>,
    pub down: Vec<Profile>,
    pub up: Vec<Up>,
    pub nat: Option<Nat>,
    pub verify: Verify,
    /// A default route there before must still be there. Off when the
    /// change gives up the WiFi client on purpose, whose route goes with it.
    pub keep_route: bool,
    pub note: Option<String>,
}

/// What activating a profile started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activated {
    /// The active connection to wait for.
    pub active: String,
    /// The interface it is coming up on.
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub interface: String,
    pub gateway: Option<IpAddr>,
}

/// Writes `state.json` once the change has held. Built by the caller, run
/// only after every check passed.
pub type Commit = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Everything the transaction asks of NetworkManager and the kernel. The
/// real one is `nm::Live`; the tests drive a fake that can fail anywhere.
#[async_trait::async_trait]
pub trait Ops: Send + Sync {
    async fn checkpoint(&self, devices: &[String], backstop: Duration) -> Result<String, String>;
    async fn rollback(&self, checkpoint: &str) -> Result<(), String>;
    async fn destroy(&self, checkpoint: &str) -> Result<(), String>;
    /// Replace the managed keyfiles with these, and have NetworkManager
    /// reload them.
    async fn write_profiles(&self, files: &[Keyfile]) -> Result<(), String>;
    async fn activate(&self, up: &Up) -> Result<Activated, String>;
    /// Take a profile down; a profile that is not up is not an error.
    async fn deactivate(&self, profile: Profile) -> Result<(), String>;
    /// Until the active connection is up, or why it will not be.
    async fn activated(&self, active: &str, limit: Duration) -> Result<(), String>;
    async fn set_nat(&self, on: bool, interface: &str) -> Result<(), String>;
    /// The kernel's IPv4 default route.
    async fn route(&self) -> Option<Route>;
    async fn has_address(&self, interface: &str) -> bool;
    /// The `--verify` target once: what answered, or why nothing did.
    async fn reach(&self, verify: &Verify, route: Option<&Route>) -> Result<String, String>;
    /// A pause between tries. Real time on a device, none in the tests.
    async fn pause(&self, pause: Duration);
}

/// The transaction's files under `/data/tessaro/network`.
#[derive(Debug, Clone)]
pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    async fn write<T: Serialize + Send + 'static>(
        &self,
        name: &str,
        value: T,
    ) -> Result<(), String> {
        let dir = self.dir.clone();
        let path = self.path(name);
        blocking("writing a network change", move || {
            create_private_dir(&dir)?;
            update::fsutil::write_json(&path, &value)
                .map_err(|err| format!("{}: {err}", path.display()))
        })
        .await
    }

    async fn read<T: for<'de> Deserialize<'de> + Send + 'static>(
        &self,
        name: &str,
    ) -> Result<Option<T>, String> {
        let path = self.path(name);
        blocking("reading a network change", move || {
            if !path.exists() {
                return Ok(None);
            }
            update::fsutil::read_json(&path).map(Some)
        })
        .await
    }

    async fn clear(&self) -> Result<(), String> {
        let record = self.path(RECORD);
        blocking("clearing a network change", move || {
            update::fsutil::remove_if_exists(&record)
                .map_err(|err| format!("{}: {err}", record.display()))
        })
        .await
    }

    pub async fn last(&self) -> Result<Option<NetChange>, String> {
        self.read(LAST).await
    }

    /// The record of a change that never finished, if one is there.
    pub async fn unfinished(&self) -> Result<Option<Record>, String> {
        self.read(RECORD).await
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|err| format!("{}: {err}", dir.display()))
}

fn pass(name: &str, detail: impl Into<String>) -> NetCheck {
    NetCheck {
        name: name.to_string(),
        passed: true,
        detail: detail.into(),
    }
}

fn fail(name: &str, detail: impl Into<String>) -> NetCheck {
    NetCheck {
        name: name.to_string(),
        passed: false,
        detail: detail.into(),
    }
}

/// A failed step: why, and the checks that ran before it.
struct Failed {
    reason: String,
    checks: Vec<NetCheck>,
}

impl Failed {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            checks: Vec::new(),
        }
    }
}

/// Run `plan` to the end: committed, or rolled back. `Err` only when it
/// could not even start - nothing was changed then.
pub async fn run(
    ops: &dyn Ops,
    files: &Files,
    plan: Plan,
    log: &Log,
    commit: Commit,
) -> Result<NetChange, String> {
    let route_before = ops.route().await;
    let mut record = Record {
        action: plan.action.clone(),
        checkpoint: None,
    };
    files.write(RECORD, record.clone()).await?;

    let checkpoint = match ops.checkpoint(&plan.devices, BACKSTOP).await {
        Ok(checkpoint) => checkpoint,
        Err(err) => {
            // Nothing has moved yet.
            let _ = files.clear().await;
            return Err(format!("NetworkManager refused a checkpoint: {err}"));
        }
    };
    record.checkpoint = Some(checkpoint.clone());
    files.write(RECORD, record.clone()).await?;
    log.info(format!("network: {} started", plan.action));

    // naked: attempt() waits only through ops.
    let outcome = attempt(ops, &plan, route_before.as_ref()).await;
    let outcome = match outcome {
        // naked: the caller's commit is blocking() under within()
        Ok(checks) => match commit.await {
            Ok(()) => Ok(checks),
            Err(reason) => Err(Failed {
                reason: format!("the settings could not be saved: {reason}"),
                checks,
            }),
        },
        Err(failed) => Err(failed),
    };

    let change = match outcome {
        Ok(checks) => {
            if let Err(err) = ops.destroy(&checkpoint).await {
                // Saved already; a checkpoint left behind would only roll
                // the runtime back at the backstop, onto what boot renders.
                log.info(format!("network: dropping the checkpoint: {err}"));
            }
            log.info(format!("network: {} committed", plan.action));
            NetChange {
                outcome: ChangeOutcome::Committed,
                action: plan.action.clone(),
                profile: plan.up.first().map(|up| up.profile.id.to_string()),
                uuid: None,
                reason: None,
                checks,
                note: plan.note.clone(),
            }
        }
        Err(failed) => {
            log.info(format!(
                "network: {} rolled back: {}",
                plan.action, failed.reason
            ));
            // naked: undo() waits only through ops.
            undo(ops, &plan.old, plan.nat.as_ref(), Some(&checkpoint), log).await;
            NetChange {
                outcome: ChangeOutcome::RolledBack,
                action: plan.action.clone(),
                profile: plan.up.first().map(|up| up.profile.id.to_string()),
                uuid: None,
                reason: Some(failed.reason),
                checks: failed.checks,
                note: None,
            }
        }
    };

    let _ = files.clear().await;
    if let Err(err) = files.write(LAST, change.clone()).await {
        log.info(format!("network: {err}"));
    }
    Ok(change)
}

/// Switch and verify. Every check that ran is in the answer either way.
async fn attempt(
    ops: &dyn Ops,
    plan: &Plan,
    route_before: Option<&Route>,
) -> Result<Vec<NetCheck>, Failed> {
    ops.write_profiles(&plan.new).await.map_err(Failed::new)?;
    for profile in &plan.down {
        ops.deactivate(*profile).await.map_err(Failed::new)?;
    }
    if let Some(nat) = &plan.nat {
        ops.set_nat(nat.on, &nat.interface)
            .await
            .map_err(|err| Failed::new(format!("setting the hotspot's NAT: {err}")))?;
    }

    let mut checks = Vec::new();
    for up in &plan.up {
        let started = match ops.activate(up).await {
            Ok(started) => started,
            Err(why) => {
                checks.push(fail(up.profile.id, &why));
                return Err(Failed {
                    reason: format!("{} could not be brought up: {why}", up.profile.id),
                    checks,
                });
            }
        };
        let device = started
            .device
            .clone()
            .or_else(|| up.device.clone())
            .unwrap_or_else(|| "its device".to_string());
        if let Err(why) = ops.activated(&started.active, ACTIVATE).await {
            checks.push(fail(up.profile.id, &why));
            return Err(Failed {
                reason: format!("{} did not come up: {why}", up.profile.id),
                checks,
            });
        }
        checks.push(pass(up.profile.id, format!("up on {device}")));

        if let Some(interface) = started.device.as_deref().or(up.device.as_deref()) {
            if settle(ops, SETTLE, || ops.has_address(interface)).await {
                checks.push(pass("address", format!("{interface} has an address")));
            } else {
                checks.push(fail("address", format!("{interface} has no address")));
                return Err(Failed {
                    reason: format!("{interface} got no address"),
                    checks,
                });
            }
        }
    }

    let mut route = ops.route().await;
    if let (true, Some(before)) = (plan.keep_route, route_before) {
        if route.is_none() && settle(ops, SETTLE, || async { ops.route().await.is_some() }).await {
            route = ops.route().await;
        }
        match &route {
            Some(now) => checks.push(pass(
                "route",
                format!("default route via {}", now.interface),
            )),
            None => {
                checks.push(fail(
                    "route",
                    format!(
                        "no default route any more (it was via {})",
                        before.interface
                    ),
                ));
                return Err(Failed {
                    reason: "the device lost its default route".to_string(),
                    checks,
                });
            }
        }
    }

    match (&plan.verify, &route) {
        (Verify::None, _) => {}
        (Verify::Gateway, None) => {
            checks.push(pass("reach", "no default route, so no gateway to ask"))
        }
        (verify, _) => {
            let mut last = String::new();
            let mut reached = None;
            for attempt in 0..REACH_TRIES {
                if attempt > 0 {
                    ops.pause(Duration::from_secs(1)).await;
                }
                match ops.reach(verify, route.as_ref()).await {
                    Ok(what) => {
                        reached = Some(what);
                        break;
                    }
                    Err(why) => last = why,
                }
            }
            match reached {
                Some(what) => checks.push(pass("reach", what)),
                None => {
                    checks.push(fail("reach", &last));
                    return Err(Failed {
                        reason: format!("{} did not hold: {last}", verify.describe()),
                        checks,
                    });
                }
            }
        }
    }
    Ok(checks)
}

/// Poll `ready` once a second until it holds or `limit` is up.
async fn settle<F, Fut>(ops: &dyn Ops, limit: Duration, ready: F) -> bool
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let tries = limit.as_secs().max(1);
    for attempt in 0..=tries {
        if attempt > 0 {
            ops.pause(Duration::from_secs(1)).await;
        }
        // naked: every caller passes an ops. method
        if ready().await {
            return true;
        }
    }
    false
}

/// Put the old configuration back, as far as it goes. The keyfiles first,
/// so the checkpoint brings back up profiles that already say the old thing.
/// Each part is tried whatever the others did: a half undone change is worse
/// than a noisy journal.
async fn undo(
    ops: &dyn Ops,
    old: &[Keyfile],
    nat: Option<&Nat>,
    checkpoint: Option<&str>,
    log: &Log,
) {
    if let Err(err) = ops.write_profiles(old).await {
        log.info(format!("network: restoring the profiles: {err}"));
    }
    if let Some(nat) = nat {
        if let Err(err) = ops.set_nat(nat.was, &nat.interface).await {
            log.info(format!("network: restoring the hotspot's NAT: {err}"));
        }
    }
    if let Some(checkpoint) = checkpoint {
        if let Err(err) = ops.rollback(checkpoint).await {
            log.debug(format!("network: checkpoint rollback: {err}"));
        }
    }
}

/// A change the previous agent never finished: undo it now, onto `current`
/// - the keyfiles `state.json` renders, which never saw the change.
pub async fn recover(
    ops: &dyn Ops,
    files: &Files,
    current: &[Keyfile],
    nat: &Nat,
    log: &Log,
) -> Result<bool, String> {
    let Some(record) = files.unfinished().await? else {
        return Ok(false);
    };
    log.info(format!(
        "network: rolled back an unfinished network change ({})",
        record.action
    ));
    let nat = Nat {
        was: nat.on,
        ..nat.clone()
    };
    // naked: undo() waits only through ops.
    undo(ops, current, Some(&nat), record.checkpoint.as_deref(), log).await;
    files.clear().await?;
    let change = NetChange {
        outcome: ChangeOutcome::RolledBack,
        action: record.action.clone(),
        profile: None,
        uuid: None,
        reason: Some("the agent stopped before it finished".to_string()),
        checks: Vec::new(),
        note: None,
    };
    files.write(LAST, change).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nm::profiles::{ETHERNET_DHCP, ETHERNET_STATIC};
    use std::sync::{Arc, Mutex};

    /// Records every call; fails the one named in `fail_at`.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail_at: Option<&'static str>,
        /// `activated` answers this error.
        not_activated: Option<String>,
        /// No default route once profiles have been switched.
        loses_route: bool,
    }

    impl Fake {
        fn failing(at: &'static str) -> Self {
            Self {
                fail_at: Some(at),
                ..Self::default()
            }
        }

        fn call(&self, name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(name.to_string());
            if self.fail_at == Some(name) {
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn switched(&self) -> bool {
            self.calls().iter().any(|call| call.starts_with("up "))
        }
    }

    #[async_trait::async_trait]
    impl Ops for Fake {
        async fn checkpoint(&self, _: &[String], _: Duration) -> Result<String, String> {
            self.call("checkpoint").map(|()| "/cp/1".to_string())
        }
        async fn rollback(&self, _: &str) -> Result<(), String> {
            self.call("rollback")
        }
        async fn destroy(&self, _: &str) -> Result<(), String> {
            self.call("destroy")
        }
        async fn write_profiles(&self, files: &[Keyfile]) -> Result<(), String> {
            let tag = files.first().map(|f| f.body.as_str()).unwrap_or("none");
            self.call(&format!("write {tag}"))?;
            self.call("write")
        }
        async fn activate(&self, up: &Up) -> Result<Activated, String> {
            self.call(&format!("up {}", up.profile.id))?;
            self.call("activate")?;
            Ok(Activated {
                active: "/ac/1".into(),
                device: up.device.clone(),
            })
        }
        async fn deactivate(&self, profile: Profile) -> Result<(), String> {
            self.call(&format!("down {}", profile.id))
        }
        async fn activated(&self, _: &str, _: Duration) -> Result<(), String> {
            self.call("activated")?;
            match &self.not_activated {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }
        async fn set_nat(&self, on: bool, _: &str) -> Result<(), String> {
            self.call(&format!("nat {on}"))
        }
        async fn route(&self) -> Option<Route> {
            if self.loses_route && self.switched() {
                return None;
            }
            Some(Route {
                interface: "eth0".into(),
                gateway: Some("10.0.2.2".parse().unwrap()),
            })
        }
        async fn has_address(&self, _: &str) -> bool {
            true
        }
        async fn reach(&self, _: &Verify, _: Option<&Route>) -> Result<String, String> {
            self.call("reach")
                .map(|()| "gateway answered".to_string())
                .map_err(|_| "gateway did not answer".to_string())
        }
        async fn pause(&self, _: Duration) {}
    }

    fn keyfile(tag: &str) -> Keyfile {
        Keyfile {
            name: "tessaro-ethernet-static.nmconnection".into(),
            body: tag.into(),
        }
    }

    fn to_static() -> Plan {
        Plan {
            action: "set network.ethernet.mode".into(),
            devices: vec!["eth0".into()],
            new: vec![keyfile("new")],
            old: vec![keyfile("old")],
            down: Vec::new(),
            up: vec![Up {
                profile: ETHERNET_STATIC,
                device: Some("eth0".into()),
            }],
            nat: None,
            verify: Verify::Gateway,
            keep_route: true,
            note: None,
        }
    }

    fn files() -> (tempfile::TempDir, Files) {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path().join("network"));
        (dir, files)
    }

    /// A commit that notes it ran, and fails if told to.
    fn commit(ran: &Arc<Mutex<bool>>, ok: bool) -> Commit {
        let ran = Arc::clone(ran);
        Box::pin(async move {
            *ran.lock().unwrap() = true;
            if ok {
                Ok(())
            } else {
                Err("disk full".to_string())
            }
        })
    }

    async fn run_with(
        fake: &Fake,
        plan: Plan,
        commit_ok: bool,
    ) -> (NetChange, bool, Files, tempfile::TempDir) {
        let (dir, files) = files();
        let ran = Arc::new(Mutex::new(false));
        let change = run(
            fake,
            &files,
            plan,
            &Log::buffered(false),
            commit(&ran, commit_ok),
        )
        .await
        .unwrap();
        let committed = *ran.lock().unwrap();
        (change, committed, files, dir)
    }

    #[tokio::test]
    async fn a_change_that_holds_is_committed_only_after_it_is_verified() {
        let fake = Fake::default();
        let (change, committed, files, _dir) = run_with(&fake, to_static(), true).await;

        assert_eq!(change.outcome, ChangeOutcome::Committed);
        assert!(committed);
        let calls = fake.calls();
        let at = |name: &str| calls.iter().position(|call| call == name).unwrap();
        assert!(at("checkpoint") < at("write new"));
        assert!(at("write new") < at("up tessaro-ethernet-static"));
        assert!(at("activated") < at("reach"));
        assert!(at("reach") < at("destroy"));
        assert!(!calls.contains(&"rollback".to_string()));
        assert!(!calls.contains(&"write old".to_string()));
        assert_eq!(files.unfinished().await.unwrap(), None);
        assert_eq!(files.last().await.unwrap(), Some(change));
    }

    #[tokio::test]
    async fn every_failure_rolls_back_and_commits_nothing() {
        for at in ["write", "activate", "activated", "reach"] {
            let fake = Fake::failing(at);
            let (change, committed, files, _dir) = run_with(&fake, to_static(), true).await;

            assert_eq!(change.outcome, ChangeOutcome::RolledBack, "failing {at}");
            assert!(!committed, "failing {at}: the settings were saved");
            let calls = fake.calls();
            assert!(calls.contains(&"write old".to_string()), "failing {at}");
            assert!(calls.contains(&"rollback".to_string()), "failing {at}");
            assert!(!calls.contains(&"destroy".to_string()), "failing {at}");
            assert_eq!(files.unfinished().await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn a_commit_that_fails_rolls_the_network_back_too() {
        let fake = Fake::default();
        let (change, _, _files, _dir) = run_with(&fake, to_static(), false).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(change.reason.unwrap().contains("disk full"));
        assert!(fake.calls().contains(&"rollback".to_string()));
    }

    #[tokio::test]
    async fn a_refused_checkpoint_changes_nothing() {
        let fake = Fake::failing("checkpoint");
        let (_dir, files) = files();
        let ran = Arc::new(Mutex::new(false));
        let outcome = run(
            &fake,
            &files,
            to_static(),
            &Log::buffered(false),
            commit(&ran, true),
        )
        .await;
        assert!(outcome.is_err());
        assert!(!fake.switched());
        assert!(!*ran.lock().unwrap());
        assert_eq!(files.unfinished().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_wrong_password_says_so() {
        let fake = Fake {
            not_activated: Some("no secrets (wrong password?)".into()),
            ..Fake::default()
        };
        let (change, committed, _files, _dir) = run_with(&fake, to_static(), true).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(!committed);
        assert!(change.reason.unwrap().contains("wrong password"));
    }

    #[tokio::test]
    async fn losing_the_default_route_is_a_failure() {
        let fake = Fake {
            loses_route: true,
            ..Fake::default()
        };
        let (change, committed, _files, _dir) = run_with(&fake, to_static(), true).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        assert!(!committed);
        assert!(change.checks.iter().any(|c| c.name == "route" && !c.passed));
    }

    #[tokio::test]
    async fn giving_up_the_wifi_uplink_on_purpose_is_not_a_lost_route() {
        let fake = Fake {
            loses_route: true,
            ..Fake::default()
        };
        let plan = Plan {
            keep_route: false,
            ..to_static()
        };
        let (change, committed, _files, _dir) = run_with(&fake, plan, true).await;
        assert_eq!(change.outcome, ChangeOutcome::Committed);
        assert!(committed);
    }

    #[tokio::test]
    async fn nat_is_set_and_put_back() {
        let fake = Fake::failing("reach");
        let plan = Plan {
            nat: Some(Nat {
                on: false,
                was: true,
                interface: "wlan0".into(),
            }),
            ..to_static()
        };
        let (change, _, _files, _dir) = run_with(&fake, plan, true).await;
        assert_eq!(change.outcome, ChangeOutcome::RolledBack);
        let calls = fake.calls();
        assert!(calls.contains(&"nat false".to_string()));
        assert!(calls.contains(&"nat true".to_string()));
    }

    #[tokio::test]
    async fn an_unfinished_change_is_rolled_back_onto_the_saved_settings() {
        let (_dir, files) = files();
        files
            .write(
                RECORD,
                Record {
                    action: "set network.ethernet.mode".into(),
                    checkpoint: Some("/cp/1".into()),
                },
            )
            .await
            .unwrap();

        let fake = Fake::default();
        let log = Log::buffered(false);
        let nat = Nat {
            on: true,
            was: true,
            interface: "wlan0".into(),
        };
        let current = vec![Keyfile {
            name: ETHERNET_DHCP.file_name(),
            body: "saved".into(),
        }];
        assert!(recover(&fake, &files, &current, &nat, &log).await.unwrap());
        let calls = fake.calls();
        for expected in ["write saved", "nat true", "rollback"] {
            assert!(
                calls.contains(&expected.to_string()),
                "{expected}: {calls:?}"
            );
        }
        assert!(log
            .lines()
            .iter()
            .any(|line| line.contains("rolled back an unfinished network change")));
        assert_eq!(files.unfinished().await.unwrap(), None);
        assert_eq!(
            files.last().await.unwrap().unwrap().outcome,
            ChangeOutcome::RolledBack
        );
        assert!(!recover(&fake, &files, &current, &nat, &log).await.unwrap());
    }
}
