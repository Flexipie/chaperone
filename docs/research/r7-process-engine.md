# R7: Process engine

Status: draft, 2026-11-02. Specifies the pipeline format. The executable gate test waits on a language (R6); the fixture at the end is that test.

## Recommendation

**A versioned TOML state machine, plus a closed set of predicates implemented in the harness.** Each lane is stamped with the pipeline id and a semantic hash of the flattened document. The engine loads that document and refuses a transition whose gates do not hold. It does not run the agents that produce evidence.

That is the form that makes v17 data: diffable, pinnable, and runnable next to another version in one process. New combinations of existing gates ship as a new pipeline file. A new kind of check is a host predicate, reviewed and tested with the harness.

Runner-up, if a later gate cannot be written as a comparison or a named host check: Starlark (`starlark` crate, the language Buck2 embeds), with a frozen fact bundle and no I/O. Not Rego, not CUE, and not a state machine compiled into the binary.

## What v17 actually gates

Sources: [VISION.md](../VISION.md) bet 1, `~/.claude/skills/ship/SKILL.md` (pipeline **v17**), `~/.claude/pipeline-changelog.md` `## v17 2026-11-01T17:30Z`, `~/.claude/skills/issue/SKILL.md` (plan check), `~/.claude/skills/merge/SKILL.md` (hard gates), `~/.local/bin/ship-handoff`.

v1 landed 2026-10-09 and v17 landed 2026-11-01: seventeen process versions in about three weeks, each a prose edit plus a changelog heading. `lane-stats` groups lanes by that heading. Since v15 a version is supposed to run about 25 lanes before the next one, so a change can be measured. The freeze rule is a cohort rule. It only works if a lane keeps the definition it started with.

The changelog states the v16 failure this version answers: the gates that held were the ones a script forced, and the ones left to the agent were skipped (plan check in 6 of 40 lanes, `diagnose` never, the fix agent missing in 4 multi-round lanes, 13 of 28 PRs merged with the last review not on the head). v17 makes three of those forced at handoff time:

| Check | What "held" means | Where it is forced today |
| --- | --- | --- |
| Plan check | Every lane that changes code records `plan check: <n> findings...`. Skip only for measure, investigation, or docs, with the reason. | `ship-handoff` regex on `summary`, and only when `next_action` is `ready for you` on a PR that is not an investigation. |
| Quality gate | Every lane with code runs it once, on the draft PR body, before the PR opens. No-code lanes record `quality gate: skipped (no code)`. | Same regex. The "before the PR" order is not checked. |
| Merge verdict | `verdict: YES on <sha>` and that sha is a prefix of `commit` (7 to 40 hex digits). NO with the 3 local-round cap spent is `needs you`, not ready. | Same script. Tested on CORE-4300's v16 handoff: missing lines refused, lines accepted, verdict on another sha refused. |

Those three sit on top of older gates the skills still require and the script only partly checks: smoke and kept-test lines, no unexplained failures, brief text at most 500 characters and at most 3 `look_at` entries, `ready` agreeing with `next_action`, summary pipeline version matching the skill file, CI not failing, no unresolved human thread, daisy review not still running, PR not draft or conflicting, local review on the head unless the later commits are tests, docs, or comments, at most 3 local rounds, no new round until the last round's findings are fixed or answered, and a diff over about 750 code lines (tests, docs, and generated files left out) handing off `needs you` before the PR opens.

Work-item status stays the daisy set: `queued`, `claimed`, `active`, `blocked`, `review`, `done`, `cancelled`. `ready for you` is not a status. It is a `next_action` while status stays `review`. The engine's phases are finer. Status is a projection.

Judgement stays outside the engine (R10 owns the producers): whether a finding would meet a user or prod, whether the merge verdict is YES for product reasons, whether a comment or a test should be deleted. The engine sees that those steps recorded an event, and whether the recorded sha, counts, and phase match the rules.

## Comparison

