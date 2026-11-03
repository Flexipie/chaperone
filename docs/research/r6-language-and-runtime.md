# R6: Language and runtime

Status: recommendation, 2026-11-02. Pick **Rust**. The two factors that decide it are below. This note is the evidence a decision record should cite.

## Pick

**Rust**, for the harness binary (ledger, gates, process supervisor, CLI).

1. **daisy-core can move, not be rewritten.** `work`, `spec`, `jobs`, and the review modules are 12,551 lines of Rust on `rusqlite`, with 90 tests in those files. herdr, the terminal runtime already in use, opens PTYs with `portable-pty`. Go, Bun, and Zig can shell out to `daisy-cli` during migration, and they would reimplement that ledger to take it over.
2. **The hot paths are already in the lead group, so that reuse is free.** On this machine a warm Rust CLI starts in 3.9 ms, tied with Zig and ahead of Go (6.3 ms) and Bun (12.2 ms). Bundled-SQLite autocommit matches Go. The release binary is 2.1 MB and links only `libSystem`, with SQLite inside.

Go is the only close alternative. It ties Rust on SQLite and has the most predictable in-process fan-out. It loses on the ledger we already have. Bun and Zig lose on the "one small self-contained binary" bet in [VISION.md](../VISION.md), and Zig also loses on language stability and the size of the contributor pool.

## What was measured

Apple M2 Max, 12 cores, macOS (Darwin 25.6.0), APFS `/tmp`. Warm page cache. Toolchains installed that day: rustc 1.95.0, Go 1.26.0, Bun 1.4.2, Zig 0.16.0 (upstream stable that week was 0.17.0, 2026-10-01).

One program per language, four subcommands. Release settings: Rust `--release` with thin LTO, `codegen-units = 1`, `strip`; Go `CGO_ENABLED=1` and `-ldflags "-s -w"`; Bun `bun build --compile --minify`; Zig `-Doptimize=ReleaseFast`.

| Path | What it does |
|------|----------------|
| `ping` | Print `ok` and exit. |
| `db PATH` | Open SQLite, `PRAGMA journal_mode=WAL`, `SELECT 1`, exit. |
| `append PATH N SYNC` | New DB, WAL, `synchronous=SYNC`, one autocommit `INSERT` per row. Payload is 256 `x` bytes. Time is inside the process, around the loop only. |
| `fan-inline` | 2,000,000 events, 8 function-pointer callbacks, each adds the index into a `u64`. |
| `fan-thread` | 100,000 events, 8 consumers, bounded queue of 1024. Rust `std::sync::mpsc`, Go channels, Zig `std.Io.Queue` on `Io.Threaded`, Bun `worker_threads`. Workers are started before the timer. |

Startup is wall time around process spawn, 8 warmup runs then 100 samples, from Python. RSS is `maximum resident set size` from `/usr/bin/time -l` on one warm run. Append and fan-out figures are the median of 5 runs.

SQLite builds are not the same, and that matters:

| Language | SQLite | How it is linked |
|----------|--------|------------------|
| Rust | 3.46.0 via `rusqlite` 0.32 `bundled` (same major as daisy) | Inside the binary |
| Go | 3.49.1 via `github.com/mattn/go-sqlite3` v1.14.28 | Inside the binary (cgo) |
| Bun | 3.51.0 | macOS system library, loaded by Bun |
| Zig | 3.51.0 | Dynamic `/usr/lib/libsqlite3.dylib` |

A C loop linked the same way as Zig, and a Python loop on SQLite 3.54.1, separate the library from the language. See SQLite below.

## Results

### Binary

| | Bytes | What it links besides `libSystem` |
|--|------:|-----------------------------------|
| Rust | 2,122,608 | Nothing. SQLite is inside. |
| Go | 3,771,458 | `CoreFoundation`, `libresolv`. SQLite is inside. |
| Bun | 62,226,930 | `libc++`, `libicucore`, `libresolv`. The file is a Bun runtime with the script attached. |
| Zig | 433,024 | `libsqlite3.dylib`. Small because SQLite is not in the file. |

