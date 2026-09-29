//! Opening one of Tessaro's SQLite stores: the device's
//! `/data/tessaro/tessaro.db`, its `/run/tessaro-kiosk/sessions.db`, and a
//! client's `tessaro.db` next to `known_hosts`.
//!
//! * **Durable commits.** `journal_mode=WAL` with `synchronous=FULL`: a kiosk
//!   loses power as a matter of routine, and a commit that returned must still
//!   be there after the cut. WAL is what lets a reader run beside a writer.
//! * **Waits are bounded.** `busy_timeout` is 5 s, well inside the agent's
//!   20 s `deadline::blocking`, so a writer in another process (the boot
//!   oneshot, a `sqlite3` shell) delays a call instead of failing it.
//! * **A connection per operation.** `connect` is cheap, so nothing shares a
//!   `Connection` across threads and nothing holds one across an await.
//! * **Migrations run at `open`**, once per process, from files compiled
//!   into the binary (`embed_migrations!` in the crate that owns the store).
//!   An applied migration is never edited: refinery checksums it and refuses.
//! * **A broken file is set aside, not fatal.** A file that is not a
//!   database, or fails `quick_check`, is renamed to
//!   `<name>.corrupt-<unix time>` with its `-wal` and `-shm`, and an empty
//!   store takes its place. A torn store must never stop the kiosk from
//!   coming up; the worst outcome is the defaults.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use refinery;
pub use rusqlite;

use rusqlite::{Connection, ErrorCode};

/// Every store keeps its applied migrations here, Rails' name for it.
pub const MIGRATIONS_TABLE: &str = "schema_migrations";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A store opened and migrated. `quarantined` is where a broken file went,
/// for the caller to log.
pub struct Opened {
    pub connection: Connection,
    pub quarantined: Option<PathBuf>,
}

/// Open `path`, creating it 0600 when missing, set a broken file aside, and
/// apply `runner`'s pending migrations.
pub fn open(path: &Path, mut runner: refinery::Runner) -> Result<Opened, String> {
    runner.set_migration_table_name(MIGRATIONS_TABLE);
    let (mut connection, quarantined) = match checked(path) {
        Ok(connection) => (connection, None),
        Err(Broken::Corrupt(why)) => {
            let aside = quarantine(path).map_err(|err| {
                format!(
                    "{} is broken ({why}) and cannot be moved aside: {err}",
                    path.display()
                )
            })?;
            let connection = checked(path).map_err(|err| err.to_string())?;
            (connection, Some(aside))
        }
        Err(Broken::Other(why)) => return Err(why),
    };
    runner
        .run(&mut connection)
        .map_err(|err| format!("{}: migrations: {err}", path.display()))?;
    Ok(Opened {
        connection,
        quarantined,
    })
}

/// A connection to a store `open` has already migrated: the per-connection
/// pragmas and nothing else.
pub fn connect(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    configure(&connection).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(connection)
}

enum Broken {
    Corrupt(String),
    Other(String),
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Broken::Corrupt(why) | Broken::Other(why) => f.write_str(why),
        }
    }
}

fn checked(path: &Path) -> Result<Connection, Broken> {
    create_private(path).map_err(|err| Broken::Other(format!("{}: {err}", path.display())))?;
    let classify = |err: rusqlite::Error| match err.sqlite_error_code() {
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => {
            Broken::Corrupt(err.to_string())
        }
        _ => Broken::Other(format!("{}: {err}", path.display())),
    };
    let connection = Connection::open(path).map_err(classify)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(classify)?;
    configure(&connection).map_err(classify)?;
    let verdict: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(classify)?;
    if verdict != "ok" {
        return Err(Broken::Corrupt(verdict));
    }
    Ok(connection)
}

fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", "ON")
}

/// The file itself, 0600, before SQLite opens it: SQLite gives its `-wal`
/// and `-shm` the database's mode, so they are private too.
fn create_private(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(drop)
}

fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let aside = sibling(path, &format!(".corrupt-{stamp}"));
    for suffix in ["-wal", "-shm"] {
        let from = sibling(path, suffix);
        if from.exists() {
            fs::rename(&from, sibling(&aside, suffix))?;
        }
    }
    fs::rename(path, &aside)?;
    Ok(aside)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod embedded {
        refinery::embed_migrations!("tests/migrations");
    }

    fn open_at(path: &Path) -> Opened {
        open(path, embedded::migrations::runner()).unwrap()
    }

    #[test]
    fn migrations_apply_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");

        let first = open_at(&path);
        first
            .connection
            .execute("INSERT INTO things (name) VALUES ('a')", [])
            .unwrap();
        drop(first);
        let again = open_at(&path);

        let applied: i64 = again
            .connection
            .query_row(
                &format!("SELECT count(*) FROM {MIGRATIONS_TABLE}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
        let things: i64 = again
            .connection
            .query_row("SELECT count(*) FROM things", [], |row| row.get(0))
            .unwrap();
        assert_eq!(things, 1);
        assert!(again.quarantined.is_none());
    }

    #[test]
    fn the_store_is_durable_and_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let opened = open_at(&dir.path().join("store.db"));
        let mode: String = opened
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let synchronous: i64 = opened
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(synchronous, 2, "FULL");
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        drop(open_at(&path));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_broken_file_is_set_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        fs::write(&path, vec![0x5a; 8192]).unwrap();

        let opened = open_at(&path);

        let aside = opened.quarantined.clone().expect("quarantined");
        assert_eq!(fs::read(&aside).unwrap(), vec![0x5a; 8192]);
        let things: i64 = opened
            .connection
            .query_row("SELECT count(*) FROM things", [], |row| row.get(0))
            .unwrap();
        assert_eq!(things, 0);
    }

    #[test]
    fn connect_sees_what_open_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        drop(open_at(&path));

        connect(&path)
            .unwrap()
            .execute("INSERT INTO things (name) VALUES ('b')", [])
            .unwrap();
        let name: String = connect(&path)
            .unwrap()
            .query_row("SELECT name FROM things", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "b");
    }
}
