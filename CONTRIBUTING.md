# Contributing

The bar and the evidence are in
[docs/research/r14-engineering-standards.md](docs/research/r14-engineering-standards.md).
This file is the short version a pull request has to meet. These rules apply to
every PR.

## License

Chaperone is dual-licensed under MIT and Apache-2.0 (`MIT OR Apache-2.0`).
By contributing, you agree that your contribution is available under both
licenses. The texts are in `LICENSE-MIT` and `LICENSE-APACHE`. See
[ADR 0001](docs/decisions/0001-name-license-oss-posture.md).

## Setup

Install the Rust toolchain named in `rust-toolchain.toml` (1.95, the compiler
the startup numbers were measured with). From the repo root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

`cargo test --workspace` is acceptable when nextest is not installed. CI runs
nextest. Format with the stable `rustfmt` from that toolchain.

Release builds use thin LTO, `codegen-units = 1`, and stripped symbols. Debug
builds are for the test job. Startup and binary-size budgets apply to release
builds only.

## Tests a change has to bring

| You touched | The PR includes |
| --- | --- |
| The write function or the predicate evaluator | Branch coverage of that module stays at 100%. The rebuild property still passes. |
| A ledger write path | The crash tests (`SIGKILL` before commit, `SIGKILL` in the commit loop) and the 50-process concurrency test, against that write function. |
| A gate or a pipeline document | A unit pass and a unit fail for the predicate. A validation or CAS refusal writes nothing; a gate denial appends one `gate.denied` and leaves the lane where it was. |
| Spawn, supervise, or the agent argv | A script under the fake agent (`crates/chaperone-fake`) that exits cleanly, and the kill test that records `session.exited` once and does not respawn. |
| An event kind | `docs/events.md` and the enum updated together. The drift test passes. |
| A CLI flag | The `--help` snapshot updated in the same commit. |
| A performance budget | The table in the R14 note updated, with the measurement in the PR. |

The five layers and their time limits:

| Layer | PR budget |
| --- | --- |
| Unit | under 10 s, each test under 50 ms |
| Property (7 properties, 256 cases, seed = commit sha) | under 30 s |
| Crash (3 tests) | under 20 s |
| Concurrency (4 tests, one of them 50 processes × 40 events) | under 30 s, 60 s timeout each |
| Lane (fake agent scripts, each under 2 s, cap 20) | under 30 s |

The whole test job stays under 3 minutes. Crash and concurrency tests do not
run beside each other (nextest `threads-required`).

Property tests pass the clock in. The write function reads the clock only
when none is passed (a CLI command), once the write lock is held. A failing
property prints its seed. Re-run that seed before changing the assertion.

The in-process `verdict_on_head` fixture and a lane script that hits the same
refusal are both required once the engine and the supervisor exist. The
fixture does not need the fake agent. The script does.

## CI plan

Required on every pull request. A red job is a failed PR. `regress` waits on
its bench dependency (#56); the other jobs run now.

| Job | What it runs |
| --- | --- |
| `fmt` | `cargo fmt --all -- --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo nextest run --workspace` on Ubuntu and macOS (`test (linux)`, `test (macos)`). Timeout 10 min. |
| `coverage` | llvm-cov. A missed branch or region in `eval`, ledger `write` or `fold` fails the job. In `write` and `fold` a region that is only a `?` is exempt: most propagate a SQLite error no test reaches without fault injection, and the rest propagate a refusal whose own check is gated. |
| `doc` | `cargo doc -p chaperone-core --no-deps` with warnings denied. |
| `deny` | `cargo deny check` (licenses, advisories, bans, sources). |
| `budgets` | Release build. Coarse ceilings: `--version` p50 ≤ 100 ms, open-and-exit p50 ≤ 150 ms, 1,000 full-sync commits ≤ 10 s, one-shot RSS ≤ 32 MiB, on Ubuntu and macOS (`budgets (linux)`, `budgets (macos)`). Follower p99 ≤ 50 ms joins when the follower exists. Timeout 20 min. |
| `regress` | Linux instruction counts, ≤ +10% against the base branch, on the pipeline hash, `to_ready`, and 100 in-memory appends. |

Windows is not required. A tag (`v*`) is a separate workflow: the tag matches
`Cargo.toml`, property tests run 10,000 cases, and the workflow publishes two
stripped binaries (`aarch64-apple-darwin`, `x86_64-unknown-linux-musl`) under
15 MiB, SQLite bundled. `cargo publish` is not part of CI.

Product budgets (reference machine, not the hosted runner) live in the R14
note. The ones a release has to meet: startup p50 ≤ 10 ms, one writer ≥ 2,000
commits/s, follower p50 ≤ 5 ms, one-shot RSS ≤ 8 MiB, ≤ 256 KiB per idle lane
in the supervisor. Paste the measured figures into the release notes.

## Versions

Four axes, so a pipeline tweak is not a binary release and a migration is not
a silent CLI break.

| Axis | Rule |
| --- | --- |
| Binary `0.y.z` | A minor bump may break CLI flags. A patch bump must not. 1.0 is the vision's milestone 8, not the first public commit. |
| `PRAGMA user_version` | Older binaries refuse to write. The newer binary is the only migrator. |
| Integration contract | Starts at 0, shared with the event schema ([docs/events.md](docs/events.md)). A removed or renamed field, kind or tool bumps it. A new nullable field does not. |
| Pipeline id + hash | A published id is immutable. New claims can move. Running lanes stay on the hash they stamped. |

## Docs and decisions

Document a public interface in the same PR that adds it: event kinds in
`docs/events.md`, the pipeline in `pipelines/`, CLI flags in `--help`.

An architectural choice that picks among real alternatives gets a file in
`docs/decisions/`, with the alternatives and the evidence. Research notes in
`docs/research/` stay the evidence. Link them. Don't paste them into the ADR.

A new ADR or research note takes the number of the issue it comes from, not the
next free one: `docs/decisions/0080-<slug>.md`, `docs/research/r80-<slug>.md`.
CI checks it against the branch name. Migrations stay sequential: two lanes that
add one conflict on `MIGRATIONS`, and the second to merge renumbers
([ADR 0080](docs/decisions/0080-numbered-files-take-the-issue-number.md)).

## Dependencies

Name a new dependency in the PR: what it is for, and what licenses it pulls in.
`cargo deny` has to pass. Adapters ship in this repo. A lane does not download
code at start.

## Commits

Short subject, plain language, one change. No attribution trailer and no
`Co-Authored-By`.
