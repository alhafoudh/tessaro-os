//! The devices this client knows: the `nodes` table of this client's
//! `tessaro.db` (`store.rs`).
//!
//! Keyed by **node id**, not by address or name. DHCP moves a device, an
//! operator renames it; neither makes it a different device, so neither may
//! break its pin. The store holds tokens, so it is 0600.
//!
//! `Nodes` is a copy read at one moment. Every change writes its one row
//! straight to the store as well, never the whole copy back, so a copy that
//! has gone stale (the GUI keeps one for as long as it runs) cannot undo
//! what tessaro-ctl or another thread stored meanwhile.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tessaro_db::rusqlite::{params, Connection};

use crate::connect::{Found, Session};
use crate::store;
use crate::tags;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub name: String,
    /// Last address it answered on, `ip:port`.
    pub address: String,
    /// SHA-256 of its TLS certificate, pinned on first use.
    pub fingerprint: String,
    pub token: Option<String>,
    /// device.tags as it last announced or reported them.
    pub tags: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Nodes {
    pub nodes: Vec<Node>,
}

pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSARO_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("tessaro");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".config").join("tessaro")
}

/// Replace `path` with `body`, readable by this user only (0600), through a
/// synced temporary beside it: a file this client keeps pins in for ssh
/// (`known_hosts`) is never half-written. Makes the directory if it is
/// missing.
pub fn write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no directory", path.display()))?;
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = dir.join(format!(".{name}.tmp"));

    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    file.write_all(body)
        .and_then(|()| file.sync_all())
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
}

/// Write `node`'s row: insert it, or replace the one with its id in place.
fn store_row(db: &Connection, node: &Node) -> Result<(), String> {
    db.execute(
        "INSERT INTO nodes (id, name, address, fingerprint, token, tags) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, address = excluded.address, \
         fingerprint = excluded.fingerprint, token = excluded.token, tags = excluded.tags",
        params![
            node.id,
            node.name,
            node.address,
            node.fingerprint,
            node.token,
            node.tags.join(",")
        ],
    )
    .map(drop)
    .map_err(|err| format!("{}: {err}", store::path().display()))
}

fn delete_row(db: &Connection, id: &str) -> Result<(), String> {
    db.execute("DELETE FROM nodes WHERE id = ?1", params![id])
        .map(drop)
        .map_err(|err| format!("{}: {err}", store::path().display()))
}

impl Nodes {
    /// Every known node. None, and no store made, when there is no store
    /// yet: a command that only reads must work where the config dir cannot
    /// be written (tessaro-ctl on the device, with no HOME, has `/`).
    pub fn load() -> Result<Self, String> {
        match store::open_existing()? {
            Some(db) => Self::load_from(&db),
            None => Ok(Self::default()),
        }
    }

    fn load_from(db: &Connection) -> Result<Self, String> {
        let fail = |err: tessaro_db::rusqlite::Error| format!("{}: {err}", store::path().display());
        let mut rows = db
            .prepare("SELECT id, name, address, fingerprint, token, tags FROM nodes ORDER BY rowid")
            .map_err(fail)?;
        let nodes = rows
            .query_map([], |row| {
                Ok(Node {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    address: row.get(2)?,
                    fingerprint: row.get(3)?,
                    token: row.get(4)?,
                    tags: tags::split(&row.get::<_, String>(5)?),
                })
            })
            .map_err(fail)?
            .collect::<Result<_, _>>()
            .map_err(fail)?;
        Ok(Self { nodes })
    }

    /// Insert or replace `node` by id, here and in the store.
    pub fn keep(&mut self, node: Node) -> Result<(), String> {
        store_row(&store::open()?, &node)?;
        self.put(node);
        Ok(())
    }

    /// Drop node `id`, here and from the store. Whether this copy knew it.
    pub fn forget(&mut self, id: &str) -> Result<bool, String> {
        delete_row(&store::open()?, id)?;
        Ok(self.remove(id))
    }