| | Graph of phases | A version is data a lane can pin | Two versions in one process | A diff names the gate that changed | A violation test is a fixture | One binary, Rust or Go or TypeScript | Cost of a new check |
| --- | --- | --- | --- | --- | --- | --- | --- |
| State machine in code | Yes. An enum and the legal transitions. | No. The version is the binary that contains that module. | Only by compiling every live version in. | A git diff of the implementation. | A unit test per module. | Yes. | A harness release for every process tweak, which is what the seventeen prose versions already are. |
| TOML plus host predicates | Yes. Phases, routes, and transitions are tables. | Yes. Stamp id plus semantic hash. | Load both documents. | Structural diff by gate id and transition id. | Feed facts, expect deny. | Yes. TOML parsers are ordinary libraries in all three languages. | A new combination is a new file. A new kind of check is one host function. |
| CUE | Constraints on one value. The phase graph is still a program. | Yes. | Yes. | Structural, once the graph is encoded as data CUE was not built to walk. | `cue eval` on a fact bundle. | Weak. The maintained evaluator is Go. Rust embedding is either cgo via `cuengine` or `cue-rust` 0.2.0, which is narrower than the Go tool. | A second language, and the host still reduces events to facts. |
| Starlark | Yes, as Python-like functions. | Yes. A `.star` file hashed at claim. | Evaluate two modules. | Textual. A helper can hide which transition changed. | Evaluate the file against fixtures. | Strong in Rust ([`starlark`](https://docs.rs/starlark), used by Buck2) and Go. Weaker in TypeScript. | The pipeline author can add any check without a release, including ones `pipeline diff` cannot explain. |
| Rego | Allow or deny on one JSON input. Ordered phases, round caps, and "return to the phase you blocked from" are outside the language. | Yes. An OPA bundle, or Wasm compiled from it. | Two bundles or two Wasm modules. | Textual rules. | `opa test`. | Wasm embeds, but you still ship a Wasm runtime, and the graph lives in the host. The official library is Go. | Aimed at authorization policies written by many teams, not one workflow. |

### State machine in code

`ship-handoff` is already this, at one transition. It parses `Pipeline version: **v17**` out of the skill file and accepts or rejects a handoff JSON. Everything before that transition is the agent following prose. Putting the whole of v17 in Rust (or Go) match arms would catch the same class of mistake earlier, and the tests would be ordinary tests.

It fails the versioning bet. A process change would be a harness release, in-flight lanes would pick up the new code unless each version is a compiled-in module, and an experiment (two definitions on similar slices) means shipping both modules and a flag. Rollback is a deploy. The semantic diff of "what did v17 require that v16 did not" is a reading of the changelog, which is how it works today and why skipped steps are invisible until `lane-stats` reconstructs them.

### TOML

TOML has no expressions. That is the property we want. The file names phases, routes, transitions, and gates. Arguments are literals (`min_len = 7`, `value = 750`). Boolean structure is three combinators, `all`, `any`, and `not`, applied to named checks. There is no loop, no function call into the ledger, and no way to read a file from the pipeline document.

The host owns the predicate functions and the fact projection. A gate author can require `verdict_on_head` on a new transition without a release. They cannot redefine "sha prefix" inside the pipeline file. That split is what keeps a version diff honest: either a gate id was added, removed, or its arguments changed, or a host function changed in a harness release (rare, reviewed, unit-tested once for every version that names it).

TOML does not force R6. Rust, Go, and TypeScript all parse it. CUE and Rego pull toward Go. Starlark pulls toward Rust or Go.

### CUE

CUE is a constraint language: unification, closed structs, defaults. It is a good fit for "this fact bundle is internally consistent" and a poor fit for "these transitions exist and this lane is on this one". Encoding the graph as CUE data means the engine still walks the graph, and CUE only re-checks the bundle the engine already built. That is two systems for one refusal.

The evaluator people mean by CUE is the Go implementation ([cuelang.org](https://cuelang.org/)). Embedding it in a non-Go harness is cgo (`cuengine`) or a third-party Rust port (`cue-rust` 0.2.0) that does not claim Go parity. Either choice fights the single local binary, and it biases the language decision R6 has not made.

Inheritance, the one CUE feature an experiment would use (`v18` is `v17` with one gate tightened), is a small overlay in the engine: `extends` plus replace-by-id. It does not need a constraint solver.

### Starlark

Starlark is deterministic and hermetic when the host does not inject time, network, or files. The Rust crate is real and is what Buck2 uses. A v17 script could implement `to_ready` as a function over a fact dict and return the failing gate id. Tests are "evaluate this file".

The cost shows up at diff and review time. Seventeen versions written as code will grow helpers (`def ready(lane): return verdict_ok(lane) and gates(lane)`), and a one-line behavior change hides in a callee. `pipeline diff` then becomes a second interpreter. Buck2 accepts that because the build graph is the program. This pipeline is a few dozen gates with stable ids, and `lane-stats` has to attribute a metric move to a specific gate.

Adopt Starlark only when a gate cannot be expressed with the combinators and the named host predicates below. The escape hatch is a new host predicate, not an embedded language "so we are not stuck".

### Rego

Rego decides whether an input is allowed. That matches one gate evaluation (`input` is the fact bundle, `deny` contains `verdict_on_head`). It does not match the rest of the product: phases, routes, a blocked lane resuming where it left off, a round cap, a version cohort, a structural diff. Those remain a state machine written next to the policy. OPA's decision log is a server feature; Wasm evaluation does not emit it ([OPA Wasm](https://openpolicyagent.org/docs/wasm), [decision logs](https://openpolicyagent.org/docs/management-decision-logs)). The ledger already wants a `gate.denied` event, which the engine can append without OPA.

Rego is the right revisit if many untrusted authors ship policies (a plugin marketplace). That is not the milestone-1 shape. One person versions one workflow. A general allow/deny language also makes it easy to write a gate that inspects the wrong fact and still returns true, which is the failure mode v17 is trying to leave.

## Engine contract

The caller names a transition id. The engine does not pick one.

1. Load the document for the lane's stamped id and semantic hash. An unknown hash is a refusal (`pipeline_unknown`), not a fallback to "current".
2. Project the ledger into a fact bundle. Predicates read facts, never events and never prose.
3. The transition applies when its route matches the lane and every required gate returns ok.
4. On failure, append `gate.denied` with the transition id, the first failing gate id, and the fact paths that gate read. The phase does not change.
5. On success, append `phase.entered` and project the work-item status from the table below.

`blocked` and `cancelled` are overlays, not edges from every phase. Entering `blocked` stores `resume_phase` and requires a non-empty blocker. Leaving `blocked` returns to `resume_phase`. `cancelled` is human-only, from any non-terminal phase.

Routes are chosen once, at claim, from facts (`lane.kind`, `lane.changes_code`, `pr.flow`), and stored on the lane. An experiment can change route rules without editing the host.

### Fact bundle

Producers write events. The projection is deterministic. During migration (R15), an adapter may synthesize these events from a legacy handoff summary. Predicates still do not parse prose.

| Fact | Meaning |
| --- | --- |
| `lane.kind` | `code`, `measure`, `investigation`, or `docs`. |
| `lane.changes_code` | The diff has code lines, using the ship step 2 count (tests, docs, generated files left out). |
| `lane.route` | `code_pr`, `no_pr`, `investigation`, or `direct`. |
| `pr.flow` | From the repo profile: `pr`, `direct`, or `none`. |
| `pr.is_draft`, `pr.conflicting`, `pr.opened_at` | GitHub facts. `opened_at` is absent until the open transition. |
| `ci.state` | `green`, `failing`, `running`, or `none`. |
| `threads.human_open` | Unresolved threads from a person. |
| `daisy_review.state` | `absent`, `running`, or `finished`. |
| `plan_check.outcome` | `ran` or `skipped`. `reason` is `measure`, `investigation`, or `docs` when skipped. `findings` is a count when ran. |
| `quality_gate.outcome` | `ran` or `skipped`. `at` is the event time. `findings` when ran. |
| `review.local_rounds` | Count of finished local reviews. |
| `review.local_head` | Sha of the newest local review. |
| `review.previous_round_closed` | Every finding of the latest round is fixed and pushed, or answered. |
| `review.code_changed_since` | A later commit changes code a user or prod would meet. |
| `verdict.decision` | `yes` or `no`. Absent if none was recorded. |
| `verdict.sha` | The sha the verdict names. |
| `commit.sha` | The head the caller wants to hand off or merge. |
| `diff.code_lines` | The ship step 2 count. |
| `smoke.recorded`, `kept_tests.recorded` | Evidence lines exist, including an explicit skip or `kept tests: none`. |
| `kept_tests.bar_ok` | Each `kept test:` line names a bar item (bug fix, money or one-way door, access, scale). |
| `verification.unexplained_failures` | Failed lines whose note does not say the failure is on the base. |
| `brief.oversize` | Any brief entry over 500 characters, or more than 3 `look_at` rows. |
| `human.merge_yes` | The owner said to merge in the lane. |
| `pipeline.id`, `pipeline.hash` | The stamp. Must equal the loaded document. |

### Predicate vocabulary

Host functions. The document only names them and passes literal arguments.

| Function | Holds when |
| --- | --- |
| `present` | The fact path is not absent. |
| `eq` | Fact equals a literal. |
| `lte` / `gt` | Numeric comparison. |
| `sha_prefix_eq` | `left` is 7 to 40 hex digits and `right` starts with `left`. This is the ship-handoff rule. |
| `one_of` | Fact is one of a list of literals. |
| `plan_check_matches_lane` | `changes_code` implies `outcome = ran`. Otherwise `outcome = skipped` and `reason` is `measure`, `investigation`, or `docs`. |
| `quality_gate_matches_lane` | Same shape: ran when there is code, `skipped` only when there is not. |
| `event_before` | Both events exist and `earlier.at` is before `later.at`. Vacuous when the later event is absent (no PR on this route). |
| `reviewed_head_ok` | `review.local_head` equals `commit.sha`, or `review.code_changed_since` is false. |
| `all` / `any` / `not` | Combinators over other gate ids. |

`diagnose` staying unused in v16 is a metric, not a v17 refusal. It is not in this vocabulary. Adding it later is a new predicate plus a transition that names it.

### Pin, diff, rollback, side by side

Published documents are immutable. A fix is a new id (`v17.1`), not an edit of `v17`'s bytes.

The stamp is the id plus the semantic hash of the flattened document:

1. Resolve `extends` (child gates and transitions replace the parent row with the same id; `drop` lists removals).
2. Drop comments and formatting.
3. Sort object keys. Sort gate tables and transition tables by `id`. Preserve `requires` order, because the first failing gate is part of the behavior.
4. Serialize with JSON Canonicalization (RFC 8785) and take SHA-256.

A comment-only edit does not fork the cohort. An argument change does. `lane-stats` groups by the hash, and prints the id as the label. Today's grouping key, the `## vN` heading, becomes that label.

`pipeline diff A B` prints added, removed, and changed gates and transitions by id. Rollback moves the pointer that new claims use (`pipelines/default`). Lanes already stamped stay on their hash. Nothing retargets a running lane unless a person runs an explicit migrate, which appends its own event.

An experiment is two ids loaded at once and an assignment recorded at claim (`experiment`, `arm`). Both arms use the same fact bundle and the same host predicates, so a metric move is the document diff, not a code path that only one arm compiled.

Cohort size is metadata, not a gate:

```toml
[cohort]
target_lanes = 25
```

The engine does not refuse to publish the next version when the count is under 25. The freeze rule is an editorial rule with a stated exception (a broken gate). The number is there so the report can say a version closed early.

## v17 document

Route choice at claim: `investigation` if `lane.kind` is `investigation`; else `no_pr` if `pr.flow` is `none`; else `direct` if `pr.flow` is `direct`; else `code_pr`.

Status projection: `claimed` stays `claimed`. `planning`, `building`, and `pre_pr` are `active`. `pr_open` through `ready` and `needs_you` are `review`. `blocked`, `done`, and `cancelled` keep those names. A `no_pr` lane may go `building` to `done` without entering `review`. Ship allows `active` to `done` for that path.

```toml
schema = 1
id = "v17"
published = "2026-11-01T17:30:00Z"

[cohort]
target_lanes = 25

[limits]
local_rounds = 3
code_lines = 750
brief_chars = 500
look_at = 3
sha_min = 7
sha_max = 40

# --- gates ---------------------------------------------------------------

[[gate]]
id = "stamp_ok"
fn = "all"
args = { of = ["stamp_id", "stamp_hash"] }

[[gate]]
id = "stamp_id"
fn = "eq"
args = { field = "pipeline.id", value = "v17" }

[[gate]]
id = "stamp_hash"
fn = "eq"
args = { field = "pipeline.hash", value_from = "loaded.hash" }

[[gate]]
id = "plan_check_recorded"
since = "v17"
fn = "plan_check_matches_lane"

[[gate]]
id = "quality_gate_recorded"
since = "v17"
fn = "quality_gate_matches_lane"

[[gate]]
id = "quality_gate_before_pr"
since = "v17"
fn = "event_before"
args = { earlier = "quality_gate", later = "pr.opened" }

[[gate]]
id = "verdict_is_yes"
since = "v17"
fn = "eq"
args = { field = "verdict.decision", value = "yes" }

[[gate]]
id = "verdict_is_no"
since = "v17"
fn = "eq"
args = { field = "verdict.decision", value = "no" }

[[gate]]
id = "verdict_on_head"
since = "v17"
fn = "sha_prefix_eq"
args = { left = "verdict.sha", right = "commit.sha", min_len = 7, max_len = 40 }

[[gate]]
id = "smoke_recorded"
fn = "eq"
args = { field = "smoke.recorded", value = true }

[[gate]]
id = "kept_tests_recorded"
fn = "all"
args = { of = ["kept_present", "kept_bar"] }

[[gate]]
id = "kept_present"
fn = "eq"
args = { field = "kept_tests.recorded", value = true }

[[gate]]
id = "kept_bar"
fn = "eq"
args = { field = "kept_tests.bar_ok", value = true }

[[gate]]
id = "no_unexplained_failures"
fn = "eq"
args = { field = "verification.unexplained_failures", value = 0 }

[[gate]]
id = "brief_ok"
fn = "eq"
args = { field = "brief.oversize", value = false }

[[gate]]
id = "rounds_at_most"
fn = "lte"
args = { field = "review.local_rounds", value = 3 }

[[gate]]
id = "rounds_below_cap"
fn = "lte"
args = { field = "review.local_rounds", value = 2 }

[[gate]]
id = "rounds_at_cap"
fn = "eq"
args = { field = "review.local_rounds", value = 3 }

[[gate]]
id = "previous_round_closed"
fn = "eq"
args = { field = "review.previous_round_closed", value = true }

[[gate]]
id = "code_changed_since_review"
fn = "eq"
args = { field = "review.code_changed_since", value = true }

[[gate]]
id = "reviewed_head_ok"
fn = "any"
args = { of = ["head_eq", "no_code_since_review"] }

[[gate]]
id = "head_eq"
fn = "eq"
args = { field = "review.local_head", value_from = "commit.sha" }

[[gate]]
id = "no_code_since_review"
fn = "eq"
args = { field = "review.code_changed_since", value = false }

[[gate]]
id = "code_lines_at_most"
fn = "lte"
args = { field = "diff.code_lines", value = 750 }

[[gate]]
id = "code_lines_over"
fn = "gt"
args = { field = "diff.code_lines", value = 750 }

[[gate]]
id = "ci_not_red"
fn = "one_of"
args = { field = "ci.state", values = ["green", "none"] }

[[gate]]
id = "no_human_threads"
fn = "eq"
args = { field = "threads.human_open", value = 0 }

[[gate]]
id = "daisy_review_settled"
fn = "one_of"
args = { field = "daisy_review.state", values = ["absent", "finished"] }

[[gate]]
id = "pr_mergeable"
fn = "all"
args = { of = ["pr_not_draft", "pr_not_conflicting"] }

[[gate]]
id = "pr_not_draft"
fn = "eq"
args = { field = "pr.is_draft", value = false }

[[gate]]
id = "pr_not_conflicting"
fn = "eq"
args = { field = "pr.conflicting", value = false }

[[gate]]
id = "human_merge_yes"
fn = "eq"
args = { field = "human.merge_yes", value = true }

[[gate]]
id = "one_local_review_at_most"
fn = "lte"
args = { field = "review.local_rounds", value = 1 }

[[gate]]
id = "decision_list_present"
fn = "present"
args = { field = "handoff.decisions" }

# Ready on a PR route. Investigation has no transition that names this.
[[gate]]
id = "ready_evidence"
since = "v17"
fn = "all"
args = { of = [
  "stamp_ok", "plan_check_recorded", "quality_gate_recorded", "quality_gate_before_pr",
  "verdict_is_yes", "verdict_on_head", "reviewed_head_ok",
  "smoke_recorded", "kept_tests_recorded", "no_unexplained_failures", "brief_ok",
  "rounds_at_most", "ci_not_red", "no_human_threads", "daisy_review_settled", "pr_mergeable",
  "code_lines_at_most",
] }

# --- code_pr -------------------------------------------------------------

[[transition]]
id = "claim_to_plan"
routes = ["code_pr", "direct", "no_pr", "investigation"]
from = "claimed"
to = "planning"

[[transition]]
id = "plan_to_build"
routes = ["code_pr", "direct", "no_pr", "investigation"]
from = "planning"
to = "building"
requires = ["plan_check_recorded"]

[[transition]]
id = "build_to_pre_pr"
routes = ["code_pr", "direct", "investigation"]
from = "building"
to = "pre_pr"

[[transition]]
id = "open_pr"
routes = ["code_pr", "investigation"]
from = "pre_pr"
to = "pr_open"
requires = ["quality_gate_recorded", "code_lines_at_most", "smoke_recorded", "kept_tests_recorded"]

[[transition]]
id = "oversize_stop"
routes = ["code_pr", "direct", "investigation"]
from = "pre_pr"
to = "needs_you"
requires = ["code_lines_over"]

[[transition]]
id = "pr_to_addressing"
routes = ["code_pr", "investigation"]
from = "pr_open"
to = "addressing"

[[transition]]
id = "start_local_review"
routes = ["code_pr", "direct"]
from = "addressing"
to = "local_review"
requires = ["rounds_below_cap"]

[[transition]]
id = "next_local_round"
routes = ["code_pr", "direct"]
from = "local_review"
to = "local_review"
requires = ["previous_round_closed", "code_changed_since_review", "rounds_below_cap"]

[[transition]]
id = "enter_final_head"
routes = ["code_pr", "direct"]
from = "local_review"
to = "final_head"
requires = ["previous_round_closed", "rounds_at_most"]

[[transition]]
id = "ask_verdict"
routes = ["code_pr", "direct"]
from = "final_head"
to = "verdict"
requires = ["reviewed_head_ok", "ci_not_red", "no_human_threads", "daisy_review_settled", "pr_mergeable", "quality_gate_before_pr"]

[[transition]]
id = "to_ready"
routes = ["code_pr", "direct"]
from = "verdict"
to = "ready"
requires = ["ready_evidence"]

[[transition]]
id = "verdict_no_at_cap"
routes = ["code_pr", "direct"]
from = "verdict"
to = "needs_you"
requires = ["verdict_is_no", "rounds_at_cap"]

[[transition]]
id = "merge_on_yes"
routes = ["code_pr", "direct"]
from = "ready"
to = "done"
requires = ["human_merge_yes", "verdict_on_head"]

# --- no_pr: measure or docs, nothing that runs ----------------------------

[[transition]]
id = "no_pr_done"
routes = ["no_pr"]
from = "building"
to = "done"
requires = ["plan_check_recorded", "quality_gate_recorded"]

# --- investigation: a doc PR, handoff is needs-you, never ready -----------

[[transition]]
id = "investigation_review"
routes = ["investigation"]
from = "addressing"
to = "local_review"
requires = ["one_local_review_at_most"]

[[transition]]
id = "investigation_needs_you"
routes = ["investigation"]
from = "local_review"
to = "needs_you"
requires = ["decision_list_present", "one_local_review_at_most", "plan_check_recorded"]

# direct skips pr_open. It reuses start_local_review onward.
# addressing is entered from pre_pr.

[[transition]]
id = "direct_to_addressing"
routes = ["direct"]
from = "pre_pr"
to = "addressing"
requires = ["quality_gate_recorded", "code_lines_at_most"]
```

`quality_gate_before_pr` on `ask_verdict` is vacuous for `direct` (no `pr.opened`). On `code_pr` it fails closed if the quality-gate event is missing or timestamped after the PR opened. The summary regex cannot see that order. The phase graph can: `open_pr` already required `quality_gate_recorded`, and this gate checks the clocks.

There is no second quality-gate transition. One pass is the absence of an edge, which is how "do it once" stays visible in a diff.

A v18 experiment that only tightens the size stop is a new file, not an edit:

```toml
schema = 1
id = "v18-size-400"
extends = "v17"

[limits]
code_lines = 400

[[gate]]
id = "code_lines_at_most"
fn = "lte"
args = { field = "diff.code_lines", value = 400 }

[[gate]]
id = "code_lines_over"
fn = "gt"
args = { field = "diff.code_lines", value = 400 }
```

`pipeline diff v17 v18-size-400` reports those two argument changes and nothing else. New claims assigned to the experiment arm stamp the flattened hash. v17 lanes keep v17.

## One gate the engine refuses

Transition `to_ready` on route `code_pr` requires `ready_evidence`, which requires `verdict_on_head`.

Facts, the same case as the CORE-4300 check in the v17 changelog (a YES verdict whose sha is not the handoff commit):

```json
{
  "pipeline": { "id": "v17", "hash": "<loaded hash>" },
  "lane": { "route": "code_pr", "kind": "code", "changes_code": true },
  "verdict": { "decision": "yes", "sha": "fff9999a" },
  "commit": { "sha": "abc1234def567890" }
}
```

`sha_prefix_eq` reads `verdict.sha` and `commit.sha`. `abc1234def567890` does not start with `fff9999a`. The engine appends:

```json
{
  "type": "gate.denied",
  "transition": "to_ready",
  "gate": "verdict_on_head",
  "facts": { "verdict.sha": "fff9999a", "commit.sha": "abc1234def567890" }
}
```

The phase stays `verdict`. A later call with `verdict.sha` of `abc1234` (7 or more hex digits, and a prefix of `commit.sha`) passes this gate. The other members of `ready_evidence` still have to hold. Missing `verdict.decision` fails earlier, at `verdict_is_yes`, which is the "no verdict" half of the same changelog test.

That refusal is the R7 test. It does not need a CLI, a worktree, or a model. After R6, the harness crate loads the TOML above, projects this fixture, requests `to_ready`, and asserts `gate.denied` / `verdict_on_head` and an unchanged phase. The mirror fixture (`abc1234` as the verdict sha, every other `ready_evidence` fact set true) asserts `phase.entered` / `ready`.

## What this leaves open

- The first executable test is specified here and is not in the tree. R6 picks the language the test compiles with. README still bars product code until the architecture notes are decided, so this note does not add a toy interpreter.
- Repo profiles stay the source of `pr.flow` and the smoke command. They are inputs to route choice, not a second pipeline language.
- `reviewed_head_ok` treats "docs, tests, and comments only" as the fact `review.code_changed_since = false`. Classifying a commit is a producer (R10), using the same rule ship states. The gate only reads the boolean.
- `diagnose`, the fix-agent requirement, and "would a user meet this finding" are not v17 refusals. Promoting any of them is a new predicate and a changelog entry, so the cohort stays interpretable.
- R10 maps each gate id to its producer and evidence format. R5 stores the events this projection reads. R15's adapter is the only place a summary regex still exists, and it writes events rather than being called from a predicate.

## Sources

- [RESEARCH.md](../RESEARCH.md) R7, [VISION.md](../VISION.md) bet 1 (process as a versioned definition the harness can refuse)
- `~/.claude/skills/ship/SKILL.md` (v17 steps 1, 5, and 6), `~/.claude/skills/issue/SKILL.md` (plan check skip), `~/.claude/skills/merge/SKILL.md` (hard gates), `~/.claude/skills/quality-gate/SKILL.md`
- `~/.claude/pipeline-changelog.md` `## v17 2026-11-01T17:30Z` and the v1 date `2026-10-09`
- `~/.local/bin/ship-handoff` (`check`, the ready-for-you PR branch)
- Starlark in Rust, embedded by Buck2: [facebook/starlark-rust](https://github.com/facebook/starlark-rust/), [docs.rs/starlark](https://docs.rs/starlark)
- CUE's maintained evaluator is Go; Rust options are FFI ([cuengine](https://docs.rs/cuengine)) or a narrower native port ([tyrchen/cue-rust](https://github.com/tyrchen/cue-rust) 0.2.0)
- Rego compiled to Wasm, and decision logs as a server feature: [OPA Wasm](https://openpolicyagent.org/docs/wasm), [OPA decision logs](https://openpolicyagent.org/docs/management-decision-logs)
- JSON Canonicalization, RFC 8785, for the semantic hash

## Decision outputs (for `docs/decisions/`)

1. Pipeline format: **TOML state machine** with a closed host predicate vocabulary. Not an in-code machine, not CUE, not Starlark, not Rego.
2. A lane stamps **id plus semantic hash**. Published ids are immutable. Rollback moves the default pointer. Experiments load two ids.
3. The first engine test is **`to_ready` denied by `verdict_on_head`** when the verdict sha is not a prefix of the handoff commit.
