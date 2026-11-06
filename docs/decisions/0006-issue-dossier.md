# ADR 0006: An agent dossier per issue key

Status: proposed, 2026-11-07 (agree before code, #69)  
Builds on: the spec and lane kinds in [events.md](../events.md), #42 (files touched per
lane), #53 (indexed key lookups)

## Context

GitHub issues are written for people. An agent starting on an issue needs more: detail
nobody wants in the issue body, links to related issues, ADRs and PRs, the files earlier
lanes touched, and which lanes have worked on the key and what they handed off. Today
that is spread over the issue body, the spec slice (`spec get --key`), `work handoffs`
per lane, worktree diffs, the `lane-brief` script and daisy's `context task`, which
Chaperone never takes over ([cli-contract.md](../cli-contract.md) "Not in M1").

The direction agreed on #69 (2026-11-07): Chaperone is not a tracker
([VISION.md](../VISION.md) non-goals). GitHub stays the source of truth for the issue's
title, done-when and discussion. Chaperone keeps an agent-only record keyed by the
tracker key, grown from the spec slice and its decisions.

"Brief" is already taken three times: the handoff's `brief` (the merge card), R4's lane
brief (the instruction file an agent starts with) and the reserved `brief` check with its
`brief.oversize` fact. This note calls the per-key record the **dossier**, with
`dossier.*` events and a `dossier` CLI noun, so none of the three changes meaning.

## Decision

1. **What it holds.** One read, `dossier get <key>`, returns these sections. Only the
   notes are new; every other section is a fact the ledger already has.

   | Section | From | Written by |
   | --- | --- | --- |
   | `key` | the argument, in `normalize_key`'s form (`owner/repo#N` or `TEAM-123`); a bare `#N` is `invalid_input` | the caller |
   | `slice` | the spec slice whose heading names the key, and that spec's decisions for the slice and the whole spec | `spec create`, `spec update`, `spec decide` |
   | `lanes` | every lane whose `source_id` is the key: `id`, `title`, `status`, `branch`, `pr_ref`, `agent`, `pipeline_id`, `created_at`, `updated_at` | `work create`, `claim`, `update` |
   | `handoffs` | per lane, its handoffs newest first: `at`, `status`, `ready`, `next_action`, `commit`, `pull_request`, the summary's first 300 characters, and the handoff brief's `decisions` | `work handoff` |
   | `files` | per lane, the newest handoff's `changed_files`, each with the lane and the handoff's `commit` | `work handoff` (the source #42 reads) |
   | `notes` | `dossier.noted` events on the key, superseded ones left out | `dossier note` (new) |

   The issue's title, body, done-when and comments are not copied. The dossier carries
   the key, and the agent reads the issue from the tracker. Worktree diffs are not read
   either: a ledger read runs no `git`. #42's overlap adds them on its own path.

2. **One new event kind, `dossier.noted`**, from `dossier note <key> --note <text>
   [--link <kind>:<ref>]... [--supersedes <event id>]`.

   | Field | Rule |
   | --- | --- |
   | `key` | `normalize_key`'s form, else `invalid_input`. Any such key, so a note can come before the first lane (at dispatch) or after the last. |
   | `note` | Prose, at most 600 characters (the finding cap), redacted at write (`note` is already a prose key). |
   | `links` | At most 5. `kind` is `issue`, `pr`, `adr`, `doc` or `commit`; `ref` is an identifier of that type (a tracker key, a PR ref, a repo-relative path, hex of 7 to 40 characters). Anything else is `invalid_input`, never redacted. |
   | `supersedes` | Optional. The id of an earlier `dossier.noted` on the same key, else `invalid_input`. |

   `lane_id` is the lane the actor holds on that key, if there is one, else null;
   `spec_id` is null. The idempotency key is `dossier:` plus the sha256 of the key and
   the canonical payload, so a retry replays. Like `attention.recorded`, it does not touch
   any lane's `updated_at`, so a note never makes a lane's next `--expected-updated-at`
   stale. Notes are never edited or deleted: a correction is a new note that supersedes
   the old one, and the log keeps both.

   Who writes: `/issue` (what it learned that the issue does not say), `/ship` (links to
   the PR and ADRs), `/orchestrate` and the owner. Text from the tracker, PRs or reviews
   is not copied into a note.

3. **No folded table. The dossier is a read.** `dossier get` runs in one read
   transaction: the slice through #53's key index, the lanes through #53's `source_id`
   index, each lane's handoffs on `events_lane`, and the notes on one new partial index,
   `events_dossier ON events (json_extract(payload_json, '$.key')) WHERE kind =
   'dossier.noted'`, the same shape as `events_job`. Every section other than the notes
   is a fold or an event that is already written once; a `dossiers` table would be a
   second write of the same facts on the handoff path, one more thing `rebuild()` must
   reproduce, and one more place to go stale.

4. **Small.** The read is capped: the newest 10 lanes, 10 handoffs per lane, 200 files
   and 20 notes, and each section reports `omitted` when it cuts, so a key with a long
   history still prints a bounded answer. `work handoffs <id>` has the full handoff.
   A write is one event with at most one 600-character note and five links.

5. **Data, never instructions, never a gate.** No fact reads `dossier.*`, so a note
   cannot satisfy or deny a transition ([R13](../research/r13-security-and-trust.md)).
   An agent reads the dossier the way it reads a handoff: as someone else's words.

6. **Errors.** `dossier get` on a key with no slice, lane or note is `not_found`, so a
   mistyped key does not look like an empty history. A malformed key, link or
   `supersedes` is `invalid_input` and writes nothing.

## Alternatives considered

- **A stored brief document per key**, revised like a spec body. Lanes would rewrite it
  at each handoff: a second write of facts the handoff already holds, a CAS fight
  between sibling lanes on one key, and text that drifts from the events it summarises.
- **A `dossiers` table folded from every handoff and spec event.** Cheaper reads, but
  one more fold on every handoff write and in `rebuild()`. With #53's indexes the read
  is a handful of index searches, so the table buys nothing a lane can feel.
- **Notes as GitHub comments.** They are public and human-facing, which is the noise
  #69 is about, and Chaperone would have to call `gh` ([ADR 0003](0003-chaperone-replaces-daisy.md)
  keeps its only outside call to `daisy-cli reviews get`).
- **Spec decisions only.** A decision needs a spec, and a follow-up issue started with
  `spawn <KEY>` has none.

## Consequences

- One code PR follows: the kind in the write and its validation, the index as a
  migration (a one-way door, with the migration tests the bar asks for), `dossier note`
  and `dossier get`, the rows in `events.md` and `cli-contract.md`, and the `--help`
  snapshot. It waits for #53. Estimate about 400 lines of code.
- At the skill switch, `/issue` step 1 reads `dossier get` instead of `lane-brief` and
  daisy's `context task` for ledger facts. #41's lane detail reads the same call.
- `changed_files` is neither capped nor checked as repo-relative paths today
  (`crates/chaperone-core/src/work/handoff.rs`). The dossier caps what it prints; the
  write-side check R13 asks for is a separate fix.
- Until the code PR, `dossier.noted` is reserved in [events.md](../events.md).

## Open items

- The name. `dossier` avoids the three existing briefs; any other unused word works.
