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

**Edges, Rust.** From each crate root (lib, bins, `src/bin/*`, `tests/*`, `build.rs`,
from each package's `Cargo.toml`) follow `mod name;` to `name.rs` or `name/mod.rs` to get
the module map. Then a file depends on the longest module prefix of each path it writes:
`use` trees, `a::b` paths in code and in macro arguments, rooted at `crate`, `self`,
`super`, a child module, a name its `use` brought in, a module a glob brought in, or a
workspace crate's name. Two edges the paths miss:

- `impl T` in one file adds methods to `T`, defined in another. Callers name only `T`, so
  `T`'s module depends on the `impl` file. Without it, `impact spec/slice.rs` missed
  `chaperone-cli/src/spec.rs`, which calls `ledger.spec_slice(...)`.
- Integration tests run the package's binaries (`CARGO_BIN_EXE_*`), so `tests/*.rs`
  depends on each bin root.

`mod` declarations are not dependency edges: a parent that only declares a child is not
changed by it; one that re-exports it has a `pub use` edge.

**Exact or lower bound.** The answer is `exact` only when every Rust file was read and
understood. A file not read (unreadable, over 1 MiB), with a parse error, a `#[path]`
module, `include!`, `super` above the crate root, or reached from no crate root makes the
whole answer `lower_bound`, and is listed in `unsure` with why. Its references could point
anywhere in the workspace through crate names, so narrowing the bound to a crate or a
depth would claim more than is known. Dynamic dispatch through `dyn Trait`, macro-made
modules and renamed dependencies are not seen; those are the known gaps of a module graph.

**Staleness.** Every call lists the tree from git and reads changed and untracked files
from the worktree, so the answer is for the worktree as it is now. `stale` is true when
some file of it had no facts (could not be read). There is no index that can fall behind.

**Cache.** [ADR 0043](../decisions/0043-code-graph-cache.md): facts by content key in one
SQLite file per origin; edges rebuilt in memory per call.

## Measured

On this repo (57 Rust files), release build, Apple M-series, warm page cache:

TIMINGS

## Next

- TypeScript imports and the naiss timing: #93.
- The UI slices (#44 to #48) draw `impact` and `overlap`.
- Symbol-level edges (a call graph) only if module-level answers prove too coarse.
