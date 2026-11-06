# ADR 0006: Plain fsync on macOS, no F_FULLFSYNC

Status: accepted, 2026-11-07 (orchestrator, owner's night-pass grant)  
Amends: [R5](../research/r5-state-and-storage.md) "`FULL` is the setting to ship": what `FULL` means on macOS  
Evidence: issue [#55](https://github.com/Flexipie/chaperone/issues/55), finding 6 in [the v0 perf review](../perf/v0-review.md)

## Context

The ledger runs WAL with `synchronous=FULL` (locked in the roadmap). On macOS, `fsync` hands the data to the drive
but does not flush the drive's write cache; only `fcntl(F_FULLFSYNC)` does. Apple's `fsync(2)` says that if the drive
loses power or the OS crashes, only some or none of the data may be written, and the drive may reorder it. Apple's own
SQLite (`/usr/bin/sqlite3` 3.51.0) defaults to `fullfsync=0` and `checkpoint_fullfsync=1`; the bundled SQLite from
`libsqlite3-sys` 0.39.1 sets neither.

Measured on the reference machine (Apple M2 Max, release, 2026-11-07), E1 is `tests/budgets.rs`, E2 is
`fifty_writers` in `tests/concurrency.rs`, each pragma added to `apply_pragmas` for the run only:

| Setting | E1 (≥ 2,000 commits/s) | E2 (≤ 5 s) |
| --- | --- | --- |
| plain `fsync` (today) | 8,650 to 12,800 commits/s, 0.08 to 0.12 ms each, 5 runs | 0.27 to 0.31 s, 3 runs |
| `fullfsync=ON` | 194 to 237 commits/s, 4.2 to 5.2 ms each, 5 runs | 2 of 3 runs failed (a writer gave up after the 5 s busy limit); the pass took 16.9 s |
| `checkpoint_fullfsync=ON` only | 1,000 to 6,200 commits/s, 3 of 5 runs miss the budget | 3.7 to 4.2 s, 3 runs |

## Decision

Keep plain `fsync`: no `fullfsync`, no `checkpoint_fullfsync`. This is weaker than Apple's default, which flushes
fully at checkpoints. On macOS, `synchronous=FULL` means a commit that returned survives a killed process. It does not
promise to survive a power loss or an OS crash (a kernel panic).

On a power loss or an OS crash the last commits can be lost. **Checkpoint corruption is an accepted risk, not ruled
out:** without a full flush the drive can reorder a checkpoint's writes to the database file, so a power loss or OS
crash during or just after a checkpoint could leave the file damaged.

## Alternatives considered

- **`fullfsync=ON`.** About 50x slower per commit, and under parallel writers the 4.5 ms flush brings back the lock
  starvation that #62 fixed. Parallel writers are the load Chaperone exists for.
- **`checkpoint_fullfsync=ON` only** (Apple's default). Covers the checkpoint corruption above, but misses E1 in 3
  of 5 runs and leaves E2 near its limit.

## Consequences

Revisit this when either happens:

- a ledger returns a corruption error (`SQLITE_CORRUPT`, "database disk image is malformed") after a power loss or
  OS crash; nothing runs `integrity_check` on its own, so this is noticed when a command fails;
- a user runs Chaperone on a machine with no battery (a desktop Mac), where power loss is a real event.

Recovery that exists: `chaperone migrate` takes a `VACUUM INTO` backup in `<ledger dir>/backups/` before every schema
upgrade (`crates/chaperone-core/src/ledger/migrate.rs`). Restoring one loses every event since that upgrade. The
schema is still at version 1, so no ledger has a backup yet: until the first upgrade ships there is nothing to restore.
There is no periodic backup.
