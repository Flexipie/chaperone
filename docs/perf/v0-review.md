# Perf review of v0

Issue [#36](https://github.com/Flexipie/harness/issues/36). Where v0 (milestone 1) is slow or heavy, measured
against the budgets in [R14](../research/r14-engineering-standards.md#performance-budgets), before the harness lanes
switch onto Chaperone.

**Short answer:** every R14 product budget that v0 can be measured against passes, most with a wide margin. One
command missed M1 at scale (`work board` past about 150 open items; fixed since in #51 and #74). One read was
cheap to fix and is fixed here (`spec get --key`, p50 9.8 ms to 6.7 ms on the realistic ledger). The rest become
follow-up issues, listed at the end.

## Method

- Machine: Apple M2 Max, local APFS, warm page cache (the R14 reference class). rustc 1.95.0, the release profile in
  `Cargo.toml` (thin LTO, one codegen unit, stripped). Bundled SQLite from `libsqlite3-sys` 0.39.1.
- Before: `main` at `d6037ca`. After: this branch (`11f4916`, the `spec get --key` fix). Both binaries ran in one
  invocation of [`measure.py`](measure.py) against copies of one seeded snapshot, so they read the same data.
- Startup and per-command latency: R14's method, 8 unmeasured spawns then 100 samples, wall time around the spawn
  from Python. Every sample must exit 0, so a refusal is never timed as a success. `/usr/bin/true` on the same harness
  is the floor (about 2 ms of the numbers below is spawn cost, not Chaperone).
- Each write sample is a real write: claims take a fresh queued item each, handoffs and check records go to items
  claimed before the loop, check records come from a second session (a holder cannot record its own checks).
- Empty ledger: freshly migrated, holding only the items each command needs. Realistic ledger: seeded through the CLI
  with 300 specs of 10 slices each, 3,000 items, each created, claimed, heartbeated, handed off twice and checked once,
  plus 3 deep lanes with 300 check records each. Only the newest 5 specs stay open (50 items); the rest are cancelled,
  as finished work leaves the board. 19,156 events, 9.3 MiB. The hot-command setup adds 216 open items, so the
  realistic runs see 269 open.
- E1 and E2 are timed in the process, around the loop only: `crates/chaperone-core/tests/budgets.rs` (new, ignored
  by default) and the existing `fifty_writers` in `tests/concurrency.rs`, both in release.
- Safety: every run lives in a fresh temp dir. Every call passes `--db` there, and the child environment holds only
  `PATH`, `HOME` and `XDG_DATA_HOME` (pointing at the temp dir) and the two `DAISY_*` actor variables, so a missed
  `--db` cannot reach a real ledger. Before each temp dir is removed, also when a run aborts, the script fails if `Library/Application Support/chaperone`
  appeared inside it, where a missed `--db` lands; other lanes writing the real ledger don't trip it.
  No `daisy-cli` call.

Reproduce:

The first binary builds the shared ledger, so both must carry application_id "CHAP" ([ADR 0005](../decisions/0005-chaperone-name.md)): a binary from before the rename writes "SEAM", which `chaperone` refuses. Build the before-binary from a commit after the rename.

```sh
cargo build --release --locked
python3 -I docs/perf/measure.py <before-binary> target/release/chaperone --items 3000
cargo test --release --locked -p chaperone-core --test budgets -- --ignored --nocapture
cargo test --release --locked -p chaperone-core --test concurrency fifty_writers -- --nocapture
```

## Against R14

Product column, reference machine. Before and after are the same unless both are shown.

| Id | Budget | Measured | |
| --- | --- | --- | --- |
| S1 | `--version` p50 ≤ 10 ms, p99 ≤ 25 ms | p50 3.2 to 3.3 ms, p99 3.8 to 4.3 ms | pass |
| S2 | open WAL, `SELECT 1`, exit: p50 ≤ 15 ms, p99 ≤ 30 ms | `work list` on an empty ledger (a superset of S2: clap, the probe, the pragmas, the schema check and JSON output): p50 4.4 to 4.5 ms, p99 5.6 to 6.2 ms | pass |
| E1 | one writer ≥ 2,000 commits/s | 12,000 to 14,300 commits/s, mean 0.07 to 0.08 ms (8 warm runs; the first run after a rebuild did 5,040) | pass, see finding 6 |
| E2 | 50 processes × 40 events ≤ 5 s | 0.89 s, correctness checks green | pass |
| L1 | follower sees an event p50 ≤ 5 ms | not measurable: no follower or wake exists in v0 | n/a |
| M1 | one-shot RSS ≤ 8 MiB | `--version` 2.4 MiB, `work list` 4.5 MiB empty and 6.7 MiB realistic, `work board` 11.7 MiB at 269 open items; after #51 `work board` 8.27 to 8.31 MiB at 1,000 open, beside `work list` 8.06 to 8.09 MiB on the same ledger; after #74 (`cache_size` 512 KiB) `work board` 6.59 to 6.62 MiB at 1,000 open and 6.72 to 6.75 MiB at 3,000 open, `work list` 6.17 to 6.28 MiB, every other hot command 4.5 to 6.1 MiB; after #42 (`overlaps` on the board) `work board` 7.20 to 7.39 MiB at 1,000 open and 7.44 to 7.73 MiB at 3,000 open, `overlap` 6.5 to 6.8 MiB at 50 lanes, and the extreme case in finding 8 7.83 to 7.98 MiB | pass: `work board` fixed (finding 1), the shared SQLite page-cache floor fixed in [#74](https://github.com/Flexipie/chaperone/issues/74) |
| M2, M3 | supervisor RSS | no supervisor in v0 | n/a |
| B1 | stripped binary ≤ 15 MiB, macOS links `libSystem` only | 3.70 MiB; links `libSystem` and `libiconv` | size passes; the link wording missed, see below |
| I1 | instruction count vs base | the `regress` job does not exist yet | n/a, finding 7 |

B1: `libiconv` comes from the `libc` crate, which links it on Apple targets unless it is built as part of std
(`libc-0.2.191/src/unix/bsd/apple/mod.rs:5545`, for its deprecated `iconv_*` bindings). A hello-world built with the
same rustc links `libSystem` only. Both are system libraries, so the intent (nothing to install next to the binary)
holds. R14's B1 cell now says so, with this measurement, as R14 asks for a miss on the first real binary.

## Per-command latency

Wall ms per spawn, p50 / p99. About 2 ms of each is the spawn floor (`/usr/bin/true`: 1.9 to 2.0 / 2.5).

| Command | Empty, before | Empty, after | Realistic, before | Realistic, after |
| --- | --- | --- | --- | --- |
| `work get` | 4.20 / 5.01 | 4.58 / 8.96 | 4.13 / 5.09 | 4.11 / 6.78 |
| `work list` | 5.35 / 6.86 | 5.19 / 6.01 | 5.93 / 6.80 | 6.16 / 11.74 |
| `work claim` | 5.38 / 9.31 | 5.11 / 9.53 | 5.25 / 6.27 | 5.40 / 9.47 |
| `work heartbeat` | 4.82 / 10.11 | 4.90 / 6.04 | 4.49 / 10.06 | 4.73 / 5.56 |
| `work handoff` | 4.36 / 5.57 | 4.87 / 9.21 | 4.88 / 10.79 | 4.82 / 5.85 |
| `work handoff`, lane with 300+ events | | | 4.93 / 6.40 | 4.47 / 5.33 |
| `spec get --key` | 4.65 / 9.06 | 4.88 / 5.76 | **9.80 / 10.90** | **6.70 / 8.06** |
| `check record` | 4.35 / 5.96 | 5.26 / 6.27 | 4.90 / 5.82 | 4.84 / 5.70 |
| `work board` (20 samples) | | | 9.53 p50 | 9.19 p50 |

Apart from `spec get --key`, before and after differ by run-to-run noise: about ±0.5 ms at p50, and single p99 samples of 9 to 12 ms that land on either binary from run to run. A 3,000-item,
19,000-event ledger costs the hot commands nothing measurable: every hot read is an index search (below), and
`ledger open` is about 1 ms over `--version`. The handoff gate projects every event of the lane inside the write; on the
3 deep lanes (300 real check events each, distinct heads, so none is replayed) a handoff is as fast as on a shallow one.

## Write throughput and lock wait

- E1, in the process: 12,000 to 14,300 commits/s at `synchronous=FULL`, about 7x the budget. R6 measured the same
  fsync-bound ceiling (13k to 19k rows/s) for a bare insert, so the write function, guard reads and fold add little.
- E2, `fifty_writers` in release: 50 processes × 40 events in 0.89 s, about 2,250 commits/s with every process start
  included.
- 50 CLI processes at once, 20 `work create` each (`measure.py`): 1.5 s wall, about 650 to 680 commits/s. Per
  call p50 42 to 45 ms, p99 168 to 179 ms, max 245 to 263 ms, against 4.6 to 5.4 ms for one writer alone. Most of that is
  waiting for the lock: the work itself is under 0.1 ms a commit (E1). Finding 2: the busy handler does not set the tail.
- Write p99 on the realistic ledger is mostly near 6 ms, with single samples of 9 to 11 ms; the likely
  cause is the WAL autocheckpoint (every 1,000 pages, synced at `FULL`). It is well inside budget, so it was not
  chased.

## Query plans

`EXPLAIN QUERY PLAN` on the realistic ledger (Python's SQLite 3.54.1; the binary bundles its own, so treat these as
what the schema allows, not a capture from the binary).

| Read | Plan | Note |
| --- | --- | --- |
| `work get` | lanes by rowid; first key via `events_lane` | good |
| `work list` | scan `lanes_status`, temp B-tree for `ORDER BY updated_at` | linear in lanes, 0.18 ms at 3,000 |
| idempotency lookup | `sqlite_autoindex_events_1` | good |
| `has_held`, latest handoff | `events_lane (lane_id=?)` | good |
| `next_id` | `max(id)` on the rowid | good |
| spec by key (`COLLATE NOCASE`) | `specs_key (external_key=?)` since ledger v3 (#53); a **scan** of `specs` before (the BINARY unique index cannot serve NOCASE) | finding 4, fixed |
| spec by slice key (`spec get --key`, no `--spec`) | `spec_keys_key (key=?)` since ledger v3 (#53); before, every spec's head body parsed in Rust, newest first | finding 3, fixed |
| slice sibling by `source_id` (`COLLATE NOCASE`), before | **scan** `lanes`, 0.25 ms at 3,000 lanes, once per sibling | fixed for linked siblings |
| slice sibling linked to the spec, after | `lanes_slot (spec_id=?)`, 0.026 ms; `lanes_source (source_id=?)` since ledger v3 | new |
| slice sibling, fallback when none is linked | `lanes_source (source_id=?)` since ledger v3 (#53); a **scan** of `lanes` before | finding 4, fixed |
| job lookup by `$.job` (`reviews get`, `jobs get`, `reviews record`) | `events_job (<expr>=?)` since ledger v2 (#54); a **scan** before, 1.9 ms at 18,000 events | finding 5, fixed |

Unused indexes: none worth dropping. `lanes_holder (agent, session)` is used by `work list --mine`; `messages_to` has
no reader yet, because messages have no command in v0.

## How much each write folds

The fold is incremental (`crates/chaperone-core/src/ledger/fold.rs:46`). An event is one `INSERT` into `events` plus,
for a lane event: one `UPDATE lanes` per changed column, one for the lease when the kind leases, one for the pipeline
stamp at the first claim, one for the phase, and one for `head_event_id`/`updated_at`; a `check.finished` upserts one
`gate_results` row and inserts its findings. A claim is about 6 statements. Nothing rebuilds: `Ledger::rebuild`
(`fold.rs:231`) runs only when called, and no CLI command calls it. The checks before the write are a handful of
indexed point reads (schema version, canonical time, idempotency key, holder, guard). At E1's 0.07 ms a commit there
is nothing here to win; folding the per-column updates into one statement would be a micro-gain for the I1 bench, not
the product.

## Binary size

3.70 MiB stripped (B1: 15 MiB). From an unstripped release build (`CARGO_PROFILE_RELEASE_STRIP=none`), `__text` is
3.1 MiB:

| Part | Share of code |
| --- | --- |
| SQLite (C, bundled) | 50% |
| generic Rust (monomorphised, not attributable to one crate) | 14% |
| `chaperone_core` | 9% |
| `core`, `std`, `alloc` | 10% |
| `clap_builder` | 5% |
| `gimli` (std's backtrace symbolizer) | 3.5% |
| `chaperone` (the CLI) | 2.6% |
| `serde_json`, `toml`, `serde`, `rusqlite`, `sha2` | 5% together |

R6's 2.1 MiB probe had no clap, serde or toml. Nothing here is close to the budget, so no size work is proposed.

## CI wall time

From `gh run view` on the last merged PRs on `main` (`d6037ca` and earlier):

| Job | Typical time once started |
| --- | --- |
| fmt | 12 to 24 s |
| doc | 18 to 25 s |
| deny | 14 to 32 s |
| clippy | 21 to 29 s |
| test (linux) | 36 to 52 s |
| test (macos) | 45 to 120 s |
| coverage (nightly, llvm-cov) | 35 to 47 s |

Every job finishes in about a minute once it starts. What dominates wall time is **waiting for a macOS runner**: on
`4304f4e` `test (macos)` started 10 minutes after the others, on `1609381` 5 minutes after, and the run for `d6037ca`
was still queued when this was written. The Linux jobs never waited. Nothing to cut in the jobs themselves; the
macOS queue is outside the repo (the owner's rule already lets a queued job not block a reviewed PR).

## Findings

Cost is measured on this machine; gain is expected, not yet measured, except where the fix landed.

| # | Finding | Measured cost | Expected gain | Action |
| --- | --- | --- | --- | --- |
| 0 | `spec get --key` read every spec's full row (two lane counts and an event read each) to find the slice, and scanned every lane once per sibling (`spec/slice.rs`) | p50 9.8 ms, p99 10.9 ms at 300 specs and 3,000 lanes (key at spec 150 of 300), growing linearly with both | 3.1 ms p50 | **Fixed here**: `spec_for_key` reads only the head body; `work_for_key` asks for the lane linked to the spec through `lanes_slot` first and scans only when none is. Output byte-identical to before on linked, unlinked, newer-unlinked and different-case keys, and the old and new lane queries agree on 6,000 random cases (lane tables with null spec ids, other sources and case variants). Now 6.7 / 8.1 ms. |
| 1 | `work board` holds every open item as structs, then as a `serde_json::Value` tree, then as one pretty string, about 27 KiB of RSS per open item. `work ready` built the whole board to answer for one item (`crates/chaperone-cli/src/work.rs`) | 11.7 MiB RSS and 9 to 10 ms at 269 open items (**M1 miss**); 89 MiB and 76 ms at 3,000 open | under 8 MiB at any realistic count; `ready` O(1) | **Fixed in #51**: the board is serialized row by row into an unlinked temp file and copied to stdout after the read ends, so a slow reader never holds the WAL snapshot (output byte-identical), and `ready` reads its one item. At 1,000 open items (`measure.py` seed, `--items 1000 --open-specs 100`) `work board` went from 34.9 to 35.3 MiB to 8.27 to 8.31 MiB, and no longer grows per item: `work list` sits at 8.06 to 8.09 MiB on the same ledger. That floor is SQLite's default 2 MiB page cache, every command's. **Fixed in #74**: the ledger connection sets `cache_size = -512` (512 KiB). Only `work list` and `work board` read enough pages to fill the cache: at 3,000 open items the board went from 9.45 to 9.53 MiB to 6.72 to 6.75 MiB. `measure.py` over default, -1024, -512 and -256 builds (both orders, 1,000 and 3,000 open), plus E1 and E2 (in-process), moved no latency beyond run-to-run spread; -256 saved only 0.3 MiB more. |
| 2 | Under contention a write waits for the lock; the guess was that SQLite's default busy handler, sleeping in growing steps up to 100 ms, made writers sleep long after the lock was free. Measured in #52: it does not | 50 CLI writers: per call p99 about 170 ms, max about 290 ms, for 0.07 ms of work | none from the busy handler (measured) | #52 ran `measure.py` (50 x 20 `work create`, release builds of `1cc515e` differing only in the sleep in `apply_pragmas`) three times, the last in reverse order: p99 233 to 298 ms with the default handler, 217 to 248 ms at 1 ms, 226 to 263 ms at 2 ms, 203 to 248 ms at 5 ms. No sleep length moves p99 beyond run-to-run spread, and throughput (479 to 547 commits/s) falls with run order, not handler. The 1.8 ms commit slot under load is far above E1's 0.07 ms, so the bound is outside the commit; it was not isolated further. The spawn floor in these runs was 2.8 to 3.4 ms (one block 5.0), above the 2 ms of the main runs, so compare within this row only. The 1 ms handler stays for #62, where the default starved a waiter for the whole 5 s (`fifty_writers` failing on CI). |
| 3 | `spec get --key` without `--spec` still parses every spec body, newest first, until one has the key | about 3 ms with the key at spec 150 of 300 newest-first, about twice that for the oldest spec or a missing key; linear | constant time | **Fixed in #53**: migration 0003 (ledger v3) adds `spec_keys`, the slice keys of each spec's head version, written by the `spec.created` and `spec.revised` fold and filled for existing specs by the migration's Rust step (keys come out of the Rust spec parser, so unlike `events_job` this needs the fold). `measure.py`, release builds of `e93c47f` and the branch, realistic ledger (300 specs, key at spec 150): `spec get --key` p50 7.48 to 5.68 ms, p99 11.63 to 7.46 ms; the rest is process start. |
| 4 | `COLLATE NOCASE` lookups on `specs.external_key` and `lanes.source_id` (no NOCASE index) scan the table, as does the sibling fallback for unlinked slices | 0.25 ms per scan at 3,000 lanes, linear | constant time | **Fixed in #53**: migration 0003 adds `specs_key` and `lanes_source`, indexes in the collation the queries name (`COLLATE NOCASE`), so matching stays any-case and the duplicate-spec refusal reads the same query. `specs_key` is not UNIQUE, so the migration cannot fail on old data; the refusal stays the guard. Plans in "Query plans"; `slice::tests::key_lookups_use_indexes` holds them. |
| 5 | Job lookups filter `json_extract(payload_json, '$.job')` over every check event | 1.9 ms at 18,000 events, linear (about 10 ms at 100,000) | constant time | **Fixed in #54**: migration 0002 (ledger v2) adds `events_job`, a partial expression index on `json_extract(payload_json, '$.job')` over the two check kinds, matching `JOB_EVENTS` term for term (the planner drops a partial index if the IN list differs, so `checks.rs` has a test on the plan). Chosen over a `job_id` column: no change to the write or fold, existing rows indexed by the migration. `measure.py` on 19,156 events, release builds of `9ff48a5` and the branch, a missing job in the process: p50 3.52 to 0.005 ms, p99 3.91 to 0.006 ms. CLI rows on the realistic ledger did not move (`check record` p50 6.76 to 6.61 ms). |
| 6 | E1's 12,000 to 14,300 commits/s at `synchronous=FULL` (this review's runs) is plain `fsync`. Bundled SQLite does not use `F_FULLFSYNC` on macOS (Apple's own build uses it at checkpoints), so on a power loss or an OS crash the last commits can be lost, and a checkpoint's reordered writes could corrupt the file (not ruled out) | n/a (durability, not speed) | a decision | Decided in #55, [ADR 0006](../decisions/0006-plain-fsync-on-macos.md): keep plain `fsync`. `fullfsync=ON` measured about 200 commits/s on E1 and broke E2; the corruption risk is accepted, with a trigger to revisit. |
| 7 | R14's `budgets` and `regress` CI jobs do not exist yet, so nothing stops a regression in these numbers | n/a | budgets enforced on every PR | Follow-up: grow `tests/budgets.rs` (this PR adds E1) into the `budgets` job, plus the gungraun `regress` bench. Done for `budgets` (S1, S2, E1, M1 on every PR); `regress` (gungraun) still open on #56. |
| 8 | #42 adds `overlaps` to `work board` (paths a lane's newest handoff shares with other open lanes in its repo) and `chaperone overlap`. A first cut joined each lane with every other lane on a shared path, a row per pair | 3,000 open lanes in one repo whose handoffs all list the same 2 files: `work board` 9.6 s, 23.5 MiB RSS, 344 MiB of output (main: 57 ms, 6.7 MiB) | linear in lanes | **Fixed in #42** before merge: `shared` groups paths two lanes share, with a lane count and the head of the ordered id list (`group_concat(id ORDER BY id)`, 240 characters), and each row matches its own handoff against it with the handoff outside the join (`CROSS JOIN`; inside, its JSON was parsed again per shared path, 41 ms instead of 6 ms at 50 lanes). Each path lists the lowest 10 other lanes and counts the rest. `measure.py`, release builds of `d9d41d8` and the branch, before / after: realistic ledger (no repo, so nothing compared) `work board` 6.62 to 6.64 / 7.20 to 7.39 MiB and p50 21.9 / 23.9 ms at 1,000 open, 6.80 / 7.44 to 7.73 MiB and p50 59.8 / 71.3 ms at 3,000 open (55 / 58 ms on a repeat); 50 open lanes with worktrees, 245 shared files: `work board` p50 5.8 to 6.0 / 7.7 to 7.9 ms, `work ready` 4.7 to 5.8 / 5.7 to 6.0 ms, `overlap` p50 88 to 90 ms (max 104), `--uncommitted` 460 to 491 ms; the extreme case above 7.83 to 7.98 MiB, p50 about 97 ms. The ordered id list was kept over an unordered one (about 0.35 MiB less) so agents that diff or re-read the board see stable output (orchestrator decision); the extreme case is far from real use and well under the 32 MiB CI ceiling. |

Not findings: startup (S1 3.2 to 3.3 ms, 2 ms of it spawn), the open path (about 1 ms over `--version`; the read-only probe
connection is deliberate, it keeps a refused newer or foreign file byte-identical), the fold, write throughput and
binary size all sit well inside their budgets.

## Follow-up issues

| Finding | Issue |
| --- | --- |
| 1, `work board` RSS and `work ready` | [#51](https://github.com/Flexipie/harness/issues/51) |
| 2, busy handler under contention | [#52](https://github.com/Flexipie/harness/issues/52) |
| 3 and 4, indexed slice-key and NOCASE lookups (schema) | [#53](https://github.com/Flexipie/harness/issues/53) |
| 5, indexed job lookups (schema) | [#54](https://github.com/Flexipie/harness/issues/54) |
| 6, `fullfsync` on macOS (owner decision) | [#55](https://github.com/Flexipie/harness/issues/55) |
| 7, `budgets` and `regress` CI jobs | [#56](https://github.com/Flexipie/harness/issues/56) |