On macOS every one of these is a single Mach-O. "Single static binary" in the vision means a file you can copy that does not need a language runtime or a distro SQLite package. Rust and Go meet that. Zig meets it only after SQLite is vendored (on the order of another megabyte, the size of the amalgamation). Bun meets the "one file" part and misses "small": the compiled hello is 59 MiB. [Bun's own docs](https://bun.com/docs/compiler/executables) say the executable is a copy of the runtime, and that the binary is still too big.

### Startup and memory

Warm start, milliseconds. `db` reopens one already-created file.

| | ping min | ping p50 | db p50 | ping RSS | RSS during 20k inserts, sync OFF |
|--|--------:|---------:|-------:|---------:|---------------------------------:|
| Rust | 3.5 | 3.9 | 4.5 | 1.6 MiB | 5.3 MiB |
| Zig | 3.5 | 3.9 | 5.6 | 1.5 MiB | 8.4 MiB |
| Go | 5.8 | 6.3 | 7.6 | 8.9 MiB | 17.7 MiB |
| Bun | 11.2 | 12.2 | 15.0 | 12.4 MiB | 30.2 MiB |

All four are fast enough that a person cannot tell them apart on one invocation. Agents that call the CLI in a loop can. 12 ms versus 4 ms is about 8 seconds across a thousand calls. Idle RSS matters for a long-lived supervisor more than for a one-shot CLI. Go's runtime is about 9 MiB before it does anything. Bun is about 12 MiB. Rust and Zig are about 1.5 MiB. Per-lane memory will be dominated by the agent CLI (Claude, Codex, Cursor), not by this choice.

### SQLite append

Autocommit, WAL, 256-byte payload. Median of 5. `per_s` is rows per second.

| | OFF, 20,000 rows | NORMAL, 10,000 | FULL, 2,000 |
|--|-----------------:|---------------:|------------:|
| Rust (bundled 3.46) | 180 ms, 111k/s | 122 ms, 82k/s | 108 ms, 19k/s |
| Go (bundled 3.49) | 175 ms, 114k/s | 121 ms, 83k/s | 115 ms, 17k/s |
| Zig (Apple 3.51) | 294 ms, 68k/s | 180 ms, 56k/s | 153 ms, 13k/s |
| Bun (Apple 3.51) | 312 ms, 64k/s | 198 ms, 51k/s | 147 ms, 14k/s |

Controls for the OFF / 20,000 case, same schema and autocommit:

| Control | Time | Library |
|---------|-----:|---------|
| C, `cc -O2 -lsqlite3` | 287 ms, then 295 ms | Apple 3.51.0, same dylib as Zig |
| Python `sqlite3`, `isolation_level = None` | 173 ms | SQLite 3.54.1 |
| Rust, Go | 175 to 180 ms | Bundled amalgamation |

Zig matches C on Apple's library. Bun is within about 6% of that C loop. Rust and Go match a fast amalgamation. The gap is the SQLite build (Apple's library is slower here, likely its locking and `F_FULLFSYNC` compile options), not the language binding. `synchronous=FULL` is fsync-bound on all four, around 13k to 19k committed rows per second. A ledger that commits one event at a time will spend its time in SQLite and the disk, whichever of these languages we pick.

`modernc.org/sqlite` (pure Go, `CGO_ENABLED=0`) was not timed. It is the route to a Go binary with no C compiler. It is a transpiled SQLite and is the one people drop to when cgo hurts cross-compiles. The numbers above are the fast cgo path. Cross-compiling that path needs a C toolchain per target. Rust's `bundled` feature cross-compiles as part of the crate.

### Fan-out

Checksums matched across all four (`15999992000000` inline, `39999600000` threaded). Every event reached every subscriber.

