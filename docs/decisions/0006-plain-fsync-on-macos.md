# ADR 0006: Plain fsync on macOS, no F_FULLFSYNC

Status: accepted, 2026-11-07 (orchestrator, owner's night-pass grant)  
Amends: [R5](../research/r5-state-and-storage.md) "`FULL` is the setting to ship": what `FULL` means on macOS  
Evidence: issue [#55](https://github.com/Flexipie/chaperone/issues/55), finding 6 in [the v0 perf review](../perf/v0-review.md)

## Context

The ledger runs WAL with `synchronous=FULL` (locked in the roadmap). On macOS, `fsync` hands the data to the drive
but does not flush the drive's write cache; only `fcntl(F_FULLFSYNC)` does. Apple's own SQLite sets
`PRAGMA fullfsync`; the bundled SQLite from `libsqlite3-sys` does not. So a commit that returned can still sit in the
drive cache, and the drive may write it out in a different order than SQLite issued it.

Measured on the reference machine (Apple M2 Max, release, 2026-11-07), E1 is `tests/budgets.rs`, E2 is
`fifty_writers` in `tests/concurrency.rs`, each pragma added to `apply_pragmas` for the run only:

| Setting | E1 (≥ 2,000 commits/s) | E2 (≤ 5 s) |
| --- | --- | --- |
| plain `fsync` (today) | 8,650 to 12,800 commits/s, 0.08 to 0.12 ms each, 5 runs | 0.27 to 0.31 s, 3 runs |
| `fullfsync=ON` | 194 to 237 commits/s, 4.2 to 5.2 ms each, 5 runs | 2 of 3 runs failed (a writer gave up after the 5 s busy limit); the pass took 16.9 s |
| `checkpoint_fullfsync=ON` only | 1,000 to 6,200 commits/s, 3 of 5 runs miss the budget | 3.7 to 4.2 s, 3 runs |

## Decision

Keep plain `fsync`: no `fullfsync`, no `checkpoint_fullfsync`. On macOS, `synchronous=FULL` means a commit that
returned survives a killed process and an OS crash. It does not promise to survive a sudden power loss.

On power loss the last commits can be lost. **Checkpoint corruption is an accepted risk, not ruled out:** without a
full flush the drive can reorder a checkpoint's writes to the database file, so a power cut during or just after a
checkpoint could leave the file damaged.

## Alternatives considered

- **`fullfsync=ON`.** About 50x slower per commit, and under parallel writers the 4.5 ms flush brings back the lock
  starvation that #62 fixed. Parallel writers are the load Chaperone exists for.
- **`checkpoint_fullfsync=ON` only.** Covers the checkpoint corruption above, but misses E1 in 3 of 5 runs and leaves
  E2 near its limit.

## Consequences

Revisit this when either happens:

- a ledger is found corrupted (`PRAGMA integrity_check` fails) after a power loss;
- a user runs Chaperone on a machine with no battery (a desktop Mac), where power loss is a real event.

Recovery that exists today: `chaperone migrate` takes a `VACUUM INTO` backup in `<ledger dir>/backups/` before every
schema upgrade (`crates/chaperone-core/src/ledger/migrate.rs`). Restoring one loses every event since that upgrade.
There is no periodic backup.
