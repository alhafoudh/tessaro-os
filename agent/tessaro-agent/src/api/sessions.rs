//! Webconfig's browser sessions and the one-time tickets that start them
//! (docs/webconfig.md).
//!
//! A browser cannot pin the device's certificate or keep a token safe, so
//! signing in with a token or a ticket gives it a session instead: a random
//! id in an `HttpOnly` cookie, standing in for that token. Everything is in
//! memory, as hashes, like `auth.json` keeps tokens. An agent that stops ends
//! every session, except when it restarts itself to apply a change: then
//! `save` hands them to the next process through `/run` (`load`).
//!
//! A session remembers its token's id and stored hash, and is only good while
//! the device still has that token: revoking it, unclaiming or a factory
//! reset ends the session without anything here being told.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use openssl::sha::sha256;
use protocol::hex;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::auth::random;
use crate::sync::lock;

/// Sessions kept at once; the one used longest ago goes first.
const MAX_SESSIONS: usize = 32;
/// How long a ticket can be redeemed in, and how many can wait at once.
pub const TICKET_LIFE: Duration = Duration::from_secs(60);
const MAX_TICKETS: usize = 16;

/// The handover file, in the agent's run directory.
pub const HANDOVER: &str = "sessions.json";

/// Whose a session or a ticket is: a token, by id and stored hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub token_id: String,
    pub token_sha: String,
}

struct Entry {
    owner: Owner,
    last_active: Instant,
}

struct Pending {
    owner: Owner,
    until: Instant,
}

#[derive(Default)]
struct Inner {
    /// By the SHA-256 of the session id, hex.
    live: HashMap<String, Entry>,
    /// By the SHA-256 of the ticket, hex.
    tickets: HashMap<String, Pending>,
}

pub struct Sessions {
    inner: Mutex<Inner>,
    /// `__Host-tessaro-<node id>`: cookies ignore ports, so two devices
    /// behind one address (qemu forwards) must not share a name.
    cookie: String,
}

/// What one session looks like in the handover file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    id_sha: String,
    owner: Owner,
    idle_s: u64,
}

