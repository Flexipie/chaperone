//! The ledger file: one SQLite database in WAL mode, versioned by
//! `PRAGMA user_version`. Agents open it with [`Ledger::open`], which never
//! creates or migrates. Only `seamlane migrate` calls
//! [`Ledger::open_and_migrate`].

mod error;
mod event;
mod fold;
mod migrate;
mod write;

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub use error::LedgerError;
pub use event::{
    Actor, Applied, Change, Event, EventKind, Finding, Findings, Mutation, Payload, Refusal,
};
use migrate::user_version;
pub use write::{KINDS, PR_FLOWS, SOURCES, STATUSES};

pub(crate) const MIGRATIONS: &[&str] = &[include_str!("../../migrations/0001_ledger.sql")];

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// `PRAGMA application_id`, set by 0001: "SEAM".
pub const APPLICATION_ID: i64 = 0x5345_414D;

pub struct Ledger {
    pub(crate) conn: Connection,
}

impl Ledger {
    /// Opens an existing ledger at this binary's schema version. A missing
    /// file or any other version is refused, and a newer file is not written.
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        let probed = probe(path)?.ok_or_else(|| LedgerError::Missing(path.into()))?;
        if foreign(probed) {
            return Err(LedgerError::NotALedger(path.into()));
        }
        let found = probed.0;
        if found != SCHEMA_VERSION {
            return Err(LedgerError::SchemaMismatch {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        let flags = OpenFlags::default().difference(OpenFlags::SQLITE_OPEN_CREATE);
        let conn = Connection::open_with_flags(path, flags)?;
        apply_pragmas(&conn)?;
        check_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Creates the ledger if missing and migrates it to this binary's
    /// version, after a backup of the old version.
    pub fn open_and_migrate(path: &Path) -> Result<Self, LedgerError> {
        Self::open_and_migrate_with(path, MIGRATIONS)
    }

    pub(crate) fn open_and_migrate_with(
        path: &Path,
        migrations: &[&str],
    ) -> Result<Self, LedgerError> {
        let supported = migrations.len() as i64;
        // A bare file name has the parent "", which cannot be opened to sync.
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        std::fs::create_dir_all(dir)?;
        // Serialises create, backup and migrate across processes. Switching
        // to WAL does not wait out busy_timeout, and two backups of one
        // version must not race.
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(path))?;
        lock.lock()?;
        // Read-only, under the lock: a read-write close would checkpoint a
        // newer binary's WAL into the file we are refusing.
        let found = match probe(path)? {
            None => 0,
            Some(probed) if foreign(probed) => {
                return Err(LedgerError::NotALedger(path.into()));
            }
            Some((found, ..)) if found > supported => {
                return Err(LedgerError::SchemaMismatch { found, supported });
            }
            Some((found, ..)) => found,
        };
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        if found > 0 && found < supported {
            migrate::backup(&conn, dir, path, found)?;
        }
        migrate::migrate(&conn, migrations)?;
        drop(lock);
        std::fs::create_dir_all(dir.join("evidence").join("sha256"))?;
        Ok(Self { conn })
    }

    /// The CLI's clock, RFC 3339 UTC with milliseconds, the form `apply`
    /// takes. Core functions never call it themselves.
    pub fn now(&self) -> Result<String, LedgerError> {
        Ok(self
            .conn
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |r| {
                r.get(0)
            })?)
    }

    #[cfg(test)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Not ours: anything but an empty file that lacks Seamlane's
/// `application_id` at a positive version. Checked before the version, so
/// another program's database is never told to upgrade or migrate.
fn foreign((found, has_schema, app): (i64, bool, i64)) -> bool {
    let empty = found == 0 && !has_schema;
    !empty && (found < 1 || app != APPLICATION_ID)
}

/// Refuses a connection whose file is not at this binary's version.
/// [`Ledger::apply`] re-checks it inside its transaction, because a newer
/// binary can migrate the file after this one opened it.
pub(crate) fn check_schema(conn: &Connection) -> Result<(), LedgerError> {
    let found = user_version(conn)?;
    if found != SCHEMA_VERSION {
        return Err(LedgerError::SchemaMismatch {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn apply_pragmas(conn: &Connection) -> Result<(), LedgerError> {
    conn.busy_timeout(Duration::from_millis(5000))?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if mode != "wal" {
        return Err(std::io::Error::other(format!("journal_mode is {mode}, not wal")).into());
    }
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// `user_version`, whether the file has any schema, and `application_id`,
/// read on a read-only connection, which cannot checkpoint or otherwise
/// write the file. `None` when there is no file. A file that is not SQLite
/// at all is not a ledger.
fn probe(path: &Path) -> Result<Option<(i64, bool, i64)>, LedgerError> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    let read = || -> Result<_, LedgerError> {
        let has_schema =
            conn.query_row("SELECT count(*) > 0 FROM sqlite_schema", [], |r| r.get(0))?;
        let app = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        Ok((user_version(&conn)?, has_schema, app))
    };
    match read() {
        Err(LedgerError::Sqlite(e))
            if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) =>
        {
            Err(LedgerError::NotALedger(path.into()))
        }
        probed => probed.map(Some),
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".lock");
    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &[&str] = &[MIGRATIONS[0], "CREATE TABLE extra (a)"];

    fn pragma<T: rusqlite::types::FromSql>(conn: &Connection, name: &str) -> T {
        conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    }

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    fn read_only(path: &Path) -> Connection {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }

    #[test]
    fn fresh_file_has_pragmas_schema_and_evidence_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ledger.db");
        let ledger = Ledger::open_and_migrate(&path).unwrap();
        let conn = ledger.conn();
        assert_eq!(pragma::<String>(conn, "journal_mode"), "wal");
        assert_eq!(pragma::<i64>(conn, "synchronous"), 2);
        assert_eq!(pragma::<i64>(conn, "busy_timeout"), 5000);
        assert_eq!(pragma::<i64>(conn, "foreign_keys"), 1);
        assert_eq!(user_version(conn).unwrap(), 1);
        assert_eq!(pragma::<i64>(conn, "application_id"), APPLICATION_ID);
        for table in [
            "events",
            "specs",
            "spec_versions",
            "lanes",
            "gate_results",
            "findings",
            "messages",
        ] {
            assert!(has_table(conn, table), "{table}");
        }
        for column in [
            "pid",
            "exit_code",
            "cli_session_id",
            "port_base",
            "port_count",
            "viewer_hint",
        ] {
            let notnull: i64 = conn
                .query_row(
                    "SELECT \"notnull\" FROM pragma_table_info('lanes') WHERE name = ?1",
                    [column],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(notnull, 0, "{column}");
        }
        let parent = path.parent().unwrap();
        assert!(parent.join("evidence/sha256").is_dir());
        drop(ledger);

        // Already current: no backup, and the agent open accepts it.
        Ledger::open_and_migrate(&path).unwrap();
        assert!(!parent.join("backups").exists());
        let ledger = Ledger::open(&path).unwrap();
        assert_eq!(pragma::<i64>(ledger.conn(), "synchronous"), 2);
    }

    #[test]
    fn missing_or_foreign_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        assert!(matches!(Ledger::open(&path), Err(LedgerError::Missing(_))));
        assert!(!path.exists());

        // An empty file is a ledger not yet migrated.
        File::create(&path).unwrap();
        let err = Ledger::open(&path).err().unwrap();
        assert!(matches!(
            err,
            LedgerError::SchemaMismatch {
                found: 0,
                supported: 1
            }
        ));
        assert!(err.to_string().contains("seamlane migrate"), "{err}");

        // Not a ledger, never read or migrated: another SQLite file at v0,
        // at a negative user_version, or at v1 or above without Seamlane's
        // application_id; or a file that is not SQLite at all.
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE old (a)")
            .unwrap();
        for version in [0, -1, 1, 40] {
            Connection::open(&path)
                .unwrap()
                .pragma_update(None, "user_version", version)
                .unwrap();
            let before = std::fs::read(&path).unwrap();
            for result in [Ledger::open(&path), Ledger::open_and_migrate(&path)] {
                assert!(
                    matches!(result, Err(LedgerError::NotALedger(_))),
                    "v{version}"
                );
            }
            assert!(
                before == std::fs::read(&path).unwrap(),
                "v{version} changed"
            );
        }
        assert!(!has_table(&read_only(&path), "events"));
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "not a database, just some text").unwrap();
        for result in [Ledger::open(&text), Ledger::open_and_migrate(&text)] {
            assert!(matches!(result, Err(LedgerError::NotALedger(_))));
        }
    }

    #[test]
    fn newer_file_refused_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let wal = dir.path().join("ledger.db-wal");
        // A newer binary killed with frames still in its WAL: copy its files
        // while its connection is open, so nothing in this process holds them.
        let src = dir.path().join("newer.db");
        let newer = Connection::open(&src).unwrap();
        newer
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE t (a); INSERT INTO t VALUES (1); PRAGMA user_version = 2;
                 PRAGMA application_id = 1397047629;",
            )
            .unwrap();
        std::fs::copy(&src, &path).unwrap();
        std::fs::copy(dir.path().join("newer.db-wal"), &wal).unwrap();
        drop(newer);
        let before = (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap());
        assert!(!before.1.is_empty());

        for result in [Ledger::open(&path), Ledger::open_and_migrate(&path)] {
            let err = result.err().unwrap();
            assert!(matches!(
                err,
                LedgerError::SchemaMismatch {
                    found: 2,
                    supported: 1
                }
            ));
            assert!(err.to_string().contains("left untouched"), "{err}");
        }
        let after = (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap());
        assert!(before == after, "ledger or WAL bytes changed");
        assert!(!dir.path().join("backups").exists());
    }

    #[test]
    fn upgrade_backs_up_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        Ledger::open_and_migrate(&path)
            .unwrap()
            .conn()
            .execute(
                "INSERT INTO specs VALUES (1, 'Flexipie/harness#1', 't', 'draft', 1, 1, 'now')",
                [],
            )
            .unwrap();

        let ledger = Ledger::open_and_migrate_with(&path, V2).unwrap();
        assert_eq!(user_version(ledger.conn()).unwrap(), 2);
        assert!(has_table(ledger.conn(), "extra"));

        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(backups, ["ledger-v1-0.sqlite"]);
        let backup = read_only(&dir.path().join("backups/ledger-v1-0.sqlite"));
        assert_eq!(user_version(&backup).unwrap(), 1);
        assert!(!has_table(&backup, "extra"));
        let key: String = backup
            .query_row("SELECT external_key FROM specs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(key, "Flexipie/harness#1");
    }

    #[test]
    fn failed_migration_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let broken = &[MIGRATIONS[0], "CREATE TABLE half (a); not sql"];
        assert!(matches!(
            Ledger::open_and_migrate_with(&path, broken),
            Err(LedgerError::Sqlite(_))
        ));
        let conn = read_only(&path);
        assert_eq!(user_version(&conn).unwrap(), 1);
        assert!(has_table(&conn, "events"));
        assert!(!has_table(&conn, "half"));
    }

    #[test]
    fn concurrent_create_and_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let race = |migrations: &'static [&'static str]| {
            std::thread::scope(|s| {
                let runs: Vec<_> = (0..4)
                    .map(|_| s.spawn(|| Ledger::open_and_migrate_with(&path, migrations).map(drop)))
                    .collect();
                for run in runs {
                    run.join().unwrap().unwrap();
                }
            });
        };

        race(MIGRATIONS);
        assert_eq!(user_version(&read_only(&path)).unwrap(), 1);
        assert!(!dir.path().join("backups").exists());

        race(V2);
        assert_eq!(user_version(&read_only(&path)).unwrap(), 2);
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(backups, ["ledger-v1-0.sqlite"]);
    }
}
