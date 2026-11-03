# Seamlane roadmap

Status: **locked**, 2026-11-03. This is the single source for product decisions, architecture,
phases, and success bars. Research evidence lives in [research/](research/) (R1–R15).
[VISION.md](VISION.md) is the north star; this doc is what we build and in what order.

## Locked decisions (2026-11-03)

| Decision | Choice |
| --- | --- |
| Product name | **Seamlane** (`seamlane` binary, crate, npm) |
| License | **MIT OR Apache-2.0** |
| Open source | **Public from first code commit**; `0.x` until contract/schema settle; **1.0** = milestone 8 quality bar |
| Language | **Rust**, reuse **daisy-core** where it fits |
| Ledger | **SQLite WAL**, `synchronous=FULL`, event + state fold in **one transaction** |
| Agent integration | **MCP server + headless lifecycle** (v0 contract in R4); adapters **Claude Code, Codex, Cursor** first |
| Process engine | **Versioned TOML** pipeline + **host predicates**; v17 expressible; engine **refuses** transitions |
| Verification | Checks as **ledger events**; **daisy reviewer stays in the daisy app** for now |
| Security baseline | **Redact at write**; review/PR/tool text **cannot satisfy gates**; unattended lanes require CLI sandbox |
| Engineering bar | [CONTRIBUTING.md](../CONTRIBUTING.md) + [r14](research/r14-engineering-standards.md) (nextest, property/crash/concurrency/fake-agent, perf budgets) |
| Milestone order | Default sequence below (no reorder requested) |

Prior name research: [r3](research/r3-name-and-license.md), [ADR 0001](decisions/0001-name-license-oss-posture.md).

## Herdr vs Seamlane (how you work)

**herdr** stays where you **start and talk to agents**: panes, keyboard, splits, `spawn`, daily
typing. That does not change through milestone 1 and remains the preferred viewer after the
runtime milestone.

**Seamlane** is the **background control plane**:

- Ledger (work, spec, gates, findings, messages)
- Process engine (pipeline version per lane, refused transitions)
- Lane-to-lane coordination (reservations, seams, typed inbox)
- Metrics and attention (`seamlane attention`, notifications from ledger rules)
- Agent adapters (MCP, lifecycle events) so lanes record facts without you babysitting

```text
You ──► herdr pane ──► agent CLI (Claude / Codex / Cursor)
              │
              └── attach / spawn unchanged in early phases

Seamlane ◄── daisy-cli router, ledger, gates, MCP ◄── agents (events, checks)
              │
              └── later: optional `seamlane attach` if you want; herdr still fine
```

Rules:

- **Do not** replace herdr with a Seamlane chat UI ([VISION](VISION.md) non-goal).
- **Do not** use herdr pane ids as lane identity in the ledger ([R8](research/r8-process-terminal-runtime.md)).
- **Milestone 6** may add `seamlane supervise` as process parent; **default UX** remains
  **command in herdr pane = `seamlane attach <lane>`** (or today’s spawn path until attach exists).
- Seamlane does **not** ship a tmux/zellij replacement; it may integrate as viewer client only.

## What we are not building

Own LLM loop, hosted multi-tenant SaaS, primary chat UI, GitHub/Linear replacement, or
reimplementing daisy’s full personal hub ([VISION](VISION.md)).

## Architecture (accepted stack)

One paragraph per layer; detail in linked research.

1. **CLI surface** — Skills keep calling **`daisy-cli` at today’s absolute path**; router sends
   harness-owned ids to `seamlane` ([R15](research/r15-migration.md)).
2. **Ledger** — Append-only events + projected rows; daisy **read-only** on same file ([R5](research/r5-state-and-storage.md)).
3. **Engine** — Load pinned TOML pipeline; deny transitions; stamp process version on claim ([R7](research/r7-process-engine.md)).
4. **Verification** — Plan check, quality gate, reviews, verdict, smoke → `check.finished` /
   `gate.recorded`; reviewer worker in daisy app ([R10](research/r10-verification-review-layer.md)).
