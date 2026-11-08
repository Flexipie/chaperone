# R14: Engineering standards

> Amended by [ADR 0003](../decisions/0003-chaperone-replaces-daisy.md): the `reader_contract` axis and daisy reading SQL views are dropped. [CONTRIBUTING.md](../../CONTRIBUTING.md) "Versions" has the four axes.

Status: recommendation, 2026-11-02. The bar the first code PR meets. The contributor-facing copy is [CONTRIBUTING.md](../../CONTRIBUTING.md).

Language is Rust ([R6](r6-language-and-runtime.md)). Storage is one SQLite file ([R5](r5-state-and-storage.md)). The pipeline is a TOML document ([R7](r7-process-engine.md)). This note decides how that code is laid out, tested, timed, versioned, and reviewed. It does not add a crate.

## Recommendation

A Cargo workspace of five crates, pipelines as data on disk, and a GitHub Actions workflow that blocks the PR. Tests are five layers. The bottom three are pure or single-process. Crash and concurrency spawn real processes against the real write function. A fake agent CLI runs a whole lane with no model.

Performance has two numbers per budget. The **product** number is the promise, measured on the same class of machine as R6 (Apple M-series, local APFS, warm page cache, release profile). The **CI** number is a coarse ceiling on a GitHub-hosted runner, plus an instruction-count regression of 10% on Linux. Wall-clock percentages are not the merge gate: Criterion's FAQ and Tokio's own CI discussion both treat hosted-runner timings as too noisy for that ([criterion FAQ](https://bheisler.github.io/criterion.rs/book/faq.html), [tokio#7779](https://github.com/tokio-rs/tokio/pull/7779)).

### Test pyramid

Milestone 1, once the ledger and the gates exist. The first PR that adds a layer includes that layer's tests. Times are for a debug `cargo nextest` run.

| Layer | What a failure means | Milestone-1 minimum | Time |
| --- | --- | --- | --- |
| **Unit** | One predicate, one SQL shape, or one parser branch is wrong. | Every R7 host predicate has a pass and a fail. The `verdict_on_head` deny fixture and its mirror pass. Migration rollback leaves `user_version`. `reader_events` omits `payload_json`. One redactor fixture per R13 pattern. `docs/events.md` matches the event-kind enum. Two pipeline ids load in one process. | Each test under 50 ms. Layer under **10 s**. |
| **Property** | An invariant fails on an input nobody wrote by hand. | Seven properties below, **256 cases** each. Seed is the commit sha, printed on failure. | Under **30 s**. |
| **Crash** | A killed process left a torn ledger or a second session. | Three tests below. `SIGKILL` hits the real write function. | Under **20 s**. |
| **Concurrency** | Two writers corrupted a row, or a lock was held wrong. | Four tests below, including the R5 probe: 50 processes, 40 events each. | Under **30 s**. Per-test timeout **60 s**. |
| **Lane** | A scripted agent walked a path the engine should have allowed or refused. | Four scripts below. Each under **2 s**. Cap **20** scripts. | Under **30 s**. |

The PR test job that runs all five stays under **3 minutes**. The workflow timeout is **10 minutes**. A tag adds a 10,000-case property run, on the same seed rule, and that run blocks the tag.

The seven properties:

1. **Rebuild.** A random sequence of accepted mutations, folded from the log, equals the tables. A random prefix does too.
2. **Retry.** The same idempotency key inserts one row. A prose field that redacts produces the same stored bytes on both attempts ([R13](r13-security-and-trust.md): redact before the key).
3. **Refusal.** A transition the engine denies appends nothing and leaves the phase unchanged.
4. **Hash.** Comments, whitespace, and key order keep the pipeline's semantic hash. An argument change does not ([R7](r7-process-engine.md)).
5. **Sha prefix.** `sha_prefix_eq` holds only when the left side is 7 to 40 hex digits and the right side starts with it.
6. **Closed vocabulary.** A document that names an unknown predicate fails to load.
7. **Lease.** Two claimants, one `changes() = 1`. The loser appends no event.

`at` is an argument of the write function ([R5](r5-state-and-storage.md)). Property tests pass the clock in. The write function reads the wall clock only when no clock is passed (a CLI command, under the write lock), so a replay of the sha seed is the same run.

The three crash tests:

1. `SIGKILL` after the insert and the fold update, before `COMMIT`. The uncommitted event is absent, the earlier seed is present, `integrity_check` is ok, the projection matches the log. This is the R5 case, moved onto the real write function.
2. `SIGKILL` during a tight commit loop. The committed prefix has distinct keys, integrity is ok, the projection matches.
3. The fake agent is killed after `session.started`. The supervisor records `session.exited` with the recovered flag and does not spawn a replacement ([R8](r8-process-terminal-runtime.md)).

The four concurrency tests:

1. **50 processes, 40 events each**, `synchronous=FULL`, a 5 s busy limit retried every 1 ms (#62), `BEGIN IMMEDIATE`, one event plus the fold, then a retry of the last key. Zero errors, zero projection mismatches, zero duplicate keys. The retry returns the stored event (`Replayed`) and does not insert.
2. Two deferred transactions that both go on to write. The second receives `SQLITE_BUSY_SNAPSHOT` immediately. This keeps the "always `BEGIN IMMEDIATE`" rule from regressing.
3. One transaction held open for 6 s. The other writer fails at about 5 s with "database is locked", and integrity still holds.
4. Two processes claim one lane. One event, one winner.

The four lane scripts, each a file the fake agent executes:

1. A `code_pr` script reaches `ready`.
2. A verdict sha that is not a prefix of the head records `gate.denied` / `verdict_on_head` and the phase stays put. Same facts as the R7 fixture, through process spawn.
3. A docs lane records plan check skipped, reason `docs`.
4. A clean exit writes one `session.exited` and the supervisor starts no second process.

daisy-core's 90 tests in the modules R6 counted join the unit layer when those modules move. They do not stand in for crash or concurrency.

Branch coverage is gated on two modules only: the write function and the predicate evaluator. CI fails if llvm-cov reports a missed branch in either. SQLite keeps 100% MC/DC on its core and says that cost is justified for a library that remembers mistakes, and is probably not cost-effective for a typical application ([testing.html](https://www.sqlite.org/testing.html) §7.7). The write path and the evaluator are the part of this program that remembers. The CLI crate is reported and not gated.

### Performance budgets

Reference machine: Apple M-series, local APFS, warm page cache, the release profile below. Startup method matches R6: 8 unmeasured spawns, then 100 samples, wall time around process spawn. Throughput is timed inside the process, around the loop only, so startup is not hidden inside the commits/s number. RSS is maximum resident set size of that process.

R6 on this machine, 2026-11-02, release, thin LTO: warm ping p50 **3.9 ms**, open-and-exit p50 **4.5 ms**, idle RSS **1.6 MiB**, binary **2.1 MiB**. A simpler autocommit `INSERT` at `synchronous=FULL` did about **19,000 rows/s**. R5's harder shape (50 writers, `BEGIN IMMEDIATE`, insert plus fold, `synchronous=FULL`) did **2,060 commits/s** in **0.971 s**. The product floors sit just above those measurements, with room for clap and the real write function. A miss on the first real binary updates this table in a commit that shows the new measurement.

| Id | What | Product (reference machine) | CI ceiling (GitHub-hosted) |
| --- | --- | --- | --- |
| S1 | `chaperone --version`, warm | p50 ≤ **10 ms**, p99 ≤ **25 ms** | p50 ≤ **100 ms** |
| S2 | Open the WAL file, `SELECT 1`, exit | p50 ≤ **15 ms**, p99 ≤ **30 ms** | p50 ≤ **150 ms** |
| E1 | One writer, `synchronous=FULL`, `BEGIN IMMEDIATE`, one event + fold, 256-byte payload | ≥ **2,000 commits/s** (mean ≤ **0.5 ms**) | 1,000 commits finish in ≤ **10 s** |
| E2 | 50 processes, 40 events, same shape | ≤ **5 s**, and the correctness checks in the concurrency row | ≤ **15 s**, same correctness |
| L1 | A follower observes a committed event id | p50 ≤ **5 ms**, p99 ≤ **20 ms**, over 1,000 events | p99 ≤ **50 ms** |
| M1 | One-shot CLI RSS | ≤ **8 MiB** | ≤ **32 MiB** |
| M4 | `chaperone impact` with a cold cache (every Rust file parsed), on this repo | ≤ **12 MiB** peak RSS (2026-11-07: 10.9 MiB; warm, every file cached, 6.9 MiB, under M1) | Not in CI |
| M2 | Supervisor RSS, zero lanes | ≤ **16 MiB** | ≤ **48 MiB** |
| M3 | Supervisor RSS per idle lane, children excluded | ≤ **256 KiB** (50 lanes ≤ **12.5 MiB** above M2) | ≤ **1 MiB** per lane |
| B1 | Stripped release binary | ≤ **15 MiB**. On macOS the file links system libraries only: `libSystem`, and `libiconv`, which the `libc` crate links on Apple targets (v0: 3.7 MiB, [perf review](../perf/v0-review.md)). SQLite is inside. | Same check, on the release workflow |
| I1 | Instruction count vs the base branch | CI only | ≤ **+10%** on three benches: semantic hash of `pipelines/v17.toml`, `to_ready` on the R7 fixture, 100 append-and-fold calls on a memory VFS with `synchronous=OFF` |
| U1 | `chaperone-ui` start to the first frame that shows the board (`--first-frame`) | p50 ≤ **300 ms** (2026-11-07: p50 236 ms, max 290 ms, 20 samples after 3 warm) | Not in CI: runners have no display |
| U2 | `chaperone-ui`: another process commits, the watcher asks for a repaint; 50 lanes × 200 events, a spec of 20 decisions, a question per lane, one lane selected | p99 ≤ **150 ms** (2026-11-08, #41: 126 to 142 ms over three runs, the 100 ms poll plus a 20 ms board, feed and lane detail reload; 123 ms with the board alone) | Not in CI |
| U3 | `chaperone-ui` CPU, window open, ledger quiet, 50 s | ≤ **0.5%** of one core (0.06%) | Not in CI |
| U4 | `chaperone-ui` max RSS | ≤ **128 MiB** (116 MiB at start, 78 MiB steady; the GL context and fonts, not the board) | Not in CI |

M4 is above M1 because a cold `impact` holds one tree-sitter tree at a time, about 45 bytes per source byte, so its peak follows the largest file it parses (here `work.rs`, 62 KB), not the file count; the 256 KiB parse cap bounds the worst case near 17 MiB, inside M1's CI ceiling ([ADR 0043](../decisions/0043-code-graph-cache.md), [R43](r43-change-graph.md)). A warm call stays under M1.

L1 is the budget that forces a wake. A 50 ms poll fails the product p50. The 300 s orchestrate poll fails it by a wide margin. M3 is the budget that matches R8: the supervisor keeps no scrollback and no transcript. A 1 MiB buffer per lane fails at 50 lanes on the product number (50 × 256 KiB). Agent CLI RSS is outside these budgets. R6 already measured that the agent, not the harness, dominates per-lane memory.

U1 to U4 are the overview window's (#40), measured by hand on the reference machine: U1 with `--first-frame` against a seeded ledger, U2 by the ignored `commit_to_redraw` test in `crates/chaperone-ui/src/watch.rs` (`cargo test --release -p chaperone-ui -- --ignored --nocapture`), U3 and U4 with `ps`. The window polls `PRAGMA data_version` every 100 ms and reloads only when it changes, so it is not an L1 follower. On a ledger that changes every poll, it reloads at most once per poll: at 50 lanes × 200 events that is about 15% of one core while the writes last.

I1 runs on Linux with [gungraun](https://github.com/tokio-rs/tokio/pull/7779) (the iai-callgrind successor) under valgrind. It is a required PR check. S1 through M3 on the reference machine run before a tag, and the measured p50 is copied into the release notes next to the budget. CI enforces the right-hand column on every PR, in a release build, on `ubuntu-latest` and `macos-latest`.

Release profile, so the binary CI ships is the binary the budget describes:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

Debug tests are not subject to S1, S2, M1, or B1.

## Repo layout

```
Cargo.toml                 workspace
rust-toolchain.toml        channel pinned, see below
deny.toml
rustfmt.toml
CHANGELOG.md               first release creates it; every release updates it
LICENSE-MIT                first commit that contains code (ADR 0001)
LICENSE-APACHE
CONTRIBUTING.md
crates/chaperone-core/      ledger, engine, migrations, deadline-bounded git and daisy-cli calls. No clap, no PTY, no home directory.
crates/chaperone-cli/       the `chaperone` binary. CLI, supervise, in-tree adapters.
crates/chaperone-fake/      scripted agent. publish = false. Not installed by `cargo install`.
crates/chaperone-graph/     the code graph behind `impact`: tree-sitter facts, module map, cache (ADR 0043).
crates/chaperone-ui/        the `chaperone-ui` window. Read-only, the only crate with a GUI dependency.
pipelines/                 v17.toml and later ids. Loaded from disk.
docs/                      vision, research, decisions, events.md, the CLI contract
.github/workflows/ci.yml
.config/nextest.toml
```

`chaperone-core` is the library jj splits out so the UI is not inside the engine ([jj architecture](https://docs.jj-vcs.dev/latest/technical/architecture/): library crate does not talk to the terminal). Daisy reads SQL views, not this crate ([R5](r5-state-and-storage.md)), so the crate is `publish = false` until a later decision says otherwise. [ADR 0001](../decisions/0001-name-license-oss-posture.md) already forbids an empty crates.io placeholder.

`pipelines/` stays data. R7's reason for TOML is that two versions load in one process and a diff names the gate. Compiling the document into the binary would make the version the version of the binary.

The fake is a workspace binary so `cargo install --path crates/chaperone-cli` does not ship it. Lane tests spawn it the way a lane spawns Claude: `chaperone` takes the agent argv. That flag is the R4 spawn contract, and the fake is one argv. There is no test-only backdoor beside it.

No `xtask` crate until a second custom command exists. The perf checks are a `cargo test --release --test budgets` binary and a gungraun bench.

Toolchain pin: **Rust 1.95**, the compiler R6 measured, in `rust-toolchain.toml` and `package.rust-version`. A bump is its own commit. jj currently tracks "stable minus one" (1.97.1 in their tree as of this note) and formats with nightly rustfmt. A new binary with no downstream compilers can pin the compiler it was measured with and format with stable rustfmt. ripgrep's CI pins a known compiler and also builds stable; we add a stable build when we bump, and until then the pin and stable are the same job.

## Fake agent

`chaperone-fake --script PATH` reads a script and speaks the R4 lifecycle on stdout: one JSON object per line, `session.started`, `turn.finished`, `session.exited`, plus the optional `usage.reported` when the script includes it. A script step may name an MCP tool call (`lane.read`, `lane.message`, `evidence.attach`). The process has no network and does not read the clock. Timestamps in the events are the timestamps in the script.

R8: a test lane forks this process and skips the attach socket. The PTY smoke is a later unix-only test, outside the 3-minute job. The engine fixture for `verdict_on_head` stays in-process and does not start the fake. R7 already said that refusal needs no CLI.

Crash and concurrency tests set nextest `threads-required` so they do not overlap. Fifty writers plus a second 50-writer test in parallel is how a correct run becomes a timeout.

## What we take from projects that already do this

| Project | Practice this bar uses |
| --- | --- |
| **SQLite** | Crash tests spawn a child and then run `integrity_check`. The parent checks that the child's work committed or vanished ([testing.html](https://www.sqlite.org/testing.html) §3.3). `mptester` is the multi-process stress test (§2.6). `veryquick` is the few-minute run before a check-in; the soak is before a release (§2). `speedtest1` is kept as its own program, not mixed into correctness. |
| **TigerBeetle** | The simulator seed can be the git hash, so CI explores a new space and a failure replays from the commit ([ci smoke PR](https://github.com/tigerbeetle/tigerbeetle/pull/1277), [VOPR](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/internals/vopr.md)). Assertions on storage invariants stay on in production builds. Their style guide's rule that matters here: an assertion failure is a bug, and the process stops ([TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md)). |
| **jj** | Workspace, library without terminal I/O, CLI on top. `cargo nextest`, `clippy --all-targets -- -D warnings`, `cargo deny`, proptest, MSRV in `Cargo.toml`, 0.x while the interface moves, `CHANGELOG.md`. |
| **ripgrep** | Integration tests run the built binary. The release tag has to match `Cargo.toml`. CI pins the cross version and the compiler. Stable rustfmt. |
| **Tokio** | Loom is how they check shared-memory interleavings, and they run it under tight caps (`LOOM_MAX_PREEMPTIONS=1`). Wall-clock criterion is not a CI merge gate. |

Power loss in the middle of `fsync` stays SQLite's promise under WAL and `synchronous=FULL`, which is the limit R5 already recorded. On macOS that promise is weaker: plain `fsync` does not flush the drive cache, so a power loss or OS crash can lose returned commits or corrupt the database file during a checkpoint ([ADR 0006](../decisions/0006-plain-fsync-on-macos.md)). We do not ship a VFS that reorders unsynchronized writes. TH3-scale MC/DC, a cluster simulator, nightly rustfmt, ripgrep's cross matrix, and a two-assertions-per-function quota are outside this bar. SQLite disables its own asserts in production because they make the engine about three times slower (§8.1). Our commit is fsync-bound (R6, about 13k to 19k rows/s). The fold invariants stay enabled in release. The rest of the CLI does not grow a TigerStyle quota.

Loom is the wrong model for "50 processes, one SQLite file". It belongs in a later PR if an in-process broadcast grows shared mutable state. That PR adds a `cfg(loom)` test with Tokio's caps. It is not part of the default PR job.

## CI plan

Required on every pull request. `fail-fast` is off so a fmt failure does not hide a test failure.

| Job | Runs on | Command | Blocks |
| --- | --- | --- | --- |
| `fmt` | `ubuntu-latest` | `cargo fmt --all -- --check` on the pinned stable | Yes |
| `clippy` | `ubuntu-latest` | `cargo clippy --workspace --all-targets -- -D warnings` | Yes |
| `test` | `ubuntu-latest`, `macos-latest` | `cargo nextest run --workspace` | Yes. Timeout 10 min. Expected under 3 min. |
| `coverage` | `ubuntu-latest` | llvm-cov, fail on a missed branch in `write` and `eval` | Yes |
| `doc` | `ubuntu-latest` | `RUSTDOCFLAGS="-D warnings" cargo doc -p chaperone-core --no-deps` | Yes |
| `deny` | `ubuntu-latest` | `cargo deny check` | Yes |
| `budgets` | `ubuntu-latest`, `macos-latest` | release build, the CI column of the budget table | Yes. Timeout 20 min. |
| `regress` | `ubuntu-latest` | gungraun, I1, ≤ +10% instructions vs the base branch | Yes |

Windows is not a required target. R8 left it out of scope. `portable-pty` speaks ConPTY, and a Windows job can be added when a lane runs there.

The tag workflow, on `v*` tags only:

1. The tag equals the workspace `version`. ripgrep's release workflow fails closed on a mismatch.
2. Property tests at 10,000 cases, seed still the commit sha.
3. Build `aarch64-apple-darwin` and `x86_64-unknown-linux-musl`, stripped, SQLite bundled. `otool -L` / `ldd` must show the R6 link set: `libSystem` on macOS, the musl loader on Linux.
4. B1, the 15 MiB ceiling.
5. GitHub release attaches those two archives. Further targets wait until someone needs them.

`cargo publish` is not a job. ADR 0001 reserves crates.io for a real release, and the library crate stays private.

Licenses `cargo deny` accepts: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, MPL-2.0, Unicode-3.0, Zlib, and the Rust dual license. An unknown license fails the job. Advisories fail the job. MPL-2.0 is on the list because it shows up in Rust trees; a new MPL dependency still needs the dependency note below, since R13 keeps third-party code inside the one binary we build, not a downloaded plugin.

## Versioning and release

Five versions, because they move for different reasons.

| Axis | Form | A breaking change is | Compatibility |
| --- | --- | --- | --- |
| Binary | Semver. **0.y.z** until the integration contract and the event schema are stable ([ADR 0001](../decisions/0001-name-license-oss-posture.md)). **1.0** is vision milestone 8. | A CLI flag, an exit code, or the on-disk ledger path. | During 0.y, a **minor** bump may break the CLI. A **patch** bump must not. Cargo's usual 0.x rule is looser than this. Skills pin a minor (R15). |
| Ledger schema | `PRAGMA user_version` | A migration. | An older binary refuses to write. The newer binary is the only migrator. `VACUUM INTO` before the upgrade ([R5](r5-state-and-storage.md)). |
| Reader contract | `reader_contract.version` | A column daisy's views expose. | Daisy errors on a version it does not know. A new base-table column that no view selects does not bump it. |
| Integration contract | Integer on the lifecycle envelope and the MCP server. Start at **0** (R4's v0). | A removed or renamed field or tool. | A new nullable field does not bump. An adapter records the version it speaks. |
| Pipeline | `schema = 1`, plus an immutable id and a semantic hash. | The TOML schema itself, which is a harness release. | A new pipeline id is a new file. Lanes already stamped stay on their hash ([R7](r7-process-engine.md)). |

`CHANGELOG.md` follows the releases, one section per tag, user-facing lines. Commit messages stay short and plain. The changelog is the place a CLI break is announced.

Before a tag, on the reference machine: run the product column, paste the measured S1, E1, and M1 into the release notes, and confirm each is inside its budget. A miss stops the tag, or updates this table in the same release with the measurement attached.

## Docs

Public interfaces are documented in the same PR that adds them ([VISION](../VISION.md) quality bar). The mechanical checks:

- `pipelines/*.toml` is the pipeline. The engine loads that file. A second copy compiled into the binary fails the "two ids, one process" unit test.
- `docs/events.md` lists every event kind. A unit test parses the kind column and compares it to the enum.
- `cargo doc -D warnings` covers public items in `chaperone-core`.
- CLI help is the CLI doc. One test runs `chaperone --help` and compares it to a committed snapshot, so a flag change is visible in review.

Decision records stay in `docs/decisions/`. Research notes stay the evidence. This note is the evidence for the bar. An ADR can cite it when the bar is accepted. The first code PR can follow `CONTRIBUTING.md` without waiting on that ADR.

A user guide (mdBook or equivalent) waits until there is a user besides the author. jj has a book because the tool is a VCS people learn. Milestone 1's docs are the CLI help, the event list, the pipeline file, and the ADRs.

## Contribution rules

The CONTRIBUTING.md at the repo root is the short copy. The rules that are easy to lose:

- A PR that changes `write` or `eval` keeps both at 100% branch coverage.
- A PR that changes a budget updates the table in this note and shows the measurement.
- A new dependency is named in the PR, with why it is there and what it pulls in. `cargo deny` has to pass. R13's rule still holds: adapters are code in this repo, not a crate downloaded at lane start.
- Commit subjects are short and plain. No attribution trailer.
- License is `MIT OR Apache-2.0`. Contributions are under both, the usual Rust grant. The `LICENSE-*` files land in the first commit that contains code.

## What this leaves open

- **R15.** Which of today's `ship-handoff` checks become the first lane scripts, and which stay in daisy until the move. The four scripts above are the minimum, not the migration.
- **Windows CI.** When a lane runs there.
- **Loom.** Only if an in-process broadcast acquires shared mutable state.
- **A hosted perf runner.** If the reference-machine column needs to block a PR without a laptop, a self-hosted Apple Silicon runner can enforce the product column directly. Until then, the tag checklist is that measurement.
- **Fuzzing the parser.** SQLite's lesson that fuzzing and full branch coverage pull apart (§4.1.6) is real, and a `cargo-fuzz` target on the pipeline parser is worth a later job. It is not a PR gate. The 10,000-case property run on the tag is the soak we can afford now.

## Sources

- This repo: [VISION.md](../VISION.md) quality bar (property, crash, concurrency, fake agent, budgets, versioned interfaces). [R5](r5-state-and-storage.md) write function, crash probe, 50-writer measurement, `BEGIN IMMEDIATE`, `busy_timeout` 5000. [R6](r6-language-and-runtime.md) startup, RSS, binary size, FULL-sync throughput, release flags, rustc 1.95.0. [R7](r7-process-engine.md) fixture, semantic hash, two documents in one process. [R8](r8-process-terminal-runtime.md) fake agent skips the socket, no scrollback in the supervisor, herdr at 24 MiB. [R4](r4-agent-cli-integration.md) lifecycle events and the three MCP tools. [R13](r13-security-and-trust.md) redaction before the idempotency key, one fixture per pattern. [ADR 0001](../decisions/0001-name-license-oss-posture.md) dual license, 0.x, no empty crates.io publish.
- SQLite: [How SQLite Is Tested](https://www.sqlite.org/testing.html) (harnesses, `mptester`, crash testing §3.3, coverage §7.7, asserts §8.1).
- TigerBeetle: [VOPR](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/internals/vopr.md), [CI uses the commit hash as the seed](https://github.com/tigerbeetle/tigerbeetle/pull/1277), [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md).
- jj: [contributing](https://docs.jj-vcs.dev/latest/contributing/) (nextest, clippy, MSRV), [architecture](https://docs.jj-vcs.dev/latest/technical/architecture/) (library vs CLI), workspace `Cargo.toml` (proptest, `deny.toml` in tree).
- ripgrep: [CI](https://github.com/BurntSushi/ripgrep/blob/master/.github/workflows/ci.yml), [release](https://github.com/BurntSushi/ripgrep/blob/master/.github/workflows/release.yml) (tag matches the crate version, pinned cross).
- Tokio: [pull-request guide](https://github.com/tokio-rs/tokio/blob/master/docs/contributing/pull-requests.md) (loom invocation), [tokio#7779](https://github.com/tokio-rs/tokio/pull/7779) (instruction counts in CI). [Criterion FAQ](https://bheisler.github.io/criterion.rs/book/faq.html) on hosted-runner noise.

## Decision outputs (for `docs/decisions/`)

1. Workspace layout above. Pipelines stay files. The fake agent is a `publish = false` binary.
2. PR CI is the eight jobs in the CI plan. Tags build two targets and refuse a tag that mismatches `version`.
3. The pyramid and the budget table are the bar. Product numbers are the reference machine. CI numbers are the coarse ceilings plus a 10% instruction regression.
4. Five version axes, with patch releases CLI-stable during 0.y.
5. `CONTRIBUTING.md` is the contributor copy of this note.
