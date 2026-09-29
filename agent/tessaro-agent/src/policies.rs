//! The browser policies: one row per document in `browser_policies` of
//! `tessaro.db` (`db.rs`), set, moved and removed with `tessaro-ctl browser
//! policies`, merged into the Chromium policy by `render.rs`.
//!
//! A document is stored as it was typed, so an editor gets its comments
//! back; `protocol::policy::check` decides what may be stored, and the
//! render checks each one again, leaving out one that no longer passes
//! (edited by hand, or a key the device took over) instead of failing.
//!
//! **The documents are in priority order**, position 1 first: it wins a key
//! others set too. Positions are always 1 to the number of documents, with
//! no gaps: a new one goes to the bottom, a move shifts the ones between,
//! a removal closes the gap.

use protocol::policy::{self, PolicyInfo, PolicyOverlap, POLICIES_MAX};
use serde_json::{Map, Value};
use tessaro_db::rusqlite::{self, params, Connection};

use crate::db::{self, Db, Stored};

/// Every stored document, name and text, in priority order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Policies {
    pub docs: Vec<(String, String)>,
}

impl Policies {
    fn at(&self, name: &str) -> Option<usize> {
        self.docs.iter().position(|(known, _)| known == name)
    }

    /// Move the document at `from` to `position` (from 1), held to the
    /// ends. Returns its position.
    fn place(&mut self, from: usize, position: u32) -> u32 {
        let to = (position.max(1) as usize - 1).min(self.docs.len() - 1);
        let doc = self.docs.remove(from);
        self.docs.insert(to, doc);
        to as u32 + 1
    }
}

impl Stored for Policies {
    const WHAT: &'static str = "the browser policies";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows =
            db.prepare("SELECT name, text FROM browser_policies ORDER BY position, name")?;
        let docs = rows
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { docs })
    }

    /// Positions are written 1..N from the order, so a hand edit that left
    /// gaps or ties is made whole by the next change.
    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM browser_policies", [])?;
        let mut insert =
            db.prepare("INSERT INTO browser_policies (name, text, position) VALUES (?1, ?2, ?3)")?;
        for (at, (name, text)) in self.docs.iter().enumerate() {
            insert.execute(params![name, text, at as i64 + 1])?;
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
    pub position: u32,
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
    /// Those that pass the check, in priority order.
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
    for (at, (name, text)) in policies.docs.into_iter().enumerate() {
        match policy::check(&text) {
            Ok(entries) => loaded.checked.push(Checked {
                name,
                position: at as u32 + 1,
                entries,
            }),
            Err(err) => loaded.refused.push((name, err.to_string())),
        }
    }
    loaded
}