5. **Adapters** — Per CLI: brief file, harness MCP, stream-json lifecycle ([R4](research/r4-agent-cli-integration.md)).
6. **Coordination** — Spec reservations, seam revisions, `seam.ack` / `seam.reject` only ([R9](research/r9-coordination-between-lanes.md)).
7. **Observability** — All `lane-stats` numbers become ledger queries; five metrics first ([R11](research/r11-observability-cost-metrics.md)).
8. **Attention** — `seamlane attention` JSON queues; interrupt rules in [R12](research/r12-interfaces-and-attention.md).

## Success bar (milestone 1)

Measured against **v16** baseline ([R2](research/r2-own-evidence.md)); cohorts under 25 lanes
are flagged, not scored ([R11](research/r11-observability-cost-metrics.md)).

| Metric | v16 baseline (indicative) | M1 target |
| --- | --- | --- |
| Head reviewed (PR lanes) | 15 / 28 | **100%** |
| Plan check / quality gate / verdict on handed-off sha | 6 / 0 / 0 of 28 | **100%** (or explicit skip reason) |
| Context per merged PR (median) | 19.1M | **≤ 23M** |
| Peak context (median) | 240k | **≤ 300k**; no lane **> 500k** |
| Flat rounds (cohort ~39 lanes) | 25 | **≤ 12** |
| Re-raised findings | 37 | **≤ 18** |
| Lanes with commits after handoff | 10 / 39 (~26%) | **≤ 10%** |
| Verified findings after handoff | ~40% | **≤ 20%** |

Gate stamps without context improvement is a failed experiment; both matter.

## Phases

### Phase 0 — Docs and contract freeze (now → first code)

- [x] Research R1–R15
- [x] Lock roadmap (this file)
- [ ] ADRs 0002+ or treat this roadmap as architecture record
- [ ] Publish integration contract + event schema drafts (versioned) before implementation
- [ ] GitHub org `seamlane` when ready; no empty crate/npm squat

**Exit:** First Rust PR has a bar ([R14](research/r14-engineering-standards.md)).

---

### Phase 1 — Milestone 1: Ledger + gates (lanes never stop)

Goal: **enforced v17 gates on new ledger lanes** while **herdr + spawn + skills** stay how you work.

Implementation follows [R15](research/r15-migration.md) phases 0–6:

| Step | Deliverable |
| --- | --- |
| 1.1 | Ledger SQLite file (R5 schema, runtime columns nullable) |
| 1.2 | `daisy-cli` **router** at existing path; marker off → all creates still daisy |
| 1.3 | **Marker on**: new work/spec keys → ledger only; in-flight daisy ids unchanged |
| 1.4 | Engine on ledger lanes: v17 stamped at claim; deny handoff/done without plan check, QG, verdict-on-sha |
| 1.5 | Review jobs → check events; worker still in daisy app |
| 1.6 | Merged **read** path: daisy, `lane-stats`, `lane-brief` see both stores |

**Still in dotfiles for M1:** all pipeline skills, `ship-handoff`, `spawn`, herdr parent,
`lane-stats` transcript scan, `pipeline-profile`, changelog.

**Not in M1:** MCP adapters on the hot path, `seamlane supervise`, coordination (R9), usage events
replacing transcript tokens, history import (R15 phase 7).

**Exit:** M1 success bar table above on ledger-lane cohorts; zero stopped in-flight lanes.

---

### Phase 2 — Event-driven waiting

- Subscriptions: review done, CI green, sibling merged, dependency cleared ([R4](research/r4-agent-cli-integration.md), [R8](research/r8-process-terminal-runtime.md))
- Replace `/orchestrate` **300s poll** and ad hoc `gh`/`daisy` poll loops where events suffice
- Agents and orchestrator wake on ledger notifications

**Exit:** Poll turns down materially on orchestrated runs ([R2](research/r2-own-evidence.md) baseline ~263/lane v16).

---

### Phase 3 — Lane coordination

- Spec reservations before code; seam publish/revise; typed inbox ([R9](research/r9-coordination-between-lanes.md))
- Overlap and stack rules enforced at grant and at ready
- Still no free-text agent chat between lanes

**Exit:** Design cases in R9 covered in tests; fewer “helper reimplemented” / overlap findings.

---

### Phase 4 — Process versions as experiments

