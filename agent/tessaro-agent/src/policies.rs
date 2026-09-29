//! The browser policies: one row per document in `browser_policies` of
//! `tessaro.db` (`db.rs`), set and removed with `tessaro-ctl browser
//! policies`, merged into the Chromium policy by `render.rs`.
//!
//! A document is stored as it was typed, so an editor gets its comments
//! back; `protocol::policy::check` decides what may be stored, and the
//! render checks each one again, leaving out one that no longer passes
//! (edited by hand, or a key the device took over) instead of failing.

use std::collections::BTreeMap;

use protocol::policy::{self, PolicyInfo, PolicyOverlap, POLICIES_MAX};
use serde_json::{Map, Value};
use tessaro_db::rusqlite::{self, params, Connection};

use crate::db::{self, Db, Stored};

/// Every stored document's text, by name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Policies {
    pub docs: BTreeMap<String, String>,
}

impl Stored for Policies {
    const WHAT: &'static str = "the browser policies";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare("SELECT name, text FROM browser_policies ORDER BY name")?;
        let docs = rows
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { docs })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM browser_policies", [])?;
        let mut insert = db.prepare("INSERT INTO browser_policies (name, text) VALUES (?1, ?2)")?;
        for (name, text) in &self.docs {
            insert.execute(params![name, text])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM browser_policies", []).map(drop)
    }
}

/// A stored document that passes the check, with its entries.
pub struct Checked {
    pub name: String,
    pub entries: Map<String, Value>,
}

/// The stored documents, or why they cannot be read: unlike `Db::read`, a
/// command must say so rather than answer as if there were none.
fn load(db: &Db) -> Result<Policies, String> {
    db.transaction(db::load::<Policies>)
}

/// The stored documents, sorted into those the render takes and those it
/// leaves out.
#[derive(Default)]
pub struct Loaded {
    /// Those that pass the check, ordered by name.
    pub checked: Vec<Checked>,
    /// Those that do not, by name, with the check's objection.
    pub refused: Vec<(String, String)>,
}

pub fn load_checked(db: &Db) -> Result<Loaded, String> {
    load(db).map(sorted)
}

/// `policies` sorted by the check.
pub fn sorted(policies: Policies) -> Loaded {
    let mut loaded = Loaded::default();
    for (name, text) in policies.docs {
        match policy::check(&text) {
            Ok(entries) => loaded.checked.push(Checked { name, entries }),
            Err(err) => loaded.refused.push((name, err.to_string())),
        }
    }
    loaded
}

pub fn list(db: &Db) -> Result<Vec<PolicyInfo>, String> {
    Ok(load(db)?
        .docs
        .into_iter()
        .map(|(name, text)| {
            let (keys, problem) = match policy::check(&text) {
                Ok(entries) => (entries.keys().cloned().collect(), None),
                Err(err) => (Vec::new(), Some(err.to_string())),
            };
            PolicyInfo {
                revision: policy::revision(&text),
                name,
                keys,
                problem,
            }
        })
        .collect())
}

fn unknown(name: &str) -> String {
    format!("no browser policy named {name}; see `tessaro-ctl browser policies list`")
}

/// One document's text.
pub fn read(db: &Db, name: &str) -> Result<String, String> {
    policy::check_name(name)?;
    load(db)?.docs.remove(name).ok_or_else(|| unknown(name))
}

/// Store `text` as `name`, when it passes the check and, with
/// `if_revision`, the stored one is still at that revision. Returns whether
/// anything was written.
pub fn store(db: &Db, name: &str, text: &str, if_revision: Option<&str>) -> Result<bool, String> {
    policy::check_name(name)?;
    policy::check(text).map_err(|err| err.to_string())?;
    db.update(|policies: &mut Policies| {
        let stored = policies.docs.get(name);
        match (if_revision, stored) {
            (Some(""), Some(_)) => {
                return Err(format!(
                    "a browser policy named {name} already exists; open it to change it"
                ))
            }
            (Some(expected), Some(stored))
                if !expected.is_empty() && policy::revision(stored) != expected =>
            {
                return Err(format!(
                    "{name} changed on the device since it was opened; reload it and edit again"
                ))
            }
            (Some(expected), None) if !expected.is_empty() => {
                return Err(format!(
                    "{name} was removed on the device since it was opened"
                ))
            }
            _ => {}
        }
        if stored.is_some_and(|stored| stored == text) {
            return Ok(false);
        }
        if stored.is_none() && policies.docs.len() >= POLICIES_MAX {
            return Err(format!(
                "the device holds at most {POLICIES_MAX} browser policies; \
                 remove one with `tessaro-ctl browser policies remove`"
            ));
        }
        policies.docs.insert(name.to_string(), text.to_string());
        Ok(true)
    })
}

pub fn remove(db: &Db, name: &str) -> Result<(), String> {
    policy::check_name(name)?;
    db.update(|policies: &mut Policies| {
        policies
            .docs
            .remove(name)
            .map(drop)
            .ok_or_else(|| unknown(name))
    })
}

