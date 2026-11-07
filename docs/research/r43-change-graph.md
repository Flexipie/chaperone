# R43: The change graph

Status: 2026-11-07. Issue [#43](https://github.com/Flexipie/chaperone/issues/43), part of
[#39](https://github.com/Flexipie/chaperone/issues/39). Decision: [ADR 0043](../decisions/0043-code-graph-cache.md).

## Question

What does a lane's change reach that its diff does not show, and how does Chaperone answer
that cheaply enough to call before every start and every ready, from any worktree?

## Sources

The owner's notes of 2026-11-04 (GitNexus, naiss `/screens`) are not in the repo. This
note works from the decisions recorded on #39 that day:

- Every view is a ledger or CLI query first (`overlap`, `impact`), drawn by the UI second,
  so agents, reviewers and the orchestrator use the same data.
- Code-graph data lives in its own rebuildable SQLite cache keyed by repo and file content
  hash, not in the ledger. Lane facts stay in the ledger.
- Languages first: Rust and TypeScript (TypeScript is #93).
- GitNexus is PolyForm Noncommercial: borrow ideas only, never depend on it. The ideas
  taken: impact grouped by depth with exact vs lower-bound, staleness on every answer, one
  index per origin shared by worktrees.
- Later, maybe: a runtime cost per node (naiss `/screens`), backend functions attributed
  to screens, clusters and execution flows.

## Recommendation

**Module-level, not symbol-level.** A node is a file with its module path
(`chaperone_core::ledger::write`); an edge says one file names another's module. That is
what tree-sitter gives without type checking, it is right far more often than it is
wrong for "what might this break", and it can say when it does not know.

**Edges, Rust.** From each crate root (lib, bins, `src/bin/*`, `tests/*`, `examples/*`,
`benches/*`, `build.rs`, from each package's `Cargo.toml`) follow `mod name;` to `name.rs` or `name/mod.rs` to get
the module map. Then a file depends on the longest module prefix of each path it writes:
`use` trees, `a::b` paths in code and in macro arguments, rooted at `crate`, `self`,
`super`, a child module, a name its `use` brought in, a module a glob brought in, or a
workspace crate's name. Three edges the paths miss:

- `impl T` in one file adds methods to `T`, defined in another. Callers name only `T`, so
  `T`'s module depends on the `impl` file. Without it, `impact spec/slice.rs` missed
  `chaperone-cli/src/spec.rs`, which calls `ledger.spec_slice(...)`.
- `macro_rules!` macros are named without a path: a file invoking `name!` depends on each
  file of its crate that defines `macro_rules! name`, and a `#[macro_export]` macro's file
  is a dependency of its crate root, where `crate::name!` and `other_crate::name!` land.
- Integration tests run the package's binaries (`CARGO_BIN_EXE_*`), so `tests/*.rs`
  depends on each bin root.

`mod` declarations are not dependency edges: a parent that only declares a child is not
changed by it; one that re-exports it has a `pub use` edge.

**Exact or lower bound.** The answer is `exact` only when every Rust file was read and
understood. A file not read (unreadable, over 256 KiB), with a parse error, nesting past
256 levels (`too_deep`), a `#[path]` module, `include!`, `super` above the crate root, or
reached from no crate root makes the whole answer `lower_bound`, and is listed in `unsure`
with why. Its references could point
anywhere in the workspace through crate names, so narrowing the bound to a crate or a
depth would claim more than is known. Dynamic dispatch through `dyn Trait`, macro-made
modules and renamed dependencies are not seen; those are the known gaps of a module graph.
Macros brought in by `#[macro_use] extern crate` get no edge. An `impl` of a workspace trait for a foreign type (`impl LocalTrait for String`) gets no edge to the trait's users either.

**Staleness.** Every call lists the tree from git and reads changed and untracked files
from the worktree, so the answer is for the worktree as it is now. `stale` is true when
some file of it had no facts (could not be read). There is no index that can fall behind.

**Cache.** [ADR 0043](../decisions/0043-code-graph-cache.md): facts by content key in one
SQLite file per origin; edges rebuilt in memory per call. A clean file is keyed by its
blob id but read from the worktree, so on a miss `git hash-object` checks the bytes read
still hash to it, and ones that do not (an edit since, `skip-worktree`) are keyed by their
sha256, never stored under a blob id they do not match.

## Measured

On this repo (57 Rust files), Apple M-series, warm page cache.

Method: R14's (8 unmeasured spawns, then the samples, wall time around the spawn), target
`crates/chaperone-core/src/ledger/write.rs`, release builds of `1f2687d` (main) and this
branch, `--cache` in a temp dir.

Two runs: the first on a quiet machine before the review fixes, the second after them
(`hash-object` on cache misses, macro and turbofish facts) with a load average of 8 to 10
from other lanes, which slows `--version` on main by about a third too.

| | main | branch, quiet | branch, after fixes, loaded |
| --- | --- | --- | --- |
| `impact`, cold (cache file removed before each call: list, parse 57 files, check blob ids, write, resolve), p50 / max of 30 | | 97 / 110 ms | 171 / 185 ms |
| `impact`, warm (every file cached), p50 / p99 of 100 | | 31.6 / 37.5 ms | 42.5 / 55.8 ms |
| `impact` RSS, cold / warm | | 9.9 / 6.0 MiB | 10.7 / 6.3 MiB |
| `--version` p50 / p99 (S1) | 3.1 / 3.7 ms quiet, 4.2 / 6.3 ms loaded | 3.2 / 4.0 ms | 4.4 / 7.4 ms |
| stripped binary (B1) | 4.23 MiB | 5.57 MiB | 5.60 MiB |

The `hash-object` check on misses costs about 30 ms for 57 files (one call); a warm call
makes none.
Most of the warm time is git: four calls of 5 to 9 ms each, run at once. A second
worktree at a commit 8 behind reused 28 of 47 files' facts and parsed 19.

**Memory.** A parse holds about 45 bytes of tree per byte of source, so a call's peak
RSS follows the largest file it parses, not the file count: a one-file repo holding
`engine.rs` (4 KB) peaks at 5.9 MiB, holding `ledger/write.rs` (62 KB) at 8.5 MiB. A cold
call over this repo is 9.9 to 10.7 MiB, past M1's 8 MiB product budget; a warm one, the usual
case once a lane has called it, is 6.0 to 6.3 MiB. Files over 256 KiB are not parsed (they make
the answer `lower_bound`), which caps the peak near 17 MiB, inside M1's 32 MiB CI
ceiling. R14's M4 row budgets a cold `impact` on this repo at 12 MiB (10.9 MiB measured at the final head, warm 6.9 MiB); a warm call stays under M1's 8 MiB.

## Next

- TypeScript imports and the naiss timing: #93.
- The UI slices (#44 to #48) draw `impact` and `overlap`.
- Symbol-level edges (a call graph) only if module-level answers prove too coarse.
