//! Shared by the CLI's integration tests.

use std::path::Path;

/// Restamps lane `id` in the ledger at `db` with `pipeline`, as a lane
/// claimed while it was the default: a claim only stamps v21 now. v21
/// leaves the stamp as the claim wrote it.
pub fn restamp(db: &Path, id: i64, pipeline: &str) {
    let doc = chaperone_core::pipeline::by_id(pipeline).unwrap();
    rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE lanes SET pipeline_id = ?1, pipeline_hash = ?2 WHERE id = ?3",
            rusqlite::params![doc.id, doc.hash, id],
        )
        .unwrap();
}
