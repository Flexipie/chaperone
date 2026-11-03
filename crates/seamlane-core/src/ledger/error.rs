use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("no ledger at {0}; run `seamlane migrate` to create it")]
    Missing(PathBuf),
    #[error("{0} is not a Seamlane ledger; refusing to migrate it")]
    NotALedger(PathBuf),
    #[error("ledger schema is v{found}, this binary supports v{supported}; {}", mismatch_hint(.found, .supported))]
    SchemaMismatch { found: i64, supported: i64 },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn mismatch_hint(found: &i64, supported: &i64) -> &'static str {
    if found > supported {
        "update seamlane; the file was left untouched"
    } else {
        "run `seamlane migrate`"
    }
}