- Two pipeline TOML versions on comparable slices ([R7](research/r7-process-engine.md))
- Cohort metrics from R11 compared fairly (pipeline hash on `lane.claimed`)
- Roll forward/back via pinned hash, not prose changelog edits alone

**Exit:** A process change is a number (rounds, escapes, tokens, attention), not a feeling.

---

### Phase 5 — Verification evidence store

- Screenshots, logs, command output → content-addressed evidence ([R10](research/r10-verification-review-layer.md))
- Smoke and kept-test proof linked from gate rows
- Optional: move more review orchestration into harness while worker stays in daisy

**Exit:** Every v17 gate has stored evidence ref where the pipeline requires it.

---

### Phase 6 — Runtime (optional parent swap)

- `seamlane supervise`: worktree, port block, PTY, wait, `lane resume` ([R8](research/r8-process-terminal-runtime.md))
- **herdr pane runs `seamlane attach <lane>`** (or spawn updated to invoke attach); you still **start from herdr**
- Retire `spawn` as parent when supervise is stable; ledger identity unchanged

**Exit:** Agent survives viewer detach; stall/death is ledger events, not screen scrape.

---

### Phase 7 — Interfaces

- **`seamlane attention`** shipped in M1 or early Phase 2; this phase adds **TUI** and/or **local web**
- daisy may become a **client** of ledger read APIs ([VISION](VISION.md))
- Notification subscriber replaces agents calling `herdr notification show` ([R12](research/r12-interfaces-and-attention.md))

**Exit:** One glance answers needs-you / ready / stuck / expensive without opening every pane.

---

### Phase 8 — Open source 1.0

Quality bar from [VISION](VISION.md): ADRs for major choices, property/crash/concurrency tests,
fake-agent whole-lane tests, perf budgets in CI, versioned public contract, security rules ([R13](research/r13-security-and-trust.md)), dogfood own development in Seamlane lanes.

**Exit:** Tag `1.0.0`; stable integration contract and event schema.

---

## Migration and dual-store (reference)

Until every active lane is ledger-native or done:

- One id, one store for life ([R15](research/r15-migration.md))
- Router lookup: daisy id → daisy code; ledger id → `seamlane`
- New ids start at `max(daisy_id) + 1_000_000`
- Phase 7 **history import** from finished daisy rows only (prose parsers), after M1 ship

## Research index

| ID | Topic | Output |
| --- | --- | --- |
| R1 | Landscape | [r1-landscape.md](research/r1-landscape.md) |
| R2 | Own evidence | [r2-own-evidence.md](research/r2-own-evidence.md) |
| R3 | Name / license | [r3-name-and-license.md](research/r3-name-and-license.md) |
| R4 | Agent CLI contract | [r4-agent-cli-integration.md](research/r4-agent-cli-integration.md) |
| R5 | State / storage | [r5-state-and-storage.md](research/r5-state-and-storage.md) |
| R6 | Language | [r6-language-and-runtime.md](research/r6-language-and-runtime.md) |
| R7 | Process engine | [r7-process-engine.md](research/r7-process-engine.md) |
| R8 | Terminal runtime | [r8-process-terminal-runtime.md](research/r8-process-terminal-runtime.md) |
| R9 | Coordination | [r9-coordination-between-lanes.md](research/r9-coordination-between-lanes.md) |
| R10 | Verification | [r10-verification-review-layer.md](research/r10-verification-review-layer.md) |
| R11 | Metrics | [r11-observability-cost-metrics.md](research/r11-observability-cost-metrics.md) |
| R12 | Attention UI | [r12-interfaces-and-attention.md](research/r12-interfaces-and-attention.md) |
| R13 | Security | [r13-security-and-trust.md](research/r13-security-and-trust.md) |
| R14 | Engineering | [r14-engineering-standards.md](research/r14-engineering-standards.md) |
| R15 | Migration | [r15-migration.md](research/r15-migration.md) |

## Next actions (implementation)

1. Draft **versioned** event schema + integration contract markdown in `docs/`.
2. Scaffold **Rust workspace** per R14 (`seamlane`, `seamlane-core`, …).
3. Implement R15 **phase 1** (ledger file + write function + tests).
4. Implement **router** + **marker** + **engine** through M1 exit criteria.

[PLAN.md](PLAN.md) is superseded by this file.
