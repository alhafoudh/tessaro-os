//! `/data/tessaro/tessaro.db`: everything the device keeps about itself.
//!
//! The settings, the tokens, the network passwords, the schedules, the
//! browser policies and the network transaction's record, as tables of one
//! SQLite store opened
//! through `tessaro-db` (WAL, `synchronous=FULL`, migrations, a broken file
//! set aside). The schema is the migrations in `migrations/device/`; the
//! `sqlite3` CLI on the image reads and edits it by hand.
//!
//! * A type kept here is `Stored`: it loads itself from its tables and saves
//!   itself back, whole, inside the transaction it is given.
//! * **`read` never fails**: whatever goes wrong is logged and the default
//!   comes back, so a broken store cannot keep the kiosk from coming up.
//! * **`update` is read, change, write in one `BEGIN IMMEDIATE`**, so the
//!   agent, the boot oneshot and a factory reset never interleave. A change
//!   that returns an error writes nothing.
//! * `transaction` is the same for a change that spans several types, such as
//!   the network commit, which writes a password and the settings together.
//!
//! Everything here is blocking. From the runtime, call it through
//! `deadline::blocking`.

use std::path::{Path, PathBuf};

use tessaro_db::rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::log::Log;

pub const FILE: &str = "tessaro.db";

mod embedded {
    refinery::embed_migrations!("migrations/device");
}

/// A type with tables of its own in the store.
pub trait Stored: Default + Sized {
    /// What it is, for the journal: "the settings".
    const WHAT: &'static str;
    fn load(db: &Connection) -> tessaro_db::rusqlite::Result<Self>;
    /// Replace what the tables hold with `self`.
    fn save(&self, db: &Connection) -> tessaro_db::rusqlite::Result<()>;
    /// Empty the tables: a factory reset.
    fn clear(db: &Connection) -> tessaro_db::rusqlite::Result<()>;
}

#[derive(Debug, Clone)]
pub struct Db {
    path: PathBuf,
}

impl Db {
    /// The store in `dir`, created and migrated. Failing to open it is
    /// logged, not returned: every read then gives the defaults, and every
    /// change fails with the reason.
    pub fn open(dir: &Path, log: &Log) -> Self {
        let db = Self::at(dir);
        match tessaro_db::open(&db.path, embedded::migrations::runner()) {
            Ok(opened) => {
                if let Some(aside) = opened.quarantined {
                    log.info(format!(
                        "{} was broken: moved to {}, starting from defaults",
                        db.path.display(),
                        aside.display()
                    ));
                }
            }
            Err(err) => log.info(format!("cannot open the store: {err}")),
        }
        db
    }

    /// The store in `dir`, already opened by this process.
    pub fn at(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
        }
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection, String> {
        tessaro_db::connect(&self.path)
    }

    pub fn read<T: Stored>(&self, log: &Log) -> T {
        let loaded = self.connect().and_then(|mut db| {
            let tx = db.transaction().map_err(|err| err.to_string())?;
            T::load(&tx).map_err(|err| err.to_string())
        });
        loaded.unwrap_or_else(|err| {
            log.info(format!(
                "cannot read {} ({err}); using the defaults",
                T::WHAT
            ));
            T::default()
        })
    }

    /// Read, change, write, in one transaction. `change` returning an error
    /// leaves the store untouched.
    pub fn update<T: Stored, R>(
        &self,
        change: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<R, String> {
        self.transaction(|tx| {
            let mut value = load::<T>(tx)?;
            let out = change(&mut value)?;
            save(tx, &value)?;
            Ok(out)
        })
    }

    /// `work` inside one write transaction, committed only when it returns
    /// `Ok`.
    pub fn transaction<R>(
        &self,
        work: impl FnOnce(&Transaction) -> Result<R, String>,
    ) -> Result<R, String> {
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| format!("{}: {err}", self.path.display()))?;
        let out = work(&tx)?;
        tx.commit()
            .map_err(|err| format!("{}: {err}", self.path.display()))?;
        Ok(out)
    }