    pub fn by_id(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.name == name)
    }

    /// Every node whose name or id starts with `start`, once each.
    pub fn by_prefix(&self, start: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.name.starts_with(start) || node.id.starts_with(start))
            .collect()
    }

    pub fn by_address(&self, address: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.address == address)
    }

    /// Insert or replace by id, in this copy only.
    pub fn put(&mut self, node: Node) {
        match self.nodes.iter_mut().find(|known| known.id == node.id) {
            Some(known) => *known = node,
            None => self.nodes.push(node),
        }
    }

    fn remove(&mut self, id: &str) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|node| node.id != id);
        self.nodes.len() != before
    }

    /// A known device that answered, pin and id checked, somewhere other
    /// than its cached address has moved: remember where, so the next
    /// connection by name goes straight there instead of scanning. Its tags
    /// are kept as it reports them too. Returns the address it was at, when
    /// it moved.
    pub fn refresh(&mut self, session: &Session) -> Result<Option<String>, String> {
        let Some((address, _)) = &session.remote else {
            return Ok(None);
        };
        let address = address.to_string();
        let Some(known) = self.by_id(&session.node.id) else {
            return Ok(None);
        };
        if known.address == address && known.tags == session.node.tags {
            return Ok(None);
        }
        let was = (known.address != address).then(|| known.address.clone());
        let moved = Node {
            address,
            tags: session.node.tags.clone(),
            ..known.clone()
        };
        self.keep(moved)?;
        Ok(was)
    }

    /// Keep the tags a known device announced, so it shows them while it
    /// is offline. Only the tags: an announcement is not checked against the
    /// pin, so it moves nothing else. Whether anything changed.
    pub fn note_found(&mut self, found: &Found) -> Result<bool, String> {
        let Some(id) = &found.id else {
            return Ok(false);
        };
        let Some(known) = self.by_id(id) else {
            return Ok(false);
        };
        if known.tags == found.tags {
            return Ok(false);
        }
        let tagged = Node {
            tags: found.tags.clone(),
            ..known.clone()
        };
        self.keep(tagged)?;
        Ok(true)
    }

    /// Pin the device this remote session talks to, with `token`, and store
    /// it.
    pub fn remember(&mut self, session: &Session, token: Option<String>) -> Result<(), String> {
        let (address, fingerprint) = session
            .remote
            .clone()
            .ok_or_else(|| "no remote session to remember".to_string())?;
        self.keep(Node {
            id: session.node.id.clone(),
            name: session.node.name.clone(),
            address: address.to_string(),
            fingerprint,
            token,
            tags: session.node.tags.clone(),
        })
    }

    /// Drop the device this remote session talks to, here and from the
    /// store. Whether this copy knew it.
    pub fn forget_session(&mut self, session: &Session) -> Result<bool, String> {
        if session.remote.is_none() || self.by_id(&session.node.id).is_none() {
            return Ok(false);
        }
        self.forget(&session.node.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, address: &str) -> Node {
        Node {
            id: id.to_string(),
            name: format!("name-{id}"),
            address: address.to_string(),
            fingerprint: "f".repeat(64),
            token: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn a_node_that_moved_is_the_same_node() {
        let mut nodes = Nodes::default();
        nodes.put(node("a", "10.0.0.5:7400"));
        nodes.put(node("a", "10.0.0.9:7400"));

        assert_eq!(nodes.nodes.len(), 1);
        assert_eq!(nodes.by_id("a").unwrap().address, "10.0.0.9:7400");
        assert!(nodes.by_address("10.0.0.5:7400").is_none());
    }

    #[test]
    fn a_prefix_matches_a_name_or_an_id_once() {
        let mut nodes = Nodes::default();
        nodes.put(node("a1", "10.0.0.5:7400"));
        nodes.put(node("b2", "10.0.0.6:7400"));
        let ids = |start| {
            nodes
                .by_prefix(start)
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(ids("name-"), ["a1", "b2"]);
        assert_eq!(ids("name-b"), ["b2"]);
        assert_eq!(ids("a"), ["a1"]);
        assert!(ids("c").is_empty());
    }

    #[test]
    fn stored_and_loaded_privately_and_a_stale_copy_undoes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this crate that touch the environment run here only.
        unsafe { std::env::set_var("TESSARO_CONFIG_DIR", dir.path().join("sub")) };

        // Reading makes nothing: the config dir may not be writable.
        let mut nodes = Nodes::load().unwrap();
        assert!(nodes.nodes.is_empty());
        assert!(!dir.path().join("sub").exists());
        nodes
            .keep(Node {
                token: Some("tsr_x".to_string()),
                ..node("a", "10.0.0.5:7400")
            })
            .unwrap();
        nodes.keep(node("b", "10.0.0.6:7400")).unwrap();

        // An announcement keeps the tags of a known device, and only them.
        let seen = Found {
            name: "elsewhere".to_string(),
            address: "10.0.0.99:7400".parse().unwrap(),
            id: Some("b".to_string()),
            fingerprint: None,
            claimed: Some(true),
            tags: vec!["floor-2".to_string(), "lobby".to_string()],
        };
        assert!(nodes.note_found(&seen).unwrap());
        assert!(!nodes.note_found(&seen).unwrap());

        let loaded = Nodes::load().unwrap();
        assert_eq!(
            loaded.by_name("name-a").unwrap().token.as_deref(),
            Some("tsr_x")
        );
        let b = loaded.by_id("b").unwrap();
        assert_eq!(b.tags, ["floor-2", "lobby"]);
        assert_eq!(b.address, "10.0.0.6:7400");
        assert!(loaded.by_id("a").unwrap().tags.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store::path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // The GUI's copy, from before tessaro-ctl logged in to b.
        let mut stale = Nodes::load().unwrap();
        let mut ctl = Nodes::load().unwrap();
        ctl.keep(Node {
            token: Some("tsr_b".to_string()),
            ..node("b", "10.0.0.6:7400")
        })
        .unwrap();
        stale.forget("a").unwrap();

        let now = Nodes::load().unwrap();
        assert!(now.by_id("a").is_none());
        assert_eq!(now.by_id("b").unwrap().token.as_deref(), Some("tsr_b"));
        assert_eq!(
            now.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
    }
}
