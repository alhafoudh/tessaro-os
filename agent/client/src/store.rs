//! `tessaro.db` in the config directory (`nodes::dir()`): what this client
//! keeps between runs. The known nodes (`nodes.rs`) and tessaro-gui's
//! window and table preferences, one SQLite store opened through
//! `tessaro-db` (WAL, `synchronous=FULL`, migrations, a broken file set
//! aside). The schema is the migrations in `client/migrations/`.
//!
//! tessaro-ctl and tessaro-gui, and the GUI's threads, each open their own
//! connection per operation and write single rows, so one never overwrites
//! what another just stored. The file holds tokens: it is 0600.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tessaro_db::rusqlite::{params, Connection};

use crate::nodes;

pub const FILE: &str = "tessaro.db";

mod embedded {
    refinery::embed_migrations!("migrations");
}

/// Where this client's store is.
pub fn path() -> PathBuf {
    nodes::dir().join(FILE)
}

/// This client's store, opened and migrated, made if it is not there: for
/// a write.
pub fn open() -> Result<Connection, String> {
    open_at(&path())
}

/// This client's store if there is one: for a read, which must never make
/// the file or its directory.
pub fn open_existing() -> Result<Option<Connection>, String> {
    let path = path();
    if !path.exists() {
        return Ok(None);
    }
    open_at(&path).map(Some)
}

pub(crate) fn open_at(path: &Path) -> Result<Connection, String> {
    tessaro_db::open(path, embedded::migrations::runner()).map(|opened| opened.connection)
}

/// tessaro-gui's preferences, by name, each a JSON value. Empty when there
/// are none or the store cannot be read: preferences are never worth an
/// error.
pub fn gui_prefs() -> BTreeMap<String, String> {
    gui_prefs_at(&path())
}

/// Replace tessaro-gui's preferences with `prefs`, in one transaction.
pub fn save_gui_prefs(prefs: &BTreeMap<String, String>) -> Result<(), String> {
    save_gui_prefs_at(&path(), prefs)
}

fn gui_prefs_at(path: &Path) -> BTreeMap<String, String> {
    if !path.exists() {
        return BTreeMap::new();
    }
    let read = || -> Result<BTreeMap<String, String>, String> {
        let db = open_at(path)?;
        let mut rows = db
            .prepare("SELECT key, value FROM gui_prefs")
            .map_err(|err| err.to_string())?;
        let prefs = rows
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| err.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|err| err.to_string())?;
        Ok(prefs)
    };
    read().unwrap_or_default()
}

fn save_gui_prefs_at(path: &Path, prefs: &BTreeMap<String, String>) -> Result<(), String> {
    let fail = |err: tessaro_db::rusqlite::Error| format!("{}: {err}", path.display());
    let mut db = open_at(path)?;
    let tx = db.transaction().map_err(fail)?;
    tx.execute("DELETE FROM gui_prefs", []).map_err(fail)?;
    for (key, value) in prefs {
        tx.execute(
            "INSERT INTO gui_prefs (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(fail)?;
    }
    tx.commit().map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_prefs_are_given_back_as_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(gui_prefs_at(&path).is_empty());

        let prefs = BTreeMap::from([
            ("scale".to_string(), "1.25".to_string()),
            ("table:nodes".to_string(), r#"{"sort":"name"}"#.to_string()),
        ]);
        save_gui_prefs_at(&path, &prefs).unwrap();
        assert_eq!(gui_prefs_at(&path), prefs);

        save_gui_prefs_at(&path, &BTreeMap::new()).unwrap();
        assert!(gui_prefs_at(&path).is_empty());
    }
}
