//! The devices this client knows: `~/.config/tessaro/nodes.json`.
//!
//! Keyed by **node id**, not by address or name. DHCP moves a device, an
//! operator renames it; neither makes it a different device, so neither may
//! break its pin. The file holds tokens, so it is written 0600 and replaced
//! atomically.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub name: String,
    /// Last address it answered on, `ip:port`.
    pub address: String,
    /// SHA-256 of its TLS certificate, pinned on first use.
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Nodes {
    #[serde(default)]
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

impl Nodes {
    pub fn load() -> Result<Self, String> {
        let path = dir().join("nodes.json");
        match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!("{}: {err}", path.display())),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = dir();
        fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let path = dir.join("nodes.json");
        let temporary = dir.join(".nodes.json.tmp");

        let mut body = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        body.push(b'\n');

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
        file.write_all(&body)
            .and_then(|()| file.sync_all())
            .map_err(|err| format!("{}: {err}", temporary.display()))?;
        fs::rename(&temporary, &path).map_err(|err| format!("{}: {err}", path.display()))
    }

    pub fn by_id(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.name == name)
    }

    pub fn by_address(&self, address: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.address == address)
    }

    /// Insert or replace by id.
    pub fn put(&mut self, node: Node) {
        match self.nodes.iter_mut().find(|known| known.id == node.id) {
            Some(known) => *known = node,
            None => self.nodes.push(node),
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|node| node.id != id);
        self.nodes.len() != before
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
    fn saved_and_loaded_privately() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this crate that touch the environment run here only.
        unsafe { std::env::set_var("TESSARO_CONFIG_DIR", dir.path()) };

        let mut nodes = Nodes::default();
        nodes.put(Node {
            token: Some("tsr_x".to_string()),
            ..node("a", "10.0.0.5:7400")
        });
        nodes.save().unwrap();

        let loaded = Nodes::load().unwrap();
        assert_eq!(
            loaded.by_name("name-a").unwrap().token.as_deref(),
            Some("tsr_x")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("nodes.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