In-process callbacks, 2,000,000 events, 8 listeners, median of 5:

| | ms |
|--|---:|
| Rust | 14.5 |
| Zig | 14.9 |
| Go | 43.6 |
| Bun | 58.4 |

Threaded queues, 100,000 events, 8 consumers. This one is noisy. Median, and the five samples:

| | median | samples (ms) |
|--|-------:|--------------|
| Go | 82 | 77, 80, 82, 90, 92 |
| Rust | 374 | 54, 66, 374, 896, 1195 |
| Zig | 1005 | 486, 832, 1005, 1094, 1240 |
| Bun | 2022 | 1105, 1616, 2022, 2071, 2252 |

Go's scheduler is the stable winner for "many blocking queues". Rust's best run (54 ms) beat Go's median, and its worst run was over a second. `std::sync::mpsc` under eight contended senders is the wrong shape for a hot broadcast. A real harness would use one writer and a broadcast channel (`tokio` broadcast, or a single watcher list). That is an API choice inside Rust, and it is the same shape daisy already has available (`tokio` is a direct dependency of `daisy-core`).

Bun's worker fan-out pays structured-clone and thread startup. The natural Bun design is the inline one: one event loop, listeners as callbacks. 58 ms for 16 million callback fires is fine. It is the wrong tool if each subscriber is a worker.

None of this is the production rate. A lane ledger appends events at human and CI pace, then wakes a handful of waiters. Every language here can do that. The threaded numbers argue for Go if the supervisor is a crowd of blocking tasks. They do not outweigh the ledger we would have to port.

### PTY

