# R10: Verification and review layer

> Amended by [ADR 0003](../decisions/0003-chaperone-replaces-daisy.md): daisy does not read the ledger, so it does not claim `check.requested` from it. Chaperone records daisy jobs with `reviews record` ([cli-contract.md](../cli-contract.md)).

Status: draft, 2026-11-02. Producers, evidence, and the reviewer home for the v17 gates in [r7-process-engine.md](r7-process-engine.md). The engine still does not call a model.

## Recommendation

**A check is a step the engine requests and records. A runner fulfills it. The engine refuses the exit transition until the finished event projects the facts that gate names.**

Daisy's review worker stays in the daisy app. PR review and local review are daisy-produced checks. The job row and the findings move onto the harness ledger with the rest of `daisy-core` (R6, R15). Plan check, the quality gate, and the merge verdict stay fresh agent sessions. They are different questions, and they are the checks a lane could skip when only the lane remembered them.

Evidence files are content-addressed, beside the database, written and fsynced before the event commits ([r5-state-and-storage.md](r5-state-and-storage.md)). The event stores hashes and the few fields the projection reads. A head-bound check stamps the sha the runner actually inspected. The model does not supply that sha.

## What the engine runs

[R7](r7-process-engine.md) says the engine loads the pipeline document and refuses a transition whose gates do not hold, and that it does not run the agents that produce evidence. Both stay true. "The engine runs a check" means the check is a row in the document and two events in the log:

