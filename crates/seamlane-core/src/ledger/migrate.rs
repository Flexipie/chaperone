use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use super::LedgerError;

pub fn user_version(conn: &Connection) -> Result<i64, LedgerError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Applies each pending migration in its own `BEGIN IMMEDIATE` transaction
/// together with its `user_version` bump, so a failure rolls back that
/// migration whole. The version is re-read under the write lock, so a
/// concurrent migrator's work is skipped, not applied twice.
pub(crate) fn migrate(conn: &Connection, migrations: &[&str]) -> Result<(), LedgerError> {
    let supported = migrations.len() as i64;
    for (i, sql) in migrations.iter().enumerate() {
        let version = i as i64 + 1;
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let found = user_version(&tx)?;
        if found > supported {
            return Err(LedgerError::SchemaMismatch { found, supported });
        }
        if found >= version {
            continue;
        }
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// `VACUUM INTO <dir>/backups/<stem>-v<version>-<n>.sqlite`, n the first free
/// number (no clock). Written as `.incomplete`, synced, then renamed, so a
/// file with the final name is always a whole snapshot. A failed backup stops
/// the upgrade. VACUUM cannot run inside a transaction, so the caller holds
/// the migrate lock instead.
pub(super) fn backup(conn: &Connection, path: &Path, version: i64) -> Result<(), LedgerError> {
    let dir = path.parent().unwrap_or(Path::new("")).join("backups");
    std::fs::create_dir_all(&dir)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for n in 0.. {
        let name = format!("{stem}-v{version}-{n}");
        let done = dir.join(format!("{name}.sqlite"));
        let pending = dir.join(format!("{name}.incomplete"));
        if done.exists() {
            continue;
        }
        match options.open(&pending) {
            Ok(file) => drop(file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
        let target = pending
            .to_str()
            .ok_or_else(|| std::io::Error::other("backup path is not UTF-8"))?;
        conn.execute("VACUUM main INTO ?1", [target])?;
        std::fs::File::open(&pending)?.sync_all()?;
        std::fs::rename(&pending, &done)?;
        return Ok(());
    }
    unreachable!("backup names exhausted")
}
