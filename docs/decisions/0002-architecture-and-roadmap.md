# ADR 0002: Architecture stack and roadmap

Status: accepted, 2026-11-03  
Supersedes: informal [PLAN.md](../PLAN.md)  
Canonical plan: [ROADMAP.md](../ROADMAP.md)

## Context

Research R1–R15 is complete. The owner accepted Seamlane, the proposed technical stack,
milestone 1 metrics, default phase order, and the split: **herdr for interactive work**,
**Seamlane for ledger, gates, and lane coordination**.

## Decision

Adopt the locked decisions, architecture layers, success bar, and phases 0–8 in
[ROADMAP.md](../ROADMAP.md) as the binding build plan until amended by a new ADR.

## Consequences

- Implementation PRs trace to a roadmap phase and research id where relevant.
- herdr remains the default place to start agents through at least milestone 1; Seamlane does not replace it as the primary terminal experience.
- `daisy-cli` router and dual-store migration proceed per R15 before optional runtime takeover (phase 6).
  Amended by [ADR 0003](0003-seamlane-replaces-daisy.md): no router and no dual store; Seamlane replaces daisy.
