# ADR 0004: Pipeline semantic hash without RFC 8785

Status: accepted, 2026-11-03  
Amends: [R7](../research/r7-process-engine.md) "Pin, diff, rollback, side by side" step 4  
Code: `crates/seamlane-core/src/pipeline/hash.rs`, pinned by the v17 hash test

## Context

A lane stamps a pipeline id and a semantic hash at claim, and `lane-stats` groups
lanes by the hash. R7 says to serialize the flattened document with JSON
Canonicalization (RFC 8785) and take SHA-256. The spec for #1 (2026-11-03) agreed to
hand-roll the canonical form on `serde_json` instead of adding an RFC 8785 crate.

## Decision

The hash is SHA-256, lowercase hex, of this JSON:
`{"gates": [...], "limits": {...}, "transitions": [...]}`.

1. Only `[limits]`, the gates and the transitions are hashed. `schema`, `id`,
   `published`, `[cohort]` and comments are not.
2. Defaults are filled in before hashing (`args = {}`, `requires = []`), `routes` are
   sorted, gates are sorted by id, and transitions by id and then by their canonical
   form. `requires` and `of` keep their order, because the first failing gate is
   behaviour.
3. Object keys are sorted by our writer, not by `serde_json`'s map, whose order
   depends on the `preserve_order` Cargo feature that any crate in the build can turn
   on. No whitespace. Strings and integers are written by `serde_json`.
4. Floats cannot reach the hash: the loader accepts only bools, integers and strings
   as values.

## Alternatives considered

- **An RFC 8785 crate.** Its only gain over the writer above is the float and Unicode
  escaping rules, and the document has no floats. It is one more dependency to vet.
- **Hashing the file bytes.** A comment or a reformat would fork the cohort, which R7
  rules out.

## Consequences

The hash is not interoperable with other RFC 8785 tools. If a document ever needs
floats, or another tool must recompute the hash, this decision is revisited, and the
v17 hash test shows whether the change moved existing hashes.