/// Remove every document: a factory reset.
pub fn clear(db: &Db) -> Result<(), String> {
    db.clear::<Policies>()
}

/// The keys `name` shares with the other documents: those it takes from one
/// earlier by name, and those one later by name takes from it.
pub fn overlaps(docs: &[Checked], name: &str) -> (Vec<PolicyOverlap>, Vec<PolicyOverlap>) {
    let mut shadows = Vec::new();
    let mut shadowed_by = Vec::new();
    let Some(own) = docs.iter().find(|doc| doc.name == name) else {
        return (shadows, shadowed_by);
    };
    for key in own.entries.keys() {
        for other in docs.iter().filter(|doc| doc.name != name) {
            if !other.entries.contains_key(key) {
                continue;
            }
            let overlap = PolicyOverlap {
                key: key.clone(),
                policy: other.name.clone(),
            };
            if other.name.as_str() < name {
                shadows.push(overlap);
            } else {
                shadowed_by.push(overlap);
            }
        }
    }
    (shadows, shadowed_by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;

    fn fixture() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &Log::buffered(true));
        (dir, db)
    }

    /// A document written straight into the table, past the check, as the
    /// sqlite3 shell could.
    fn by_hand(db: &Db, name: &str, text: &str) {
        db.update(|policies: &mut Policies| {
            policies.docs.insert(name.to_string(), text.to_string());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn stored_as_typed_and_listed_by_name() {
        let (_dir, db) = fixture();
        let text = "// keep this\n{\"PrintingEnabled\": false,}\n";
        assert!(store(&db, "zeta", "{\"A\": 1}", None).unwrap());
        assert!(store(&db, "lockdown", text, None).unwrap());
        assert!(!store(&db, "lockdown", text, None).unwrap());

        assert_eq!(read(&db, "lockdown").unwrap(), text);
        let listed = list(&db).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|doc| doc.name.as_str())
                .collect::<Vec<_>>(),
            ["lockdown", "zeta"]
        );
        assert_eq!(listed[0].keys, ["PrintingEnabled"]);
        assert_eq!(listed[0].revision, policy::revision(text));
    }

    #[test]
    fn a_save_from_an_old_revision_is_refused() {
        let (_dir, db) = fixture();
        store(&db, "a", "{\"A\": 1}", Some("")).unwrap();
        let opened = policy::revision("{\"A\": 1}");
        assert!(store(&db, "a", "{\"A\": 2}", Some("")).is_err());
        store(&db, "a", "{\"A\": 2}", Some(&opened)).unwrap();
        let err = store(&db, "a", "{\"A\": 3}", Some(&opened)).unwrap_err();
        assert!(err.contains("changed on the device"), "{err}");
        remove(&db, "a").unwrap();
        assert!(store(&db, "a", "{\"A\": 3}", Some(&opened)).is_err());
    }

    #[test]
    fn what_the_check_refuses_is_never_stored() {
        let (_dir, db) = fixture();
        assert!(store(&db, "a", "{\"CACertificates\": []}", None).is_err());
        assert!(store(&db, "a", "{", None).is_err());
        assert!(store(&db, "../a", "{}", None).is_err());
        assert!(list(&db).unwrap().is_empty());
        assert!(remove(&db, "a").unwrap_err().contains("no browser policy"));
    }

    #[test]
    fn a_stored_document_that_no_longer_passes_is_left_out() {
        let (_dir, db) = fixture();
        store(&db, "good", "{\"A\": 1}", None).unwrap();
        by_hand(&db, "bad", "{\"ProxyMode\": \"direct\"}");
        let loaded = load_checked(&db).unwrap();
        assert_eq!(loaded.checked.len(), 1);
        assert_eq!(loaded.refused[0].0, "bad");
        assert!(list(&db).unwrap()[0].problem.is_some());
    }

    #[test]
    fn a_factory_reset_empties_the_table() {
        let (_dir, db) = fixture();
        store(&db, "a", "{\"A\": 1}", None).unwrap();
        clear(&db).unwrap();
        assert!(list(&db).unwrap().is_empty());
    }

    #[test]
    fn overlaps_are_named_from_both_sides() {
        let (_dir, db) = fixture();
        store(&db, "a", "{\"K\": 1, \"A\": 1}", None).unwrap();
        store(&db, "b", "{\"K\": 2}", None).unwrap();
        store(&db, "c", "{\"K\": 3}", None).unwrap();
        let docs = load_checked(&db).unwrap().checked;
        let (shadows, shadowed_by) = overlaps(&docs, "b");
        assert_eq!(
            shadows,
            [PolicyOverlap {
                key: "K".into(),
                policy: "a".into()
            }]
        );
        assert_eq!(
            shadowed_by,
            [PolicyOverlap {
                key: "K".into(),
                policy: "c".into()
            }]
        );
    }
}