1. Entering the phase that owns the check appends `check.requested` (check id, round, head sha, producer kind). The phase does not change again until a matching `check.finished` exists, or the lane is `blocked` with a named blocker.
2. The runner (today the lane's supervisor, later the process owner from R8) sees `check.requested` and starts the producer. It appends `check.finished`. The projection turns that payload into facts. Predicates read facts, never the report text.
3. The caller then names the exit transition. The engine evaluates the gates. On failure it appends `gate.denied` and leaves the phase, as R7 specifies.

The lane session may request a transition and may dispose findings (fixed or answered). It may not write `check.finished` for a check whose producer is `agent` or `daisy`. A finish whose `actor_session` is the lane's session is a refused write, before any gate runs. That is the independence rule. v16 skipped plan check in 6 of 40 lanes, skipped the quality gate entirely, and merged 13 of 28 PRs with the last review not on the head, because those steps lived in prose the same session could forget ([pipeline changelog v17](~/.claude/pipeline-changelog.md), [r2-own-evidence.md](r2-own-evidence.md)).

During migration the lane agent is the runner. It still cannot be the author of the report. R15's adapter may synthesize `check.finished` from a legacy handoff summary. That adapter is the only prose parser, and it is not on the live path.

### The five judgement steps

| Step | Requested when | Producer | Fresh session | Head-bound |
| --- | --- | --- | --- | --- |
| `plan_check` | `planning`, when the lane changes code. Otherwise one skip event, reason `measure`, `investigation`, or `docs`. | Agent, the plan-check brief in `issue` | Yes, `reviewer` | No. Once, before the build. The sha is the tree at plan time. |
| `quality_gate` | `pre_pr`, after the draft PR body is a file. One request. No second transition names it. | Agent, the `quality-gate` skill | Yes, `reviewer`, read-only | No. Once, on the tree that opened the PR. v17 does not re-run it. |
| `local_review` | `start_local_review` and `next_local_round` | Daisy when the profile has `review.daisy`. Otherwise a fresh `reviewer` agent. | Yes | Yes. `review.local_head` is the sha the runner snapshotted. |
| `pr_review` | `pr_open`, only when the profile has `review.daisy` | Daisy worker | The daisy job, not the lane | Yes, for the score. `daisy_review_settled` only cares that it is not still running. |
| `merge_verdict` | `verdict`, after `ask_verdict` | Agent, `merge --verdict-only` | Yes, `general`, never given the lane's reasoning | Yes. The runner writes `verdict.sha`. |

Smoke, kept tests, the diff count, CI, threads, and the PR flags are host collectors. They are steps in the same sense (requested, finished, projected) and they are not judgements. The table of gates below names them.

Codex and Greptile stay advisory. No v17 gate reads them. A thread from a person is `threads.human_open`. A body that starts with `<!-- daisy:finding` is a daisy finding, not a person.

## Where the reviewer lives

The reviewer is two programs today.

- The queue, the head sha, and the finding rows live in `daisy-core`: `jobs.rs` (4,147 lines), `review_outcomes.rs` (2,403), `review_missed.rs` (1,136). R6 already counts these as ledger code to move, not rewrite.
- The worker lives in the daisy app: `src-tauri/src/jobs/review.rs` (7,143 lines). Lenses, a deterministic merge, one refuter, one assess call, then a single GitHub review. Model calls are read-only. The app has to be open. `daisy-cli` exit 5 means the app is closed, and `/ship` then goes on without that review.

| | A missing review can block `ready` | App closed | Lens pipeline stays | Plan check, quality gate, and verdict stay separate | Fits "no model loop in the harness" |
| --- | --- | --- | --- | --- | --- |
| Leave it as a daisy side effect (today) | Only if the lane writes the summary line. | Exit 5, and the lane may continue. | Yes | Yes, and easy to skip | Yes |
| Move the worker into the harness | Yes | Yes | A port of 7,143 lines plus providers | Pressure to fold the three new checks into the lenses | No. [VISION.md](../VISION.md) non-goal, and R6 says the harness is not adopting daisy's model loop. |
| **Worker stays. The check is a harness event.** | Yes. Unfinished projects as `running`. | PR and local review wait. The other three checks still run. | Yes | Yes | Yes |

**The worker stays in daisy. The check record moves.**

`review.daisy` selects the producer of `pr_review` and `local_review`. The harness appends `check.requested`. The app claims that request, runs the job it already runs, and the finish is a harness event (via `daisy-cli` until R15 moves the queue). Local reviews still never post. PR reviews still post one GitHub review.

`daisy_review.state` projects like this, so a closed app cannot pass as "no review needed":

| Profile `review.daisy` | Job | Fact |
| --- | --- | --- |
| Off | None | `absent` |
| On | `check.finished` for `pr_review` on this head | `finished` |
| On | Requested, running, or the app is closed and nothing has finished | `running` |

`daisy_review_settled` allows `absent` and `finished` only ([r7](r7-process-engine.md)). `running` fails `ask_verdict`. "Daisy closed, not reviewed" is a blocker, not a skip that can become `ready`. The lane records the blocker and stops. It does not invent `absent`.

Plan check, the quality gate, and the merge verdict do not join this worker. The plan check attacks a design before code exists (`issue`, at most 5 findings: what breaks, what to drop, for whom). The quality gate is read-only and is not about correctness (comments, tests outside the bar, size, a hand-rolled helper, PR body claims). The verdict is one fresh session on product harm and unshown claims (`verdict-prompt.md`), and a reason that is only test taste is dropped before the event is written. Daisy's lenses answer a fourth question, on the diff, with a score. v17 added the first three because the lens review was the wrong tool and because the lane skipped them. One pipeline that also "does the plan" would make that skip structural again.

The lifetime actionable rate is 74.4% of 1,349 judged findings ([r2](r2-own-evidence.md)). That number belongs to this worker. Moving the prompts into the harness would reset it without a cohort.

Prompt files are not pipeline gates. `check.finished` stores `prompt_sha256`. A prompt edit does not change the semantic hash of the TOML, so it does not fork the v17 cohort. R11 can split metrics on that hash. A process change that must be measured as a version is a gate or an argument in the document, not a sentence in a skill.

## Evidence

R5 stores `evidence_ref` on `gate_results` and keeps blobs out of the row. The format:

```
<ledger-dir>/evidence/sha256/<first two hex>/<64 hex>
```

The runner writes a manifest to a temp file in that directory, fsyncs, renames into the hash path, fsyncs the directory, then commits the event. A crash in between leaves an orphan file. The event payload holds the fields the projection reads and `evidence_ref`, the manifest hash. Sibling files (stdout, a screenshot) are named only by their own hashes inside the manifest. The reader resolves a hash under that root and rejects anything that is not 64 hex digits. No path from an agent is opened.

Caps, so one smoke cannot grow the WAL or the disk without a bound: a text file over 256 KiB is stored as its last 256 KiB with `truncated: true` and `original_bytes`. An image must be `image/png` or `image/webp` and at most 8 MiB, or the write is refused. The manifest itself is small JSON.

`gate_results.evidence_ref` is the manifest hash (R5's one column). Daisy's `reader_gates` can follow it without `payload_json`, which `reader_events` omits on purpose. A rebuild test checks that the payload fields equal the manifest the hash names.

### Head binding

R5 says a pass on an older sha is a different row and cannot satisfy the head. That rule is the v17 verdict bug. It applies to the head-bound steps: `merge_verdict`, `local_review`, `pr_review`, and the host snapshots taken at `ask_verdict` (CI, threads, draft, conflicting). Those snapshots are re-collected at the transition. A stored green CI from an older sha does not project `ci.state`.

`plan_check` and `quality_gate` are lane-scoped on purpose. v17 runs each once. A second quality gate is the absence of an edge, which is how R7 keeps "do it once" visible in a diff. Their `gate_results` row keeps the sha they ran on, and the projection reads outcome and time, not equality with `commit.sha`.

`smoke.recorded` is also lane-scoped in v17: a recorded run or an explicit skip holds the gate. `/ship` already tells the lane to re-run smoke after a code change. That re-run is a producer obligation (a new `smoke` event on the new sha). It is not a v17 refusal. Adding `smoke_on_head` would be a new predicate and a new pipeline id.

### Records

Field lists are the payload. The manifest file is the same JSON plus sibling hashes.

**`plan_check`.** `outcome` `ran` or `skipped`. When skipped, `reason` is `measure`, `investigation`, or `docs`. When ran, `findings` is a count and `changed_plan` is a count, and the manifest lists at most 5 findings: `severity`, `claim`, `who`. This is the `issue` brief, not a daisy finding.

**`quality_gate`.** `outcome` `ran` or `skipped`. Skipped only with `reason` `no_code`. When ran, findings use the skill's five kinds (`comments`, `tests`, `size`, `reuse`, `pr-body`), each `file`, `line`, `change`, `why`, at most 10. `CLEAN` is a finish with `findings: 0`. `fixed` is how many the fix pass disposed before `open_pr`. The gate reads `outcome`, not `fixed`.

**`local_review` and `pr_review`.** `head_sha`, `round` (local only), `score`, `findings[]` with `finding_key`, `severity` (`blocker`, `convention`, `risk`, `nit`, `question`), `path`, `line`, `title`, `body`, `verified`. Daisy's posted body stays on GitHub. The ledger keeps the structured finding. Body text is untrusted input (R5, R13).

**`finding.updated`.** The lane, or the fix agent, writes `status` `fixed` (with `commit`) or `answered` (one line). `previous_round_closed` is true when every finding on the latest local round has one of those. The engine does not decide whether a user would meet it. That sentence is the lane's, and it is the disposition, not a gate of its own.

**`merge_verdict`.** `decision` `yes` or `no`, `sha` written by the runner from the snapshot it passed in, `why`, `proven`, `not_proven`, and for no at most 3 `reasons` each with `kind` (`wrong for a user`, `prod risk`, `unshown claim`, `timing`) and `file` `line`. The runner parses the report block and drops a reason that is only test taste, PR text, or style, as `/merge` already requires. If nothing remains, it records `yes`. The model's prose does not contain the sha the gate compares.

**`smoke`.** One or more cases. A case is `passed`, `failed`, or `skipped`. Passed carries `argv`, `exit`, `showed` (what was observed, not only that it ran), and at least one evidence hash (`stdout` or `screenshot`). The write is refused without that hash. Skipped carries `reason` `nothing_runs` or `attempt_failed` and the error. `smoke.recorded` is true when at least one case exists. An investigation or a no-code lane records `nothing_runs`.

**`kept_tests`.** `items[]` of `file`, `bar`, `guards`, or `{none: true}`. `bar` is one of `bug fix`, `regression`, `money`, `one-way door`, `access`, `scale`, `read budget` (the set `ship-handoff` accepts). `kept_tests.recorded` is true for a list or for `none`. `kept_tests.bar_ok` is true for `none`, or when every item's `bar` is in that set. Whether the test should exist is the quality gate's `tests` kind, not this check.

**`verification`.** The command list from `/ship` step 1: `command`, `status` `passed`, `failed`, or `skipped`, `note`. `verification.unexplained_failures` counts `failed` rows whose note does not say the failure is already on the base (the `ship-handoff` note rule: main, base, pre-existing, flake, not ours).

**`diff.measured`.** `code_lines` from the `/ship` step 2 numstat, tests, docs, and generated paths left out, plus the numstat hash. `code_lines_at_most` and `code_lines_over` read the same number (750 in v17).

**`commit.classified`.** Host, not the lane. Inputs are `review.local_head` and `commit.sha`. A clean rebase (`git range-diff` shows every commit as `=`) sets `review.code_changed_since` false, reason `rebased_clean`. Otherwise paths the step 2 exclusion would drop do not count. A remaining line counts as code unless it is blank or a comment (`//`, `#`, `/*`, `*`, `--`, and block closers). Any code line sets the fact true. A hunk the classifier will not claim counts as code. Evidence is the diff hash. This is the `/ship` "tests, docs, or comments" rule as a function, so the boolean R7 reads is not a sentence the lane picked.

**`ci.snapshot`, `threads.snapshot`, `pr.snapshot`.** Taken at the transition that names them. CI: `gh pr checks`, state `green`, `failing`, `running`, or `none`. Threads: the `/ship` step 6 GraphQL query, unresolved, split into people, daisy findings, and bots. `threads.human_open` is the people count. PR: `isDraft`, and `mergeable` of `CONFLICTING`. Known-flake handling stays in the runner before it records `green` or `failing`. The snapshot stores the citation when a flake was rerun.

**`brief`.** The lane writes `goal`, `merge_case`, `decisions`, `look_at`, `check_by_hand`. The host sets `brief.oversize` when any entry is over 500 characters or `look_at` has more than 3 rows (`ship-handoff` `MAX_TEXT`).

**`decision_list`.** Investigation only. Items are `title` and `done_when`. The field is absent until there is one item, so `present` means a non-empty list.

**`human.merge_yes`.** `{pr, sha, source}` with `source` `pane` or `chat`. The writer is a user-attributed adapter. An agent session cannot write it. `merge_on_yes` is the owner's yes, which `/merge` already requires.

**Stamp.** At claim the engine writes `pipeline.id` and `pipeline.hash` from the document it loaded. No external producer.

## Every v17 gate

Predicate text is the host function in [r7](r7-process-engine.md). The producer column is who may write the event the fact comes from. Combinators have no producer and no file.

| Gate | Producer | Evidence | Check |
| --- | --- | --- | --- |
| `stamp_ok` | None. Combinator. | None | `all` of `stamp_id`, `stamp_hash`. |
| `stamp_id` | Engine, at claim. | Stamp on the lane. | `pipeline.id` equals `v17`. |
| `stamp_hash` | Engine, at claim. | Stamp on the lane. | `pipeline.hash` equals the loaded document hash. |
| `plan_check_recorded` | Fresh `reviewer` agent. Skip reasons are host-checked against `lane.kind`. | `plan_check` | `plan_check_matches_lane`: code implies `ran`. Otherwise `skipped` with `measure`, `investigation`, or `docs`. |
| `quality_gate_recorded` | Fresh `reviewer` agent, `quality-gate` skill. | `quality_gate` | `quality_gate_matches_lane`: code implies `ran`. `skipped` only with no code. |
| `quality_gate_before_pr` | None. Clocks on the two events. | `quality_gate.at`, `pr.opened.at` | `event_before`. Vacuous when the route never opens a PR. |
| `verdict_is_yes` | Fresh `general` agent, `merge --verdict-only`. Sha from the runner. | `merge_verdict` | `verdict.decision` equals `yes`. |
| `verdict_is_no` | Same event. | `merge_verdict` | `verdict.decision` equals `no`. |
| `verdict_on_head` | Runner, not the model. | `merge_verdict.sha`, `commit.sha` | `sha_prefix_eq`, 7 to 40 hex digits, `commit.sha` starts with `verdict.sha`. |
| `smoke_recorded` | Host runner in the worktree. | `smoke` | `smoke.recorded` is true (a case exists, including an explicit skip). |
| `kept_tests_recorded` | None. Combinator. | None | `all` of `kept_present`, `kept_bar`. |
| `kept_present` | Host parser over the lane's list. | `kept_tests` | `kept_tests.recorded` is true (items, or `none`). |
| `kept_bar` | Host, the bar set. | `kept_tests` | `kept_tests.bar_ok` is true. |
| `no_unexplained_failures` | Host, over verification notes. | `verification` | `verification.unexplained_failures` equals 0. |
| `brief_ok` | Lane writes the words. Host measures. | `brief` | `brief.oversize` is false. |
| `rounds_at_most` | Projection of finished `local_review` events. | `local_review` | `review.local_rounds` at most 3. |
| `rounds_below_cap` | Same projection. | `local_review` | `review.local_rounds` at most 2. |
| `rounds_at_cap` | Same projection. | `local_review` | `review.local_rounds` equals 3. |
| `previous_round_closed` | Lane or fix agent writes dispositions. Projection reads them. | `finding.updated` | `review.previous_round_closed` is true. |
| `code_changed_since_review` | Host `commit.classified`. | Diff hash, reason. | `review.code_changed_since` is true. |
| `reviewed_head_ok` | None. Combinator. | None | `any` of `head_eq`, `no_code_since_review`. |
| `head_eq` | Projection of the newest local review. | `local_review.head_sha` | `review.local_head` equals `commit.sha`. |
| `no_code_since_review` | Same classifier as `code_changed_since_review`. | `commit.classified` | `review.code_changed_since` is false. |
| `code_lines_at_most` | Host numstat. | `diff.measured` | `diff.code_lines` at most 750. |
| `code_lines_over` | Same measurement. | `diff.measured` | `diff.code_lines` greater than 750. |
| `ci_not_red` | Host, `gh pr checks`, at the transition. | `ci.snapshot` | `ci.state` is `green` or `none`. |
| `no_human_threads` | Host, the step 6 GraphQL query. | `threads.snapshot` | `threads.human_open` equals 0. |
| `daisy_review_settled` | Daisy worker when the profile asks. Projection as above. | `pr_review` | `daisy_review.state` is `absent` or `finished`. |
| `pr_mergeable` | None. Combinator. | None | `all` of `pr_not_draft`, `pr_not_conflicting`. |
| `pr_not_draft` | Host, `gh pr view`. | `pr.snapshot` | `pr.is_draft` is false. |
| `pr_not_conflicting` | Host, `gh pr view`. | `pr.snapshot` | `pr.conflicting` is false. |
| `human_merge_yes` | User adapter only. | `human.merge_yes` | `human.merge_yes` is true. |
| `one_local_review_at_most` | Projection of `local_review`. | `local_review` | `review.local_rounds` at most 1. |
| `decision_list_present` | Lane, investigation handoff. | `decision_list` | `handoff.decisions` is present, and the writer only stores a non-empty list. |
| `ready_evidence` | None. Combinator. | None | `all` of the members R7 lists on `to_ready`. |

`diagnose`, the fix-agent requirement, and "would a user or prod meet this finding" stay off this list. R7 already says promoting any of them is a new predicate and a new changelog entry. The fix agent is how a lane carries out `finding.updated`. A missing fix agent is visible as a finding that stays open, which already blocks `previous_round_closed`.

## Rounds are transitions

The lane does not keep a review loop in its head.

`start_local_review` requires `rounds_below_cap` and appends `check.requested` for `local_review` round 1 on `commit.sha`. The runner starts daisy, or one fresh reviewer, on that snapshot. `check.finished` projects `review.local_rounds` and `review.local_head`.

`next_local_round` requires `previous_round_closed`, `code_changed_since_review`, and `rounds_below_cap`. Tests, docs, and comments do not pass `code_changed_since_review`, so they do not earn a round. A third finish is the last. `verdict_no_at_cap` sends a no verdict at 3 rounds to `needs_you`.

`enter_final_head` requires the latest round closed. `ask_verdict` requires `reviewed_head_ok` plus the host snapshots. Then `merge_verdict` is requested once. A no with rounds left is a fix, a disposition, and, only if the fix changed code, another local round, then the verdict again. That is the existing `/ship` step 6 rule, with the round cap in the document instead of in the session.

## One gate, end to end

`plan_to_build` requires `plan_check_recorded`. A code lane with no plan-check event:

```json
{
  "pipeline": { "id": "v17", "hash": "<loaded hash>" },
  "lane": { "route": "code_pr", "kind": "code", "changes_code": true }
}
```

`plan_check_matches_lane` fails. The engine appends `gate.denied` for transition `plan_to_build`, gate `plan_check_recorded`, and the phase stays `planning`.

The runner starts a reviewer session that is not the lane. A finish from the lane's own session is refused at the write. A real finish:

```json
{
  "type": "check.finished",
  "check": "plan_check",
  "outcome": "ran",
  "findings": 2,
  "changed_plan": 1,
  "producer": {
    "kind": "agent",
    "role": "reviewer",
    "session": "01J-not-the-lane",
    "prompt_sha256": "<hash of the plan-check brief>"
  },
  "evidence_ref": "<manifest hash>"
}
```

The projection sets `plan_check.outcome` to `ran` and `plan_check.findings` to 2. The same transition then passes this gate. The other required gates on later transitions still have to hold. A measure lane passes the same gate only with `outcome: skipped` and `reason: measure`. `skipped` on a code lane fails it.

The same shape is `quality_gate_before_pr`. `open_pr` already required `quality_gate_recorded`. The later gate compares `quality_gate.at` with `pr.opened.at`. A quality-gate event written after the PR opened fails `ask_verdict` on `code_pr`. The summary regex in `ship-handoff` cannot see that order. The two timestamps can.

## What this leaves open

- The runner that turns `check.requested` into a process is R8. Until then the lane agent is the runner and the write rule is what keeps it from grading itself.
- Subscribing to `check.finished` instead of polling `reviews get` is the events milestone in [VISION.md](../VISION.md). The event is specified here so that subscriber has a type.
- Redaction of command output before it is hashed is R13. The store is local, and reader views do not dump payloads. That is not a secret scanner.
- R15's adapter is the only writer that may turn a legacy `plan check:` line into `check.finished`. Live lanes use the records above. The adapter does not weaken `daisy_review.state` for a profile that asks for daisy.
- `smoke` on the final head, and a prompt hash as a cohort key, are named and not gates.

## Sources

- [RESEARCH.md](../RESEARCH.md) R10, [VISION.md](../VISION.md) bet 1 and milestone 5 (independent checks as steps, evidence stored and linked)
- [r7-process-engine.md](r7-process-engine.md) (the gate ids, the fact bundle, "the engine does not run the agents")
- [r5-state-and-storage.md](r5-state-and-storage.md) (`evidence_ref`, hash outside the row, fsync then commit, reader views omit payloads)
- [r6-language-and-runtime.md](r6-language-and-runtime.md) (`jobs.rs` and the review modules move, the harness does not adopt daisy's model loop)
- [r2-own-evidence.md](r2-own-evidence.md) (v16 skip counts, 74.4% actionable, head not reviewed)
- [r4-agent-cli-integration.md](r4-agent-cli-integration.md) (a fresh session is an adapter obligation, not a daisy feature)
- `~/.claude/skills/issue/SKILL.md` (plan check), `quality-gate/SKILL.md`, `ship/SKILL.md` (v17 steps 1, 5, and 6), `merge/SKILL.md`, `merge/verdict-prompt.md`, `daisy-review/SKILL.md`
- `~/.claude/pipeline-changelog.md` `## v17 2026-11-01T17:30Z`
- `~/.local/bin/ship-handoff` (the prose checks this layer replaces on the live path)
- daisy, read 2026-11-02: `src-tauri/src/jobs/review.rs` (7,143 lines, the worker), `daisy-core/src/jobs.rs` (4,147), `review_outcomes.rs` (2,403), `review_missed.rs` (1,136)

## Decision outputs (for `docs/decisions/`)

1. The daisy review **worker stays in the daisy app**. PR review and local review are harness check steps that the worker fulfills. Plan check, the quality gate, and the merge verdict are fresh agent sessions, recorded the same way.
2. Evidence is a **content-addressed manifest** beside the ledger. The event carries the hash and the projected fields. Head-bound shas come from the runner.
3. **Every v17 gate id** in the table above has a producer, a record, and the R7 check. Combinators have none.
