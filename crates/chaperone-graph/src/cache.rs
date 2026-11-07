//! Facts by content key, in a SQLite file of their own (ADR 0043). A
//! cache: anything wrong with it is answered by parsing again, never by
//! failing the query.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, params};

use crate::parse::{FACTS_VERSION, Facts};

pub struct Cache {
    conn: Option<Connection>,
    /// Why the cache was not used, when it was not.
    pub skipped: Option<String>,
}

fn table() -> String {
    format!("facts_v{FACTS_VERSION}")
}

impl Cache {
    pub fn open(path: Option<&Path>) -> Cache {
        let Some(path) = path else {
            return Cache {
                conn: None,
                skipped: Some("no cache path".into()),
            };
        };
        match Self::connect(path) {
            Ok(conn) => Cache {
                conn: Some(conn),
                skipped: None,
            },
            Err(e) => Cache {
                conn: None,
                skipped: Some(set_aside(path, &e)),
            },
        }
    }

    fn connect(path: &Path) -> rusqlite::Result<Connection> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
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
    pub fn get(&mut self, keys: &[&str], path: Option<&Path>) -> HashMap<String, Facts> {
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
            self.fail(path, &e);
        }
        out
    }

    /// Stores new facts in one transaction, and drops rows of files no
    /// worktree has had for a while: past four times this tree's count,
    /// every row not in `live`.
    pub fn put(&mut self, new: &[(String, Vec<u8>)], live: &[&str], path: Option<&Path>) {
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
            self.fail(path, &e);
        }
    }

    fn fail(&mut self, path: Option<&Path>, e: &rusqlite::Error) {
        self.conn = None;
        self.skipped = Some(match path {
            Some(p) => set_aside(p, e),
            None => e.to_string(),
        });
    }
}

/// A file SQLite calls corrupt or not a database is renamed aside, never
/// deleted (another process may have it open); the next run starts a new
/// one. Any other error leaves it in place.
fn set_aside(path: &Path, e: &rusqlite::Error) -> String {
    let corrupt = matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    );
    if corrupt {
        let mut aside = path.as_os_str().to_owned();
        aside.push(".bad");
        let _ = std::fs::rename(path, aside);
        format!("{e}; set aside as .bad")
    } else {
        e.to_string()
    }
}