impl Sessions {
    pub fn new(node_id: &str) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cookie: format!("__Host-tessaro-{node_id}"),
        }
    }

    #[cfg(test)]
    pub fn cookie_name(&self) -> &str {
        &self.cookie
    }

    /// A new session for `owner`: the id to put in the cookie.
    pub fn create(&self, owner: Owner, timeout: Duration) -> Result<String, String> {
        let id = hex(&random(32)?);
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        inner
            .live
            .retain(|_, entry| now.duration_since(entry.last_active) < timeout);
        while inner.live.len() >= MAX_SESSIONS {
            let Some(oldest) = inner
                .live
                .iter()
                .min_by_key(|(_, entry)| entry.last_active)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            inner.live.remove(&oldest);
        }
        inner.live.insert(
            digest(&id),
            Entry {
                owner,
                last_active: now,
            },
        );
        Ok(id)
    }

    /// The owner of session `id`, if it is still good: used within
    /// `timeout`, and its token still the device's (`current` answers the
    /// stored hash of a token id). `active` counts this as use.
    pub fn check(
        &self,
        id: &str,
        timeout: Duration,
        active: bool,
        current: impl Fn(&str) -> Option<String>,
    ) -> Option<Owner> {
        let key = digest(id);
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        let entry = inner.live.get_mut(&key)?;
        let good = now.duration_since(entry.last_active) < timeout
            && current(&entry.owner.token_id).as_deref() == Some(entry.owner.token_sha.as_str());
        if !good {
            inner.live.remove(&key);
            return None;
        }
        if active {
            entry.last_active = now;
        }
        Some(entry.owner.clone())
    }

    /// End session `id`, if there is one.
    pub fn end(&self, id: &str) {
        lock(&self.inner).live.remove(&digest(id));
    }

    /// A ticket that `redeem` turns into a session for `owner`, once.
    pub fn ticket(&self, owner: Owner) -> Result<String, String> {
        let ticket = hex(&random(32)?);
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        inner.tickets.retain(|_, pending| now < pending.until);
        if inner.tickets.len() >= MAX_TICKETS {
            return Err("too many tickets waiting to be redeemed; try again in a minute".into());
        }
        inner.tickets.insert(
            digest(&ticket),
            Pending {
                owner,
                until: now + TICKET_LIFE,
            },
        );
        Ok(ticket)
    }

    /// Whose `ticket` is, if it has not expired or been redeemed before.
    pub fn redeem(&self, ticket: &str) -> Option<Owner> {
        let now = Instant::now();
        let pending = lock(&self.inner).tickets.remove(&digest(ticket))?;
        (now < pending.until).then_some(pending.owner)
    }

    /// Write the sessions still good for the next agent process, which
    /// `load`s them. Tickets are not handed over: they last a minute.
    pub fn save(&self, path: &Path, timeout: Duration) -> Result<usize, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let now = Instant::now();
        let saved: Vec<Saved> = lock(&self.inner)
            .live
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.last_active) < timeout)
            .map(|(id_sha, entry)| Saved {
                id_sha: id_sha.clone(),
                owner: entry.owner.clone(),
                idle_s: now.duration_since(entry.last_active).as_secs(),
            })
            .collect();
        if saved.is_empty() {
            return Ok(0);
        }
        let text = serde_json::to_vec(&saved).map_err(|err| err.to_string())?;
        let partial = path.with_extension("partial");
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&partial)
            .and_then(|mut file| file.write_all(&text))
            .and_then(|()| std::fs::rename(&partial, path));
        written.map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(saved.len())
    }

    /// Take over the sessions the previous agent process saved, and remove
    /// the file, so a later start does not take them again.
    pub fn load(&self, path: &Path) -> Result<usize, String> {
        let text = match std::fs::read(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        let _ = std::fs::remove_file(path);
        let saved: Vec<Saved> =
            serde_json::from_slice(&text).map_err(|err| format!("{}: {err}", path.display()))?;
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        for session in saved.iter().take(MAX_SESSIONS) {
            let idle = Duration::from_secs(session.idle_s);
            let last_active = now.checked_sub(idle).unwrap_or(now);
            inner.live.insert(
                session.id_sha.clone(),
                Entry {
                    owner: session.owner.clone(),
                    last_active,
                },
            );
        }
        Ok(inner.live.len())
    }

    /// The session id a request's `Cookie` header carries, if it does.
    pub fn session_in<'a>(&self, header: &'a str) -> Option<&'a str> {
        header.split(';').find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == self.cookie && !value.is_empty()).then_some(value)
        })
    }

    /// The `Set-Cookie` that gives the browser session `id` for `timeout`.
    pub fn set_cookie(&self, id: &str, timeout: Duration) -> String {
        format!(
            "{}={id}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
            self.cookie,
            timeout.as_secs()
        )
    }

    /// The `Set-Cookie` that makes the browser forget it.
    pub fn clear_cookie(&self) -> String {
        format!(
            "{}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            self.cookie
        )
    }
}

