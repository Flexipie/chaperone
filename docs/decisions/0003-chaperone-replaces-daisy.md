# ADR 0003: Chaperone replaces daisy

Status: accepted, 2026-11-03  
Amends: [ADR 0002](0002-architecture-and-roadmap.md)  
Supersedes: the router and dual-store plan in [r15-migration.md](../research/r15-migration.md)  
Contracts: [events.md](../events.md), [cli-contract.md](../cli-contract.md)

## Context

[R15](../research/r15-migration.md) planned a milestone 1 where both stores coexist:
a router at today's `daisy-cli` path sends harness-owned ids to `chaperone`, a marker
decides which store takes new creates, ledger ids start at `max(daisy_id) + 1_000_000`,
reads merge both stores, and daisy reads the ledger through views versioned by
`reader_contract`. The owner's goal (2026-11-03, spec for #1) is that Chaperone replaces
daisy's work and spec ledger completely, with no dependency on `daisy-cli` except
daisy's review worker.

## Decision

1. The Chaperone ledger is standalone. Chaperone does not read or write daisy's
   database, and daisy does not read the ledger.
2. `chaperone` keeps `daisy-cli`'s argv, envelope and exit codes
   ([cli-contract.md](../cli-contract.md)), so a skill switches by changing the binary
   path, not by a router.
3. Chaperone's only daisy calls are `daisy-cli reviews get <job>`, from `reviews record`,
   and one read-only exception agreed in #109: chaperone-ui's Backlog tab reads daisy's
   GitHub mirror (`daisy-cli github list --issues --state open --repo <slug>` and
   `github get <key>` for blockers), so it adds no GitHub load. Neither writes to daisy.
4. Superseded from R15: the router, the marker, the id floor, merged reads, and daisy
   reading ledger views. The `reader_contract` version axis is dropped.

## Alternatives considered

- **R15's router.** Keeps every skill on one path and moves ids over one at a time. But
  it puts code in the daisy repo, couples releases of both binaries, and needs an id
  floor and a lookup on every call. It is migration machinery for a store that is going
  away.
- **Dual store with merged reads.** Lets `lane-stats` and `lane-brief` see old and new
  lanes in one query. It also makes every read path a join across two schemas that
  version separately, which is the coupling the ledger exists to remove.

## Consequences

- Plain ids, no floor. `source` and `--repo` are stored as given, there is no
  `linear_outbox`, and `linear_state` is `null`.
- Lanes in flight stay in daisy until they finish. New lanes move only when the skills
  switch, which is a separate follow-up after the first demo.
- The review worker stays in the daisy app. Lanes keep running `daisy-cli reviews local`
  and Chaperone records the outcome as `check.finished`.
- `work board` and `reviews stats` are not in M1. They come before the skill switch.