pub fn list(db: &Db) -> Result<Vec<PolicyInfo>, String> {
    Ok(load(db)?
        .docs
        .into_iter()
        .enumerate()
        .map(|(at, (name, text))| {
            let (keys, problem) = match policy::check(&text) {
                Ok(entries) => (policy::keys(&entries), None),
                Err(err) => (Vec::new(), Some(err.to_string())),
            };
            PolicyInfo {
                revision: policy::revision(&text),
                name,
                position: at as u32 + 1,
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
    load(db)?
        .docs
        .into_iter()
        .find(|(known, _)| known == name)
        .map(|(_, text)| text)
        .ok_or_else(|| unknown(name))
}

/// What storing a document did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    /// The text or the position changed.
    pub changed: bool,
    pub position: u32,
}

/// Store `text` as `name`, when it passes the check and, with
/// `if_revision`, the stored one is still at that revision. A new one goes
/// to the bottom unless `position` places it; a stored one keeps its place
/// unless `position` moves it.
pub fn store(
    db: &Db,
    name: &str,
    text: &str,
    if_revision: Option<&str>,
    position: Option<u32>,
) -> Result<Saved, String> {
    policy::check_name(name)?;
    policy::check(text).map_err(|err| err.to_string())?;
    db.update(|policies: &mut Policies| {
        let at = policies.at(name);
        let stored = at.map(|at| &policies.docs[at].1);
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
        let (at, same_text) = match at {
            Some(at) => {
                let same = policies.docs[at].1 == text;
                policies.docs[at].1 = text.to_string();
                (at, same)
            }
            None if policies.docs.len() >= POLICIES_MAX => {
                return Err(format!(
                    "the device holds at most {POLICIES_MAX} browser policies; \
                     remove one with `tessaro-ctl browser policies remove`"
                ))
            }
            None => {
                policies.docs.push((name.to_string(), text.to_string()));
                (policies.docs.len() - 1, false)
            }
        };
        let now = match position {
            Some(position) => policies.place(at, position),
            None => at as u32 + 1,
        };
        Ok(Saved {
            changed: !same_text || now != at as u32 + 1,
            position: now,
        })
    })
}

/// Move `name` to `position` (from 1, held to the ends), the others
/// shifting to make room. Returns where it is now and whether it moved.
pub fn move_to(db: &Db, name: &str, position: u32) -> Result<(u32, bool), String> {
    policy::check_name(name)?;
    db.update(|policies: &mut Policies| {
        let at = policies.at(name).ok_or_else(|| unknown(name))?;
        let now = policies.place(at, position);
        Ok((now, now != at as u32 + 1))
    })
}

pub fn remove(db: &Db, name: &str) -> Result<(), String> {
    policy::check_name(name)?;
    db.update(|policies: &mut Policies| {
        let at = policies.at(name).ok_or_else(|| unknown(name))?;
        policies.docs.remove(at);
        Ok(())
    })
}

/// Remove every document: a factory reset.
pub fn clear(db: &Db) -> Result<(), String> {
    db.clear::<Policies>()
}

/// The keys `name` shares with the other documents: those it wins over one
/// lower in priority, and those one higher in priority wins over it.
pub fn overlaps(docs: &[Checked], name: &str) -> (Vec<PolicyOverlap>, Vec<PolicyOverlap>) {
    let mut shadows = Vec::new();
    let mut shadowed_by = Vec::new();
    let Some(own) = docs.iter().find(|doc| doc.name == name) else {
        return (shadows, shadowed_by);
    };
    for key in policy::keys(&own.entries) {
        for other in docs.iter().filter(|doc| doc.name != name) {
            if !other.entries.contains_key(&key) {
                continue;
            }
            let overlap = PolicyOverlap {
                key: key.clone(),
                policy: other.name.clone(),
            };
            if other.position > own.position {
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
            policies.docs.push((name.to_string(), text.to_string()));
            Ok(())
        })
        .unwrap();
    }

    fn set(db: &Db, name: &str, text: &str) -> Saved {
        store(db, name, text, None, None).unwrap()
    }

    fn order(db: &Db) -> Vec<String> {
        list(db).unwrap().into_iter().map(|doc| doc.name).collect()
    }

    #[test]
    fn stored_as_typed_new_ones_at_the_bottom() {
        let (_dir, db) = fixture();
        let text = "// keep this\n{\"SpellcheckEnabled\": false,}\n";
        assert_eq!(
            set(&db, "zeta", "{\"A\": 1}"),
            Saved {
                changed: true,
                position: 1
            }
        );
        assert_eq!(set(&db, "lockdown", text).position, 2);
        assert!(!set(&db, "lockdown", text).changed);

        assert_eq!(read(&db, "lockdown").unwrap(), text);
        let listed = list(&db).unwrap();
        assert_eq!(order(&db), ["zeta", "lockdown"]);
        assert_eq!(
            (listed[1].position, &listed[1].keys[..]),
            (2, &["SpellcheckEnabled".to_string()][..])
        );
        assert_eq!(listed[1].revision, policy::revision(text));
    }

    #[test]
    fn keys_are_listed_sorted() {
        let (_dir, db) = fixture();
        set(&db, "a", "{\"Zed\": 1, \"Alpha\": 2, \"Mid\": 3}");
        assert_eq!(list(&db).unwrap()[0].keys, ["Alpha", "Mid", "Zed"]);
    }

    #[test]
    fn a_move_shifts_the_others_and_is_held_to_the_ends() {
        let (_dir, db) = fixture();
        for name in ["a", "b", "c", "d"] {
            set(&db, name, "{}");
        }
        assert_eq!(move_to(&db, "d", 1).unwrap(), (1, true));
        assert_eq!(order(&db), ["d", "a", "b", "c"]);
        assert_eq!(move_to(&db, "d", 99).unwrap(), (4, true));
        assert_eq!(order(&db), ["a", "b", "c", "d"]);
        assert_eq!(move_to(&db, "b", 0).unwrap(), (1, true));
        assert_eq!(move_to(&db, "b", 1).unwrap(), (1, false));
        assert!(move_to(&db, "nope", 1).is_err());

        remove(&db, "a").unwrap();
        let positions: Vec<u32> = list(&db).unwrap().iter().map(|doc| doc.position).collect();
        assert_eq!(positions, [1, 2, 3], "no gap after a removal");

        // A save can place it too, new or stored.
        assert_eq!(store(&db, "e", "{}", None, Some(1)).unwrap().position, 1);
        assert_eq!(
            store(&db, "e", "{}", None, Some(3)).unwrap(),
            Saved {
                changed: true,
                position: 3
            }
        );
        assert_eq!(order(&db), ["b", "c", "e", "d"]);
    }

    #[test]
    fn a_save_from_an_old_revision_is_refused() {
        let (_dir, db) = fixture();
        store(&db, "a", "{\"A\": 1}", Some(""), None).unwrap();
        let opened = policy::revision("{\"A\": 1}");
        assert!(store(&db, "a", "{\"A\": 2}", Some(""), None).is_err());
        store(&db, "a", "{\"A\": 2}", Some(&opened), None).unwrap();
        let err = store(&db, "a", "{\"A\": 3}", Some(&opened), None).unwrap_err();
        assert!(err.contains("changed on the device"), "{err}");
        remove(&db, "a").unwrap();
        assert!(store(&db, "a", "{\"A\": 3}", Some(&opened), None).is_err());
    }

    #[test]
    fn what_the_check_refuses_is_never_stored() {
        let (_dir, db) = fixture();
        assert!(store(&db, "a", "{\"CACertificates\": []}", None, None).is_err());
        assert!(store(&db, "a", "{", None, None).is_err());
        assert!(store(&db, "../a", "{}", None, None).is_err());
        assert!(list(&db).unwrap().is_empty());
        assert!(remove(&db, "a").unwrap_err().contains("no browser policy"));
    }

    #[test]
    fn a_stored_document_that_no_longer_passes_is_left_out() {
        let (_dir, db) = fixture();
        set(&db, "good", "{\"A\": 1}");
        by_hand(&db, "bad", "{\"ProxyMode\": \"direct\"}");
        let loaded = load_checked(&db).unwrap();
        assert_eq!(loaded.checked.len(), 1);
        assert_eq!(loaded.refused[0].0, "bad");
        assert!(list(&db).unwrap()[1].problem.is_some());
    }

    #[test]
    fn a_factory_reset_empties_the_table() {
        let (_dir, db) = fixture();
        set(&db, "a", "{\"A\": 1}");
        clear(&db).unwrap();
        assert!(list(&db).unwrap().is_empty());
    }

    #[test]
    fn overlaps_are_named_from_both_sides() {
        let (_dir, db) = fixture();
        set(&db, "a", "{\"K\": 1, \"A\": 1}");
        set(&db, "b", "{\"K\": 2}");
        set(&db, "c", "{\"K\": 3}");
        let docs = load_checked(&db).unwrap().checked;
        let (shadows, shadowed_by) = overlaps(&docs, "b");
        let overlap = |policy: &str| PolicyOverlap {
            key: "K".into(),
            policy: policy.into(),
        };
        // a is position 1, above b; c is below it.
        assert_eq!(shadows, [overlap("c")]);
        assert_eq!(shadowed_by, [overlap("a")]);
    }

    #[test]
    fn the_migration_keeps_which_policy_wins() {
        // Before positions, the last name won: it becomes position 1.
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &Log::buffered(true));
        db.transaction(|tx| {
            tx.execute_batch(
                "DELETE FROM browser_policies;
                 INSERT INTO browser_policies (name, text, position) VALUES
                   ('alpha', '{}', 0), ('zulu', '{}', 0), ('mike', '{}', 0);",
            )
            .map_err(|err| err.to_string())?;
            tx.execute_batch(
                include_str!("../migrations/device/V20260929180000__order_browser_policies.sql")
                    .split_once("ADD COLUMN position INTEGER NOT NULL DEFAULT 0;")
                    .map(|(_, numbering)| numbering)
                    .unwrap(),
            )
            .map_err(|err| err.to_string())
        })
        .unwrap();
        assert_eq!(order(&db), ["zulu", "mike", "alpha"]);
    }
}
