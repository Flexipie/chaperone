//! Facts by content key, in a SQLite file of their own (ADR 0043). A
//! cache: anything wrong with it is answered by parsing again, never by
//! failing the query.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::{Connection, ErrorCode, TransactionBehavior, ffi, params};

use crate::parse::{FACTS_VERSION, Facts};

pub struct Cache {
    conn: Option<Connection>,
    path: PathBuf,
    /// Why the cache was not used, when it was not.
    pub skipped: Option<String>,
}

fn table() -> String {
    format!("facts_v{FACTS_VERSION}")
}

impl Cache {
    /// `given`: the path was passed by hand (`--cache`), not Chaperone's
    /// own per-origin file.
    pub fn open(path: Option<&Path>, given: bool) -> Cache {
        let Some(path) = path else {
            return Cache {
                conn: None,
                path: PathBuf::new(),
                skipped: Some("no cache path".into()),
            };
        };
        match Self::connect(path, given) {
            Ok(conn) => Cache {
                conn: Some(conn),
                path: path.to_path_buf(),
                skipped: None,
            },
            Err(e) => Cache {
                conn: None,
                path: path.to_path_buf(),
                skipped: Some(set_aside(path, &e)),
            },
        }
    }

    fn connect(path: &Path, given: bool) -> rusqlite::Result<Connection> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let not_cache = || {
            rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_CANTOPEN),
                Some("not a chaperone graph cache".into()),
            )
        };
        // SQLite takes an empty file for an empty database. One passed by
        // hand is someone else's (a lock file); one at Chaperone's own path
        // is a cache a first run left before writing to it.
        if given && std::fs::metadata(path).is_ok_and(|m| m.len() == 0) {
            return Err(not_cache());
        }
        // SQLite names the WAL after the path it opened, so a hard link to the
        // ledger hides the ledger's WAL from the check below. Chaperone's own
        // cache is never hard-linked.
        if std::fs::metadata(path).is_ok_and(|m| m.nlink() > 1) {
            return Err(not_cache());
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // `--cache` may name another SQLite file (the ledger), and nothing may
        // write to it, its WAL included. Until the check passes, closing does
        // not checkpoint, and the check holds the write lock, so a migration
        // in flight is waited for and seen.
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let foreign: bool = conn.query_row(
            "SELECT (SELECT user_version FROM pragma_user_version) != 0 OR EXISTS (
            SELECT 1 FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'facts_v[0-9]*')",
            [],
            |r| r.get(0),
        )?;
        if foreign {
            let _ = conn.execute_batch("ROLLBACK");
            // A WAL with no frames (this open's, or one left empty) is safe to
            // checkpoint: closing then removes it as any last reader's does.
            let wal = conn.path().map(|p| format!("{p}-wal"));
            if wal.is_none_or(|w| std::fs::metadata(w).map_or(true, |m| m.len() == 0)) {
                let _ = conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, false);
            }
            return Err(not_cache());
        }
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {} (key TEXT PRIMARY KEY, facts BLOB NOT NULL) WITHOUT ROWID;
            COMMIT",
            table()
        ))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, false)?;
        Ok(conn)
    }

    /// The cached facts for `keys`; keys with none are left out.
    pub fn get(&mut self, keys: &[&str]) -> HashMap<String, Facts> {
        let mut out = HashMap::new();
        let Some(conn) = &self.conn else { return out };
        let result = (|| {
            let mut stmt =
                conn.prepare(&format!("SELECT facts FROM {} WHERE key = ?1", table()))?;
            for key in keys {
                let bytes: Option<Vec<u8>> = match stmt.query_row([key], |r| r.get(0)) {
                    Ok(b) => Some(b),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(e),
                };
                // A row this build cannot read is parsed again.
                if let Some(f) = bytes.and_then(|b| serde_json::from_slice(&b).ok()) {
                    out.insert(key.to_string(), f);
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.fail(&e);
        }
        out
    }

    /// Stores new facts in one transaction, and drops rows of files no
    /// worktree has had for a while: past four times this tree's count,
    /// every row not in `live`.
    pub fn put(&mut self, new: &[(String, Vec<u8>)], live: &[&str]) {
        let Some(conn) = &mut self.conn else { return };
        let t = table();
        let result = (|| {
            // Immediate: the prune's write must not fail to upgrade a read lock.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            {
                let mut ins = tx.prepare(&format!(
                    "INSERT OR IGNORE INTO {t} (key, facts) VALUES (?1, ?2)"
                ))?;
                for (key, facts) in new {
                    ins.execute(params![key, facts])?;
                }
            }
            let rows: i64 = tx.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))?;
            if rows as usize > 4 * live.len().max(256) {
                tx.execute_batch(
                    "CREATE TEMP TABLE IF NOT EXISTS live (key TEXT PRIMARY KEY); DELETE FROM live",
                )?;
                let mut keep = tx.prepare("INSERT OR IGNORE INTO live (key) VALUES (?1)")?;
                for key in live {
                    keep.execute([key])?;
                }
                drop(keep);
                tx.execute(
                    &format!("DELETE FROM {t} WHERE key NOT IN (SELECT key FROM live)"),
                    [],
                )?;
            }
            tx.commit()
        })();
        if let Err(e) = result {
            self.fail(&e);
        }
    }

    fn fail(&mut self, e: &rusqlite::Error) {
        self.conn = None;
        self.skipped = Some(set_aside(&self.path, e));
    }
}