fn digest(secret: &str) -> String {
    hex(&sha256(secret.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: Duration = Duration::from_secs(7 * 24 * 3600);

    fn owner(id: &str) -> Owner {
        Owner {
            token_id: id.into(),
            token_sha: format!("sha-{id}"),
        }
    }

    fn device_has(ids: &'static [&'static str]) -> impl Fn(&str) -> Option<String> {
        move |id| ids.contains(&id).then(|| format!("sha-{id}"))
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_lasts_while_it_is_used() {
        let sessions = Sessions::new("abcd");
        let id = sessions.create(owner("t1"), WEEK).unwrap();
        let has = device_has(&["t1"]);

        tokio::time::advance(WEEK - Duration::from_secs(60)).await;
        assert_eq!(sessions.check(&id, WEEK, true, &has), Some(owner("t1")));
        tokio::time::advance(WEEK - Duration::from_secs(60)).await;
        assert!(sessions.check(&id, WEEK, false, &has).is_some());
        // That was not use: a week after the last use it is gone.
        tokio::time::advance(Duration::from_secs(120)).await;
        assert!(sessions.check(&id, WEEK, true, &has).is_none());
        assert!(sessions.check(&id, WEEK, true, &has).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_shorter_timeout_applies_at_once() {
        let sessions = Sessions::new("abcd");
        let id = sessions.create(owner("t1"), WEEK).unwrap();
        tokio::time::advance(Duration::from_secs(600)).await;
        let short = Duration::from_secs(300);
        assert!(sessions
            .check(&id, short, true, device_has(&["t1"]))
            .is_none());
    }

    #[tokio::test]
    async fn a_session_ends_with_its_token() {
        let sessions = Sessions::new("abcd");
        let id = sessions.create(owner("t1"), WEEK).unwrap();
        assert!(sessions.check(&id, WEEK, true, device_has(&[])).is_none());
        // Not revived by the same id coming back with another secret.
        let reissued = |id: &str| (id == "t1").then(|| "sha-other".to_string());
        let id = sessions.create(owner("t1"), WEEK).unwrap();
        assert!(sessions.check(&id, WEEK, true, reissued).is_none());
        assert!(sessions
            .check("nonsense", WEEK, true, device_has(&["t1"]))
            .is_none());
    }

    #[tokio::test]
    async fn signing_out_ends_only_that_session() {
        let sessions = Sessions::new("abcd");
        let one = sessions.create(owner("t1"), WEEK).unwrap();
        let two = sessions.create(owner("t1"), WEEK).unwrap();
        sessions.end(&one);
        let has = device_has(&["t1"]);
        assert!(sessions.check(&one, WEEK, true, &has).is_none());
        assert!(sessions.check(&two, WEEK, true, &has).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn the_least_recently_used_session_makes_room() {
        let sessions = Sessions::new("abcd");
        let has = device_has(&["t1"]);
        let first = sessions.create(owner("t1"), WEEK).unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        let ids: Vec<String> = (0..MAX_SESSIONS)
            .map(|_| sessions.create(owner("t1"), WEEK).unwrap())
            .collect();
        assert!(sessions.check(&first, WEEK, true, &has).is_none());
        assert!(ids
            .iter()
            .all(|id| sessions.check(id, WEEK, false, &has).is_some()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_ticket_is_good_once_and_for_a_minute() {
        let sessions = Sessions::new("abcd");
        let ticket = sessions.ticket(owner("t1")).unwrap();
        assert_eq!(sessions.redeem(&ticket), Some(owner("t1")));
        assert_eq!(sessions.redeem(&ticket), None);

        let ticket = sessions.ticket(owner("t1")).unwrap();
        tokio::time::advance(TICKET_LIFE + Duration::from_secs(1)).await;
        assert_eq!(sessions.redeem(&ticket), None);
        assert_eq!(sessions.redeem("nonsense"), None);
    }

    #[tokio::test]
    async fn tickets_waiting_are_bounded() {
        let sessions = Sessions::new("abcd");
        for _ in 0..MAX_TICKETS {
            sessions.ticket(owner("t1")).unwrap();
        }
        assert!(sessions.ticket(owner("t1")).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn sessions_survive_a_handover_and_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(HANDOVER);
        let before = Sessions::new("abcd");
        let id = before.create(owner("t1"), WEEK).unwrap();
        tokio::time::advance(Duration::from_secs(100)).await;
        assert_eq!(before.save(&file, WEEK).unwrap(), 1);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&file).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        assert!(!std::fs::read_to_string(&file).unwrap().contains(&id));

        let after = Sessions::new("abcd");
        assert_eq!(after.load(&file).unwrap(), 1);
        assert!(!file.exists());
        let has = device_has(&["t1"]);
        assert!(after.check(&id, WEEK, false, &has).is_some());
        // Its idle time came along.
        let almost = Duration::from_secs(150);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(after.check(&id, almost, false, &has).is_none());

        assert_eq!(Sessions::new("abcd").load(&file).unwrap(), 0);
    }

    #[test]
    fn the_cookie_is_found_among_others_and_named_for_the_node() {
        let sessions = Sessions::new("abcd");
        assert_eq!(sessions.cookie_name(), "__Host-tessaro-abcd");
        assert_eq!(
            sessions.session_in("a=b; __Host-tessaro-abcd=f00; c=d"),
            Some("f00")
        );
        assert_eq!(sessions.session_in("__Host-tessaro-other=f00"), None);
        assert_eq!(sessions.session_in("__Host-tessaro-abcd="), None);
        let set = sessions.set_cookie("f00", WEEK);
        assert!(set.starts_with("__Host-tessaro-abcd=f00; Path=/; Secure; HttpOnly"));
        assert!(set.contains("SameSite=Strict"));
        assert!(set.ends_with("Max-Age=604800"));
        assert!(sessions.clear_cookie().contains("Max-Age=0"));
    }
}
