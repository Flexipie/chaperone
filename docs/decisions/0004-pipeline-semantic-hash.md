# ADR 0004: Pipeline semantic hash without RFC 8785

Status: accepted, 2026-11-03  
Amends: [R7](../research/r7-process-engine.md) "Pin, diff, rollback, side by side" step 4  
Code: `crates/chaperone-core/src/pipeline/hash.rs`, pinned by the v19, v20 and v21 hash tests

## Context

A lane stamps a pipeline id and a semantic hash at claim, and `lane-stats` groups
lanes by the hash. R7 says to serialize the flattened document with JSON
Canonicalization (RFC 8785) and take SHA-256. The spec for #1 (2026-11-03) agreed to
hand-roll the canonical form on `serde_json` instead of adding an RFC 8785 crate.

## Decision

The hash is SHA-256, lowercase hex, of this JSON:
`{"gates": [...], "limits": {...}, "transitions": [...]}`.

1. Only `[limits]`, the gates and the transitions are hashed. `schema`, `id`,
   `published`, `[cohort]` and comments are not. (Amended below for schema 2.)
2. Defaults are filled in before hashing (`args = {}`, `requires = []`), `routes` are
   sorted, gates are sorted by id, and transitions by id and then by their canonical
   form. `requires` and `of` keep their order, because the first failing gate is
   behaviour.
3. Object keys are sorted by our writer, not by `serde_json`'s map, whose order
   depends on the `preserve_order` Cargo feature that any crate in the build can turn
   on. No whitespace. Strings and integers are written by `serde_json`.
4. Floats cannot reach the hash: the loader accepts only bools, integers and strings
   as values.

Amended for schema 2 (2026-11-10): a schema 2 document also hashes `"schema": 2`,
`"runners"` (each with its two limits resolved to numbers, `budget_cents` and
`timeout_secs`, and its tools sorted with repeats dropped; from #337 `max_tokens`, absent
on a runner with none, replaces `budget_cents`) and `"checks"` (each with
`prompt_sha256`, the SHA-256 of its prompt file's text). A changed prompt or model is
then a new stamp, so cohorts stay comparable. Schema 1 hashes are unchanged, and a
schema 1 document that carries `[runners]` or `[checks]` does not load.

Amended for lookup by id (2026-11-15, #244): a lane's document is found by its pipeline id
alone, not by id and hash. The stamp is the document's hash at claim. A prompt edited in
place moves the hash for new claims, while a lane in flight keeps its older stamp and runs
the edited prompts, so a cohort by hash can mix the two. The owner chose this over a new id
per prompt edit (v20.1, v20.2), which would keep replay exact.

Amended for v21 (2026-11-22): the id edited in place moves from v20 to v21. v20 is no
longer edited, so its hash is fixed. v21 starts as v20's rules and names v20's prompt
files; a prompt that changes for v21 is a new file that only v21 names, so a shared file
never changes under v20.

Amended for token limits (2026-11-27, #337): the runner limit changed from US cents to
tokens, a change to the runner shape both documents load through, so v20 was edited once
more with v21 and both hashes moved. Lanes in flight on either id keep their older stamp.

## Alternatives considered

- **An RFC 8785 crate.** Its only gain over the writer above is the float and Unicode
  escaping rules, and the document has no floats. It is one more dependency to vet.
- **Hashing the file bytes.** A comment or a reformat would fork the cohort, which R7
  rules out.

## Consequences

The hash is not interoperable with other RFC 8785 tools. If a document ever needs
floats, or another tool must recompute the hash, this decision is revisited, and the
pinned hash tests show whether the change moved existing hashes.
