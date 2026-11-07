# ADR 0043: The code graph lives in its own cache, keyed by content

Status: accepted, 2026-11-07  
Evidence: issue [#43](https://github.com/Flexipie/chaperone/issues/43), the decisions on
[#39](https://github.com/Flexipie/chaperone/issues/39) (2026-11-04), [R43](../research/r43-change-graph.md)

## Context

`chaperone impact <files>` answers "what depends on this change" from module nodes and
import edges read with tree-sitter. Reading every Rust file of a repo on every call costs
a parse per file. The facts a parse gives depend only on the file's bytes. Many lanes run
on one repo at once, each in its own worktree at its own commit, and most of their files
are the same bytes.

The owner decided on #39 that graph data stays out of the ledger and lives in a
rebuildable SQLite cache keyed by repo and file content hash, with one index per origin
shared by worktrees.

## Decision

1. **One SQLite file per origin**, at `<user cache dir>/chaperone/graph/<16 hex of
   sha256(origin URL)>.db` (the realpath of git's common dir when there is no origin).
   Every worktree and clone of one origin shares it. `impact --cache <file>` picks another.
2. **Rows are facts by content key**: `facts_v<N>(key TEXT PRIMARY KEY, facts BLOB)`,
   `WITHOUT ROWID`. The key is git's blob id for a clean tracked file (from
   `git ls-files -s`; on a miss, only if the bytes read still hash to it) and
   `sha256:<hex>` of the bytes for a changed or untracked one. `facts` is what the file
   says (its `mod` declarations, paths, `use` names, `impl` self types), never the resolved
   edges: those depend on the whole tree, so they are rebuilt in memory on each call.
3. **Versioned by table name.** `N` is `FACTS_VERSION` in `crates/chaperone-graph`,
   bumped when the extractor or the grammar changes. A binary never reads another
   version's rows, so two lanes on different builds share the file without deleting each
   other's work.
4. **A cache, never a source of truth.** Any SQLite error answers from a fresh parse and
   says so in `cache.skipped`. A SQLite file SQLite calls corrupt or not a database is
   renamed to `.bad`, never deleted (another process may have it open in WAL mode); any
   other file, an existing empty one, or a SQLite file with a `user_version` or tables of
   its own (the ledger) is never written to. Writes are one
   transaction of `INSERT OR IGNORE`, so two worktrees writing at once agree. Rows grow
   with history; past four times the current tree's file count (at least 1,024), a write
   drops every row not in the current tree, and other worktrees reparse what they lose.

## Alternatives considered

- **Tables in the ledger.** Rejected on #39: the ledger holds lane facts with a write
  function, crash and concurrency tests, and `PRAGMA user_version` migrations. Graph rows
  are derived, rebuilt at will, and many times more numerous; they would make every
  ledger backup and migration carry a cache.
- **No cache, parse every call.** This repo parses in well under a second, but a lane
  calls `impact` often and larger repos (#93) would pay the full parse each time.
- **One cache per worktree.** Simpler to reason about, but N lanes parse the same bytes N
  times and leave N files behind. The content key makes sharing safe without it.
- **Store resolved edges.** Faster queries, but edges depend on the whole tree, so a
  shared store would need one edge set per commit and dirty state. Resolving in memory
  from cached facts took a few milliseconds here (R43).

## Consequences

The cache can be deleted at any time; the next call rebuilds it. It is also what keeps a
call's memory down: a parse holds about 45 bytes of tree per byte of source, so a cold
call over this repo peaks at about 10.9 MiB RSS, past R14's M1 one-shot budget of 8 MiB, and a
warm one, with every file cached, at 6.9 MiB (R43). Files over 256 KiB are not parsed
(the answer is then `lower_bound`), which caps a cold call near 17 MiB. R14 has an M4 row
for a cold `impact`: 12 MiB on this repo. A change to what a parse
extracts must bump `FACTS_VERSION`. The UI slices (#44 to #48) read the graph through
`chaperone-graph`, not the cache file.