| | Library | Notes |
|--|---------|--------|
| Rust | [`portable-pty` 0.9](https://crates.io/crates/portable-pty) (MIT, from WezTerm) | herdr already spawns with it: `herdr-dev/src/pty/backend.rs` calls `native_pty_system().openpty`. POSIX and Windows ConPTY. |
| Go | [`creack/pty`](https://github.com/creack/pty) | The usual Go PTY package. POSIX. Windows is a separate ConPTY path. Not smoked in this pass. |
| Bun | `Bun.Terminal` / `Bun.spawn({ terminal })` | POSIX `openpty` and Windows ConPTY, in the runtime. Smoked here: `bun pty.ts` spawning `/bin/echo hello-pty` printed `hello-pty` and exited 0 on Bun 1.4.2. [`node-pty`](https://github.com/oven-sh/bun/issues/28925) still segfaults under Bun (closed as a duplicate in April 2026). Adapters must use `Bun.Terminal`, not the Node native addon. |
| Zig | [`conduit`](https://github.com/pedronaugusto/conduit) (wants Zig 0.17), [`zigpty`](https://github.com/pithings/zigpty) | Both speak POSIX and ConPTY. Both track a compiler that is still breaking between minor versions. Young compared with `portable-pty`. |

R8 still chooses who owns the PTY (this process, herdr, tmux, zellij). The language pick does not wait on that. The herdr checkout read for this note is AGPL-3.0-or-later (`herdr-dev` at v0.6.8). That license is stale: R8 found installed herdr 0.9.5 is Apache-2.0, relicensed on 2026-07-22. Depending on `portable-pty` itself is MIT either way.

### Adapters and plugins

Day-one adapters are in-tree: Claude Code, Codex, Cursor, and the contract from R4. A dynamic plugin ABI is not required, and R13 will treat third-party code in the supervisor as a supply-chain risk.

| | Out-of-tree code |
|--|------------------|
| TypeScript | Best fit. Dynamic `import` is the ecosystem default. |
| Go | `plugin` is cgo, Linux-oriented, and rarely used. In-tree packages or subprocesses are the real options. |
| Rust | No stable Rust ABI for dylibs. In-tree crates, WASM, or a subprocess. |
| Zig | C ABI or a vendored package. Small ecosystem. |

Subprocess adapters work in all four. TypeScript wins only if outside contributors ship adapters as npm packages. That is a later goal, and it can be a small Node sidecar without making the supervisor TypeScript.

### Contributor pool

Stack Overflow, "extensive development work in the past year":

| | 2025 survey | 2026 timeseries (n = 14,099) |
|--|------------:|-----------------------------:|
| TypeScript | 43.6% | 43.8% |
| Rust | 14.9% | 18.0% |
| Go | 16.4% | 16.5% |
| Zig | 2.1% | (row truncated in the source we read; 2025 is the last full figure) |

Sources: [2025 survey](https://survey.stackoverflow.co/2025/technology), [2026 language timeseries](https://survey.stackoverflow.co/2026/technology/data/languages-timeseries). Rust was the most admired language in the 2025 survey (about 72%).

TypeScript has the widest pool, and most of it does not write process supervisors, PTY code, or crash-safe ledgers. Rust and Go are both large enough for an open-source systems CLI. Zig at about 2% is a real constraint if this is meant to take outside contributors. Writing the Zig bench against 0.16 also hit that churn directly: `std.time.nanoTimestamp` is gone, `main` now takes `std.process.Init`, and I/O goes through the new `Io` interface. [0.15.1 removed `async` / `await`](https://ziglang.org/download/0.15.1/release-notes.html). [0.16 put concurrency in `std.Io`](https://ziglang.org/download/0.16.0/release-notes.html): `Io.Threaded` is the feature-complete backend, and the evented, io_uring, and kqueue backends are still experiments. Zig is not 1.0.

## daisy-core, counted

Source: `/Users/flexipie/Desktop/Code/1PORTFOLIO/daisy/src-tauri/crates/daisy-core`, read 2026-11-02. The crate is the data layer. The Tauri app and `daisy-cli` are shells. SQLite access stays inside the crate (`rusqlite` 0.32, `bundled`, via the workspace).

| File | Lines | `#[test]` fns | What it is |
|------|------:|-------------:|------------|
| `src/work.rs` | 3,508 | 20 | Lease, status machine, handoffs, append-only `work_events`, ready rule |
| `src/spec.rs` | 1,357 | 7 | Versioned spec body, slice parser, decisions |
| `src/jobs.rs` | 4,147 | 41 | Job queue and the review-job API (`enqueue_review_job`, `review_view`, head SHA, verdicts) |
| `src/review_outcomes.rs` | 2,403 | 15 | Outcome classification, harvest, quality stats |
| `src/review_missed.rs` | 1,136 | 7 | Missed-finding accounting |
| **Total** | **12,551** | **90** | |

`src/db.rs` is another 635 lines and 40 ordered migrations. `src/agents.rs` is 1,503 lines of daisy's own agent-run records. The harness is not adopting daisy's model loop. The parts that correspond to vision step 1 (ledger and gates) are the five files above.

Reuse has a boundary. `daisy-core` also contains notes, inbox, Linear, GitHub, and Slack. Depending on the whole crate would drag the personal hub schema into the harness, which [VISION.md](../VISION.md) says we are not reimplementing. The move is a source move: state machines, parsers, SQL shape, and tests, onto a new migration series. That move is mechanical in Rust. In Go, TypeScript, or Zig it is a translation, and the tests get rewritten with it.

Until that move, every language can keep calling `daisy-cli` (R15). That keeps lanes running. It does not remove the rewrite. The first milestone's point is the ledger leaving daisy.

## Why the others lose

**Go.** Same SQLite class as Rust on a bundled amalgamation, better and steadier queue fan-out, a single binary around 3.6 MB, startup about 6 ms, idle RSS about 9 MiB. cgo is the tax on cross-compiles. The ledger, the spec parser, and the review queries would be written again. Pick Go if we decide that rewrite is cheaper than living in Rust. The measurements do not say that.

**Bun (TypeScript).** Best adapter ecosystem and the largest contributor pool. `bun:sqlite` is real, and `Bun.Terminal` worked in a smoke test. The compiled binary is 59 MiB, warm start is about 12 ms, idle RSS is about 12 MiB, and worker fan-out is about 2 seconds for a case Go finishes in 80 ms. That fights "one local binary, instant startup". `node-pty` is not usable. A TypeScript supervisor also cannot call `daisy-core`.

**Zig.** Tied with Rust on startup, smallest file on disk, C-speed SQLite on the library it links, and PTY packages exist. The measured binary is not self-contained (`libsqlite3.dylib`). The language is pre-1.0, the standard library I/O rewrite is one release old, and the evented backends are unfinished. The contributor pool is about 2%. There is no path to the 12,551 lines except a port.

## What this does not decide

- **R5 (storage).** These numbers assume SQLite, because that is what daisy uses and what R5 is set up to test. A different store would drop the `rusqlite` strings and keep the Rust state machines. It would not make Go or TypeScript able to import the crate.
- **R8 (who supervises the PTY).** Owning it, embedding herdr, or talking to tmux are all possible from Rust. Current herdr is Apache-2.0 (see R8). That license does not change the language.
- **Tokio versus threads.** The bench used std threads and, for Zig, `Io.Threaded`. daisy-core already depends on Tokio. The supervisor can use a dedicated SQLite writer plus async PTY reads without another language.
- **A pure-Go SQLite port.** Not measured. It would not change the reuse result.

## Sources

- This repo: [VISION.md](../VISION.md) bets 1 and 7 (process as code, one local binary, instant startup). [RESEARCH.md](../RESEARCH.md) R6.
- daisy: `daisy/src-tauri/crates/daisy-core` (`Cargo.toml`, `src/lib.rs`, `src/db.rs`, the five modules above). Workspace `rusqlite` 0.32, feature `bundled`.
- herdr, as read for the PTY path on 2026-11-02: `herdr-dev` at v0.6.8 (`Cargo.toml` license AGPL-3.0-or-later, `portable-pty` 0.9), `herdr-dev/src/pty/backend.rs`. The installed 0.9.5 license is corrected in [r8-process-terminal-runtime.md](r8-process-terminal-runtime.md).
- Measurements: programs compiled and run on this machine on 2026-11-02, method in the section above. SQLite version strings from the bundled headers (`SQLITE_VERSION` in `libsqlite3-sys` 0.30.1 and `mattn/go-sqlite3` v1.14.28), from `bun:sqlite` `sqlite_version()`, and from `sqlite3_libversion()` in the C control.
- Bun binary size and compile model: [Single-file executable](https://bun.com/docs/compiler/executables), [Smaller executables](https://bun.com/docs/compiler/size). PTY: [Bun spawn / Terminal](https://bun.com/docs/runtime/child-process). `node-pty` crash: [oven-sh/bun#28925](https://github.com/oven-sh/bun/issues/28925).
- Zig status: [0.15.1 release notes](https://ziglang.org/download/0.15.1/release-notes.html) (`async`/`await` removed), [0.16.0 release notes](https://ziglang.org/download/0.16.0/release-notes.html) (`std.Io`), [download page](https://ziglang.org/download/) (0.17.0, 2026-10-01). PTY packages: [conduit](https://github.com/pedronaugusto/conduit), [zigpty](https://github.com/pithings/zigpty).
- Contributor pool: [Stack Overflow 2025](https://survey.stackoverflow.co/2025/technology), [2026 language timeseries](https://survey.stackoverflow.co/2026/technology/data/languages-timeseries).
- Rust PTY: [portable-pty on crates.io](https://crates.io/crates/portable-pty). Go SQLite, cgo: [mattn/go-sqlite3](https://github.com/mattn/go-sqlite3). Pure Go alternative, not benchmarked: [modernc.org/sqlite](https://pkg.go.dev/modernc.org/sqlite).