    /// Empty `T`'s tables - a factory reset.
    pub fn clear<T: Stored>(&self) -> Result<(), String> {
        self.transaction(|tx| T::clear(tx).map_err(|err| format!("clearing {}: {err}", T::WHAT)))
    }
}

/// `T` as the transaction sees it.
pub fn load<T: Stored>(tx: &Transaction) -> Result<T, String> {
    T::load(tx).map_err(|err| format!("reading {}: {err}", T::WHAT))
}

pub fn save<T: Stored>(tx: &Transaction, value: &T) -> Result<(), String> {
    value
        .save(tx)
        .map_err(|err| format!("writing {}: {err}", T::WHAT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Auth;
    use crate::secrets::Secrets;
    use crate::state::State;
    use protocol::Secret;

    fn fixture() -> (tempfile::TempDir, Db, Log) {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::buffered(true);
        let db = Db::open(dir.path(), &log);
        (dir, db, log)
    }

    #[test]
    fn an_empty_store_reads_as_the_defaults() {
        let (_dir, db, log) = fixture();
        assert_eq!(db.read::<State>(&log), State::default());
        assert_eq!(db.read::<Auth>(&log), Auth::default());
        assert!(log.lines().is_empty(), "{:?}", log.lines());
    }

    #[test]
    fn an_update_is_there_for_the_next_read() {
        let (_dir, db, log) = fixture();
        db.update(|state: &mut State| {
            state
                .settings
                .insert("browser.url".into(), "https://a.test/".into());
            state.revision += 1;
            Ok(())
        })
        .unwrap();

        let state: State = Db::at(db.path().parent().unwrap()).read(&log);
        assert_eq!(state.settings["browser.url"], "https://a.test/");
        assert_eq!(state.revision, 1);
    }

    #[test]
    fn a_failed_change_writes_nothing() {
        let (_dir, db, log) = fixture();
        db.update(|state: &mut State| {
            state.settings.insert("screen.scale".into(), "2".into());
            Ok(())
        })
        .unwrap();

        let err = db
            .update(|state: &mut State| -> Result<(), String> {
                state.settings.clear();
                Err("refused".into())
            })
            .unwrap_err();

        assert_eq!(err, "refused");
        assert_eq!(db.read::<State>(&log).settings["screen.scale"], "2");
    }

    #[test]
    fn a_transaction_keeps_all_of_its_writes_or_none() {
        let (_dir, db, log) = fixture();
        let outcome: Result<(), String> = db.transaction(|tx| {
            let mut secrets = load::<Secrets>(tx)?;
            secrets.wifi_psk = Some(Secret("hunter2hunter2".into()));
            save(tx, &secrets)?;
            Err("the settings could not be saved".into())
        });

        assert!(outcome.is_err());
        assert_eq!(db.read::<Secrets>(&log), Secrets::default());
    }

    #[test]
    fn concurrent_updates_lose_nothing() {
        let (_dir, db, log) = fixture();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        db.update(|state: &mut State| {
                            state.revision += 1;
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(db.read::<State>(&log).revision, 200);
    }

    #[test]
    fn clearing_empties_only_that_type() {
        let (_dir, db, log) = fixture();
        db.update(|state: &mut State| {
            state.settings.insert("screen.osk".into(), "never".into());
            Ok(())
        })
        .unwrap();
        db.update(|auth: &mut Auth| auth.issue("laptop", "claim").map(|_| ()))
            .unwrap();

        db.clear::<State>().unwrap();

        assert!(db.read::<State>(&log).settings.is_empty());
        assert!(db.read::<Auth>(&log).claimed());
    }

    #[test]
    fn a_broken_store_is_set_aside_and_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), vec![0x5a; 8192]).unwrap();
        let log = Log::buffered(true);

        let db = Db::open(dir.path(), &log);

        assert_eq!(db.read::<State>(&log), State::default());
        assert!(
            log.lines().iter().any(|line| line.contains("was broken")),
            "{:?}",
            log.lines()
        );
    }
}
