//! Facts by content key, in a SQLite file of their own (ADR 0043). A
//! cache: anything wrong with it is answered by parsing again, never by
//! failing the query.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, ffi, params};

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
        match std::fs::metadata(path) {
            Ok(m) if m.len() == 0 && given => return Err(not_cache()),
            Ok(m) if m.len() > 0 => {
                // `--cache` may name another SQLite file (the ledger), and the
                // check must not write to it: a read-write connection would
                // checkpoint its WAL into it on close. Without a WAL, read it
                // as immutable (a read-only open would still make -wal and
                // -shm beside it); with one, read-only, so its WAL is seen.
                let mut wal = path.as_os_str().to_owned();
                wal.push("-wal");
                let ro = if Path::new(&wal).exists() {
                    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
                } else {
                    let Some(p) = path.to_str() else {
                        return Err(not_cache());
                    };
                    let uri = format!(
                        "file:{}?immutable=1",
                        p.replace('%', "%25")
                            .replace('?', "%3f")
                            .replace('#', "%23")
                    );
                    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
                    Connection::open_with_flags(uri, flags)?
                };
                ro.busy_timeout(Duration::from_secs(5))?;
                let foreign: bool = ro.query_row(
                    "SELECT (SELECT user_version FROM pragma_user_version) != 0 OR EXISTS (
                    SELECT 1 FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'facts_v[0-9]*')",
                    [],
                    |r| r.get(0),
                )?;
                if foreign {
                    return Err(not_cache());
                }
            }
            _ => {}
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {} (key TEXT PRIMARY KEY, facts BLOB NOT NULL) WITHOUT ROWID",
            table()
        ))?;
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
            let tx = conn.transaction()?;
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
        let mut aside = path.as_os_str().to_owned();
        aside.push(".bad");
        let _ = std::fs::rename(path, aside);
        format!("{e}; set aside as .bad")
    } else {
        e.to_string()
    }
}