/// A SQLite file SQLite calls corrupt or not a database is renamed aside,
/// never deleted (another process may have it open); the next run starts a
/// new one. Any other error, or file (`--cache` may name any), leaves it in
/// place.
fn set_aside(path: &Path, e: &rusqlite::Error) -> String {
    let corrupt = matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    );
    let mut head = [0; 16];
    let sqlite = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| &head == b"SQLite format 3\0");
    if corrupt && sqlite {
        // Its WAL and index go with it: a stale WAL beside the next cache
        // file would be replayed into it.
        for suffix in ["", "-wal", "-shm"] {
            let (mut from, mut to) = (path.as_os_str().to_owned(), path.as_os_str().to_owned());
            from.push(suffix);
            to.push(format!(".bad{suffix}"));
            let _ = std::fs::rename(from, to);
        }
        format!("{e}; set aside as .bad")
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #95: refusing the ledger as a cache must leave it and its WAL as they
    /// were, also through a symlink or a hard link (a read-write close
    /// checkpointed them).
    #[test]
    fn ledger_and_its_wal_left_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live.db");
        let c = Connection::open(&live).unwrap();
        c.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
             PRAGMA user_version = 4; CREATE TABLE events (x); INSERT INTO events VALUES (1);",
        )
        .unwrap();
        // A copy while the writer is open: the schema is only in its WAL.
        let ledger = dir.path().join("ledger.db");
        std::fs::copy(&live, &ledger).unwrap();
        std::fs::copy(
            dir.path().join("live.db-wal"),
            dir.path().join("ledger.db-wal"),
        )
        .unwrap();
        drop(c);
        let link = dir.path().join("link.db");
        std::os::unix::fs::symlink(&ledger, &link).unwrap();
        let bytes = |p: &str| std::fs::read(dir.path().join(p)).unwrap();
        let before = (bytes("ledger.db"), bytes("ledger.db-wal"));
        let hard = dir.path().join("hard.db");
        std::fs::hard_link(&ledger, &hard).unwrap();
        for path in [&ledger, &link, &hard] {
            let cache = Cache::open(Some(path), true);
            assert_eq!(
                cache.skipped.as_deref(),
                Some("not a chaperone graph cache")
            );
            drop(cache);
            assert!(
                before == (bytes("ledger.db"), bytes("ledger.db-wal")),
                "{path:?}"
            );
        }
        // A cache of its own is still used.
        let own = Cache::open(Some(&dir.path().join("own.db")), true);
        assert!(own.conn.is_some() && own.skipped.is_none());
    }
}
