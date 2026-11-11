# R11: Observability, cost and metrics

Status: draft, 2026-11-02. The metrics list for bet 6 in [VISION.md](../VISION.md), and the event definition of every number `lane-stats` prints. R5 stores the events. R7 stamps the cohort. R4's adapters write usage, and until then `usage report` writes it from transcripts (`origin: transcript`).

## Recommendation

The ledger is the source of every number below. A metric is a query over those events. OpenTelemetry is an optional export of the usage events, in the GenAI conventions as they stood on the unreleased `semantic-conventions-genai` main branch on this date. It is not a second database, and it cannot rebuild a lane median.

Record five families first, in this order. Together they cover the ranked costs in [r2-own-evidence.md](r2-own-evidence.md) and milestone 1's score row. Everything else in the catalog waits.

1. **Peak context and context per merged PR**, from `usage.reported`, with a coverage count beside them.
2. **Flat rounds and re-raised findings**, from `review.finished` and `finding.recorded`.
3. **Head reviewed**, from the sha on `review.finished` and the sha on `pr.merged` (or `pr.opened` while the PR is open).
4. **Work after the handoff**, from `handoff.recorded` against `pr.commit` and against later `review.finished` rows: commits after the handoff, and the share of verified findings after it.
5. **The three v17 gates**, from `gate.recorded`: plan check ran, quality gate ran, verdict YES on the handed-off sha.

Every one of those is grouped by the pipeline hash on `lane.claimed`. That hash is the cohort key, not a sixth metric. A comparison under 25 lanes is flagged and not scored. Token medians are split by agent and by repo before they are compared.

Dollars stay null until a CLI reports a price or a versioned price table exists. Adapters do not estimate cost. Cache reads, cache writes, and reasoning tokens are stored beside the totals and are never added on top of them.

## Sources

| Input | What it is |
|---|---|
| `~/dotfiles/bin/lane-stats` | 719 lines, read 2026-11-02. Every formula in the catalog is this file, not the skill's paraphrase. |
| `~/.claude/skills/lane-stats/SKILL.md` | Which aggregates the report treats as the comparison, and the caveats (powered cohort, investigation lanes, `pr_first_score` definition change). |
| [r2-own-evidence.md](r2-own-evidence.md) | The v16 baselines and the five cost ranks. Run 2026-11-02T13:03:57Z, 154 lanes, all Claude, one repo. |
| [r4-agent-cli-integration.md](r4-agent-cli-integration.md) | Token fields on Claude Code, Codex, Cursor, Gemini CLI, opencode, and Amp. Checked 2026-11-02. |
| [r5-state-and-storage.md](r5-state-and-storage.md) | One SQLite log. Metrics are queries. Heartbeats stay off the log. |
| [r7-process-engine.md](r7-process-engine.md) | Cohort is pipeline id plus semantic hash, stamped at claim. `target_lanes = 25`. |
| [r1-landscape.md](r1-landscape.md) | Session cost dashboards already exist. A cost joined to a process version does not. |
| OTel GenAI metrics | [`gen-ai-metrics.md`](https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-metrics.md) and [`gen-ai-token-metrics.md`](https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-token-metrics.md) on `main`, fetched 2026-11-02. Status of both documents: Development. No tagged release contains the token counters below. |
| OTel token-metric change | [PR 374](https://github.com/open-telemetry/semantic-conventions-genai/pull/374), merged 2026-09-22. Removes `gen_ai.client.token.usage`. |
| Released conventions | `gen_ai.client.token.usage` is still the histogram in semantic-conventions v1.41.0. The Python helper still emitted it as of mid-October 2026. Do not copy v1.41 names into new events. |

## Events the queries read

R5's rebuild set (`lane.claimed`, `lane.status_changed`, `gate.recorded`, `finding.opened`, `finding.updated`, …) is necessary and not sufficient. The rows below are the extra payloads. Names that R4, R5, or R7 already use keep those names. A migration importer (R15) may synthesize them from daisy, a transcript, or a handoff summary. A query never parses prose and never opens a transcript.

| Event | Fields a metric reads |
|---|---|
| `lane.claimed` | `at`, `agent`, `repo`, `pipeline_id`, `pipeline_hash`, `experiment`, `arm`, `lane_kind`, `route`. The hash is R7's semantic hash. `version_from` is `stamp` when a later handoff carried a pipeline id, `claim` when the hash came from the clock of this event. |
| `lane.status_changed` | `status`. |
| `handoff.recorded` | `at`, `next_action`. A handoff whose `next_action` starts with `reviewing` is a progress note (v11). The first **real** handoff is the earliest one that does not. If every handoff is a progress note, the earliest one stands in. |
| `session.started` | `session_id`, `lane_id`, `agent`, `model`, `source` (`parent` or `subagent`). One session has one lane. A stack that shares a worktree gets one session per lane, or the importer leaves `lane_id` null. |
| `session.exited` | `session_id`, `exit_code`. Absent while a detached executor is still running (R4, Amp orb). |
| `usage.reported` | See the next section. One event per provider message id. Retries of the same id do not insert again. |
| `turn.finished` | `session_id`, `message_id`. |
| `tool.finished` | `session_id`, `tool_use_id`, `name`, `poll` (bool), `poll_classifier` (today `poll.v1`). |
| `guard.blocked` | `session_id`, `rule`. `coauthor` is the rule `lane-stats` counts. |
| `review.finished` | `review_id`, `source` (`local` or a PR review), `score` (null if unscored), `head_sha`, `patch_id`, `branch`, `pr`, `started_at`, `finished_at`, `reviewer_version`. |
| `finding.recorded` | `finding_id`, `review_id`, `title`, `path`, `severity`, `lenses`, `verified`, `outcome`. A later judgement updates the same id. The metric reads the latest row. |
| `pr.opened` | `pr`, `at`, `head_sha`, `additions`, `deletions`. |
| `pr.merged` | `pr`, `at`, `head_sha`, `patch_id`. |
| `pr.commit` | `pr`, `message`, `committed_at`, `additions`, `deletions`, `code_lines`, `test_lines`, `files`, `code_classifier`, `noise_classifier`. First time a commit message is seen wins. A rebase that re-dates the same message does not add a second commit. |
| `pr.threads_observed` | `pr`, `observed_at`, `reason` (`at_merge`), unresolved `{author}` list. Written when the merge transition runs. |
| `gate.recorded` | `gate_id`, `outcome` (`ran` or `skipped`), `reason`, `count`, `sha`, `at`. Last event per gate id wins, matching today's "last regex hit in the summaries" rule. |
| `evidence.recorded` | `kind` (`smoke`, `kept_test`, `checklist`), `command`, `status`. Distinct on lowercased command across the lane. |
| `issue.opened` | Mirror import: `key`, `title`, `at`, `body`, `prs_named`. |
| `escape.judged` | `lane_id`, `sha`, `verdict` (`confirmed`, `rejected`, `cost_only`). Written by a person after reading the diff. Absent means the lead is unjudged. |
| `price.published` | `price_id`, model, currency, per-token rates for uncached input, cache read, cache write, output, reasoning. Not required for any number below. |

Heartbeats and lease renewals stay off the log, as in R5. They are not metrics.

Classifiers are versioned because a regex edit would otherwise rewrite history:

| Id | Today's rule, copied from `lane-stats` |
|---|---|
| `poll.v1` | Tool name `Monitor`, or a Bash command matching `\bsleep\s+\d`, `\buntil\b…\bdo\b`, `gh pr checks … --watch`, `gh run watch`, or `\s--wait\b`. |
| `code.v1` | A path is not code when it matches generated files, `.json`, `bun.lock`, `yarn.lock`, `.md` / `.mdx`, `.agents/`, `docs/`, `__tests__/`, `.test.`, `.spec.`, or `.snap`. |
| `escape-noise.v1` | A path is ignored for escapes when it matches generated files, lockfiles, `.md`, `.agents/`, `docs/`, `/seed/`, `.test.tsx?`, `.spec.tsx?`, or `.snap`. JSON that is not a lockfile still counts. |
| `escape-subject.v1` | Subject matches `^(fix\|revert\|Revert)\b`. |
| `measure.v1` | Title matches `^\s*measure\b`, `^\s*(run\|ship)\b.*\bon prod\b`, or `\bprod run\b`. |
| `coauthor.v1` | A tool result contains `BLOCKED by guard-bash: commit carries a Co-`. |

`code.v1` and `escape-noise.v1` disagree on purpose (JSON and `.ts` tests). Keep them as two ids.

## Token and cost across CLIs

`usage.reported` carries the CLI's raw object plus a normalized record. Its `origin` says where it came from. In M1 that is `transcript`: `chaperone usage report` reads the lane's Claude Code and Codex transcripts ([events.md](../events.md), Usage). The R4 adapters write the same kind later. A missing field is null. A field the CLI sent as `0` stays `0`. The current transcript parser treats a missing key inside a usage object as `0`, which turns a partial record into a fake zero. The event does not.

| Field | Meaning |
|---|---|
| `message_id` | Deduping key. Claude writes one JSONL line per content block with the same usage; count the message once, as `lane-stats` does. |
| `source` | `parent` or `subagent`. A parent total is never assumed to include children (R4). |
| `model` | The model that produced this usage, when the CLI names one. |
| `uncached_input`, `cache_read`, `cache_write`, `output`, `reasoning` | Null when that CLI has no such field. |
| `input_includes_cache` | `true`, `false`, or `unknown`. |
| `output_includes_reasoning` | `true`, `false`, or `unknown`. |
| `input_total` | The OTel input number: all input tokens, cache included. Set only when the inclusion flag is known. Anthropic-shaped (`false`): `uncached_input + cache_read + cache_write`, null if any of those three is null. Inclusive (`true`): the CLI's input count already is the total, and `cache_read` / `cache_write` are subsets of it. `uncached_input` leaves the cache out for every CLI. `unknown`: `input_total` is null. |
| `output_total` | Same rule for reasoning. |
| `cost_usd` | The CLI's own price, if it sent one. Else null. |
| `raw` | The CLI object, for a later remap. No secrets (R13). |

Normalized map, from R4's field list:

| CLI | What the stream actually has | Inclusion, until a fixture says otherwise |
|---|---|---|
| Claude Code | `input_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `output_tokens`. `total_cost_usd` when the CLI knows a price. Subagent transcripts are separate files. | `input_includes_cache = false` (`cache_creation` maps to `cache_write`). This is the historical `context_tokens` series: one turn is input + cache read + cache write. `cost_usd` from `total_cost_usd`. Reasoning null. |
| Codex | `turn.completed` usage: input, cached input, output, reasoning. No cache-write field. No cost field. | `cache_read` from cached input. `cache_write` null. `input_includes_cache` stays `unknown` until a recorded `turn.completed` shows whether `input` already contains the cached count. Until then Codex lanes stay out of the token median. |
| Cursor | `result.usage`: `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`, observed on the CLI and the changelog. The output-format schema page still omits `usage`. No cost field. | Same rule: `unknown` inclusion, raw stored, median excluded, until a fixture pins it. A missing `usage` object is null, not zero. |
| Gemini CLI | `stream-json` `result` has per-model token totals. `--output-format json` has `stats`. | One `usage.reported` per model on that result. `input_total` null until the fixture. |
| opencode | Documented aggregate is `opencode stats --json --cost`. Per-message tokens exist on the session API. `run --format json` is not a stable usage contract. | Read the session or stats API. Do not parse the run stream for tokens. Cost from `--cost` may fill `cost_usd` for the session, with `message_id` null and the event marked aggregate so it is not added to per-message sums. |
| Amp | Assistant stream: `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`. A result message may also carry `usage`. | Same Anthropic shape as Claude once a fixture confirms exclusion. A detached orb (`amp -ox` without an attached stream) writes no usage and no `session.exited`. |

Parent and subagent stay two series. Claude's `--max-budget-usd` folds children into the budget flag; the ledger still records the child event when the stream shows one. Adding the series double-counts any child tokens that also appear in the parent, which R2 already saw.

`input_total` matches the OTel attribute `gen_ai.usage.input_tokens` (cache included) and the counter `gen_ai.client.inference.usage.input_tokens`. `cache_read`, `cache_write`, and `reasoning` match the subset counters. OTel says those subsets are already inside the totals. A query that adds `input_total + cache_read` over-counts. `lane-stats` never stored the parts, so today's 4.99B parent figure cannot be split into cache and uncached, and it cannot be priced.

There is no OTel cost metric. The token counters are described as a proxy for cost. Output was 15.2M against 4.99B parent context in R2, so output does not move the token ranking, and it can still move a dollar ranking because output and cache writes are priced differently from cache reads. Dollars are comparable only inside one model, one `price.published` id, and one `cost_usd` source (`cli` or `table`). A vendor price cut is a different price id, not a process win.

R1's conclusion stands: Factory, Devin, Amp, and Cursor already sell a session cost. The harness stores the CLI's usage. It does not build a second cost dashboard.

## Context size

`peak_context` is the largest parent `input_total` on the lane. It is the size of one call, which is what the 300k / 500k / 700k warning is looking at. `context_tokens` is the sum of parent `input_total` across calls. The sum counts the prefix again on every turn. Both numbers stay. The sum is the spend proxy `lane-stats` already reports. The peak is the window.

Subagent calls have their own peak, `subagent_peak_context` (new, see below). They do not raise the parent peak.

A turn with null `input_total` is dropped from the sum and from the peak, and it counts against coverage. Coverage is `usage_turns / turns` for that lane. A lane with no parent usage is null on every token field, which is what `lane-stats` does for a missing transcript, and what it does today for every Codex and Cursor lane (`transcript: n/a`).

Stack sessions are the break with today's script. `lane-stats` finds every transcript whose cwd is the worktree, then divides the sums evenly across the lanes that shared that file list (`transcript_shared_by`). The peak is not divided. That split is a reconstruction. New sessions carry `lane_id`. Imported sessions whose `lane_id` is null stay out of the median and show up as `usage_unassigned`. They are not divided. R2's published medians remain the divided series. A report that places them next to event medians says which series each column is.

The warning hook and a future budget (VISION bet 6, "stop a lane before it burns a day") both read `usage.reported`. The hook is a consumer. A second counter inside the hook will drift from the report. Stopping a lane is a gate in R7, and the fact it reads is `peak_context` from these events. R11 does not define the stop.

OTel's per-operation histograms (`gen_ai.client.inference.operation.input_tokens`) are the distribution of single calls. They are the right export for a p95. They are the wrong place to compute a lane's peak or a version's median. Those are SQL over the events. Histogram buckets cannot be summed across lanes to recover a median.

## OpenTelemetry

Export is opt-in. The default install has no collector, which is bet 7 (one local binary, offline, no daemon). When export is on, the exporter maps events it already stored. It does not open transcripts, and it does not invent tokens the adapter left null.

Map, for a `usage.reported` whose totals are non-null:

| Ledger | OTel instrument (Development, `main` as of 2026-11-02) |
|---|---|
| `input_total` | counter `gen_ai.client.inference.usage.input_tokens` |
| `output_total` | counter `gen_ai.client.inference.usage.output_tokens` |
| `cache_read` | counter `gen_ai.client.inference.usage.cache_read.input_tokens` (subset of input) |
| `cache_write` | counter `gen_ai.client.inference.usage.cache_write.input_tokens` (subset of input) |
| `reasoning` | counter `gen_ai.client.inference.usage.reasoning.output_tokens` (subset of output) |
| one call's `input_total` | histogram `gen_ai.client.inference.operation.input_tokens` |
| one call's `output_total` | histogram `gen_ai.client.inference.operation.output_tokens` |

Attributes worth setting: `gen_ai.provider.name` (`anthropic` or `openai` from the CLI, not from a guess about who hosts the model), `gen_ai.request.model`, `gen_ai.response.model` when both exist, `gen_ai.operation.name = chat` for a normal turn, `gen_ai.token.modality = unknown` unless the CLI split modalities. OTel requires the modality on the counters. `unknown` is the specified value when the CLI did not say. `gen_ai.conversation.id` is `session_id`.

Do not export `gen_ai.client.token.usage`. That histogram is the v1.41 instrument. PR 374 removed it because one histogram keyed by `gen_ai.token.type` does not sum cleanly. The replacement counters have no stability guarantee. Pin the convention revision in the exporter, and treat a rename as an exporter change, not a ledger migration.

Agent and tool instruments describe one in-process invocation:

| Instrument | What it measures | Why it is not a lane metric |
|---|---|---|
| `gen_ai.invoke_agent.duration` | One agent invocation, seconds, until the last chunk or an error. | A lane is many sessions, plus human waits and gates. |
| `gen_ai.invoke_agent.inference_calls` | Model calls that agent itself made. Children count on the child. | Matches `source = parent` versus `subagent`, at invocation scope. |
| `gen_ai.invoke_agent.tool_calls` | Client-side tool calls of that invocation. | Poll turns are a subset, classified by `poll.v1`, not by this histogram. |
| `gen_ai.execute_tool.duration` | One tool call. | Useful later for slow tools. Not a `lane-stats` number. |
| `gen_ai.invoke_workflow.duration` | A whole workflow of agents. | Closest OTel name to a lane, and still the wrong boundary: it has no pipeline hash, no handoff, no merge. |
| `gen_ai.server.*` | A model server the operator runs. | This harness is a client of CLIs. |

`gen_ai.invoke_agent.duration` buckets top out at 409.6 seconds. Claim-to-handoff is hours. Exporting a lane as one agent invocation will saturate the histogram and still miss the cohort. If a collector needs the join, put `harness.pipeline.hash` and `harness.lane.id` on the exported spans as non-`gen_ai` attributes. The comparison query stays in SQLite.

Server-side tools (a provider's built-in search) are outside `gen_ai.invoke_agent.tool_calls` by spec. They are also outside `poll.v1`. Leave them in `raw` until a CLI exposes them as their own tool events.

## What is computed from events

Three shapes:

1. **Fold.** The number is a pure query on the lane's events. Token totals, scores, gates, handoff time. Rebuilding the database rebuilds the number.
2. **Ingest, then fold.** The harness computes something once, stores it on the event, and the query reads the stored value. Code lines, test lines, and patch ids are this shape. The classifier id and the inputs (base sha, head sha, exclude generated) sit on `pr.commit` or `review.finished`, so a disagreement can be recomputed. The query does not call `git`.
3. **Join at report time.** Escape leads and follow-up issues name commits and tickets that are not this lane's events. The lane side is `pr.commit` and `pr.merged`. The other side is a snapshot of `origin/main` and the issue mirror, and the report prints the snapshot ids the way `lane-stats` prints `github_mirror` and `origin_main_at`. `escape.judged` is the only part of that number that enters the ledger.

A refused transition writes nothing (R5). `gate.denied` is the exception R7 already requires: the denial is the event, the phase does not change. Denied gates are visible. They are not a `lane-stats` number yet (`gate_denials` in the new-metric list).

## Fair process-version comparison

`lane-stats` groups a lane by a `pipeline vN` stamp in any handoff summary, and otherwise by the changelog heading that was current at claim time. R7 replaces that with the hash on `lane.claimed`. A later publish does not move the lane. An explicit migrate appends its own event and is a different cohort. `version_from` records whether the id was stamped on the claim or recovered from a handoff during import.

The report procedure, in order:

1. Group by `pipeline_hash`. Label the column with `pipeline_id`.
2. If the cohort has under 25 lanes, set `too_few_lanes` and do not treat its median as a result. Under about 8 lanes, the row is an anecdote. R2 has one powered version (v16, 39 lanes). v8 and v9 have zero because those windows were hours long.
3. A change that spans several small versions is judged on the pooled lanes that share that change, and the report names the hashes in the pool. Pooling is a person saying which hashes share the change. It is not automatic.
4. Split token medians by `agent` and by `repo` before comparing them. R2 is Claude on one repo. A mixed column is a coverage change.
5. Split `flat_rounds`, `claim_to_handoff_h`, and token medians by `lane_kind` (`code`, `measure`, `investigation`, `docs`) and by route. The skill already reads investigation lanes apart.
6. Print the code-line band next to score and token medians. R2's bands: 0–199, 200–599, 600–999, 1,000+. The 600–999 band was the low-score band in that run. A version that ships larger lanes will move tokens with no process change.
7. Token medians include only lanes with non-null parent `input_total`. Print `lanes_with_usage` and the count of turns dropped for null totals. Lower coverage is not a cheaper cohort.
8. A definition break gets a new series id, and the old number is kept under the old id. Known breaks: `pr_first_score` (v1 is after a local pre-review, v2 onward is the first PR review before fixes), transcript reconstruction versus `usage.reported`, even-split stack sessions versus `lane_id`, live thread lookup versus `pr.threads_observed` at merge, and any classifier id change.
9. Credit a version with the metric its changelog named. Other moves are described and not claimed.
10. `open_to_merge_h` is printed. It mostly records when a person merged. It is not evidence for a process change.
11. Escape leads are listed. `confirmed_escapes` is the compared number, and it is null until `escape.judged` exists. R2's criterion 9 is "stay 0 confirmed", with leads explicitly not the metric.
12. An experiment compares `arm` values assigned on `lane.claimed`, same repo, same agent, overlapping code-line bands, and only once each arm has 25 lanes or the report says it cannot. Both arms use the same host predicates (R7), so the diff of the two pipeline documents is the thing a move can be blamed on.
13. A reviewer change (R2 names v2, v4, v9, v16) splits score and finding metrics by `reviewer_version`. The lifetime actionable rate is not a per-version number.
14. Dollar columns also group by `model` and `price_id`.

The comparison statistic is the median, which is what `lane-stats` already prefers (`statistics.median`, rounded to one decimal). Means stay available for scores because the script computes `handoff_score_mean`, and they are the wrong headline: R2's worst lane is 271M context. Token medians should stay integers. Rounding tokens to one decimal is an artifact of a shared helper.

`manual` is not a pipeline cohort. It is PRs by a named author in the 21 days before v1, whose branch key is not a lane. It has a first PR score, an additions count, and an open-to-merge time. It has no tokens, no handoff, and no gates. Import it as `cohort = manual` with those three facts only.

## Catalog

Lane grain is one work item. Version grain is the set of lanes with that `pipeline_hash` (today: that version label). "Median" means the median of the non-null lane values. `with_pr` means the lane has at least one `pr.opened`.

### Per lane

| Field | Event definition |
|---|---|
| `version`, `agent`, `version_from` | `lane.claimed.pipeline_id`, `.agent`, and `.version_from`. During import, a handoff stamp overrides the claim-time changelog version, which is today's `stamp or version_at(claim)` rule. |
| `additions`, `deletions` | Sum of `pr.opened.additions` / `.deletions` over the lane's PRs. A PR belongs to the lane when its branch key equals the lane key and `pr.opened.at` is at or after claim minus one hour. |
| `code_additions` | Sum over PRs of `pr.commit.code_lines` (classifier `code.v1`) for a merged PR whose merge commit is in the main snapshot, else the same count stored from the host's file list while the PR is open. |
| `test_additions` | Sum of `pr.commit.test_lines` when every PR of the lane is merged and present in the snapshot. Otherwise null. An open PR's tests are not counted, because they may still be scratch. |
| `head_reviewed` | True when every PR has a newest local `review.finished` whose `head_sha` equals the PR head, or whose `patch_id` equals the PR's `patch_id`. False when any PR fails that. Null when the lane has no PR. `unreviewed_prs` is the failing PR numbers. `rebased_clean_prs` is the PR numbers saved by the patch id. |
| `open_threads_at_merge` | Count of unresolved entries on `pr.threads_observed` with `reason = at_merge`, summed over merged PRs. Null when any merged PR has no observation. `open_thread_authors` is the distinct authors. Today's script reads GitHub live, so a thread resolved after merge disappears. The event freezes the moment of merge. That is a definition break. |
| `followup_issues` | Issue keys from `issue.opened` whose `at` is after the first `pr.opened` and at or before merge plus `escape_days` (default 14), whose title or body names one of the lane's PR numbers, excluding the lane's own key, plus keys named on a handoff `followups:` line. Minus `planned_followups`. |
| `planned_followups` | The subset of those keys whose title matches `measure.v1`. |
| `pr_first_scores` | For each PR, the `score` of the earliest scored `review.finished` with `source != local`. The version median uses the first element, matching the script. |
| `handoff_score` | `score` of the last scored `review.finished` whose `finished_at` is strictly before the first real handoff. Null if none. `handoff_score_source` is that review's `source`. |
| `local_scores_after_pr` | Scores of local `review.finished` rows whose `started_at` is after the first `pr.opened`, in `started_at` order. |
| `flat_rounds` | Count of adjacent pairs in `local_scores_after_pr` where the later score is less than or equal to the earlier. A drop counts. A lane with one such review has zero. |
| `jobs_before_handoff` | Scored reviews with `finished_at` before the first real handoff. |
| `jobs_after_handoff` | Scored reviews that are not in that set. A scored review with null `finished_at` counts here. |
| `commits_after_handoff` | `pr.commit` rows whose first-seen `committed_at` is after the first real handoff. |
| `codex_comments` | Count of mirror PR comments and reviews on the lane's PRs whose author is `chatgpt-codex-connector`. Freshness is the GitHub mirror snapshot. |
| `needs_you` | The latest `handoff.recorded.next_action` starts with `needs you`, or `lane.status_changed` is `blocked`. |
| `no_pr` | No `pr.opened`. |
| `handoffs` | Count of `handoff.recorded`, including progress notes. |
| `merged_without_handoff` | Has at least one PR, every PR has `pr.merged`, and there is no `handoff.recorded`. |
| `reraised_findings` | Among `finding.recorded` with `verified` true, group by stripped lowercased `title`. For each group, distinct `review_id` minus 1, then sum. A repeated title on a different path still counts. Unverified findings do not. This is the script's key. Do not tighten it in place. |
| `plan_check` | Last `gate.recorded` for `plan_check`: integer `count` when `outcome = ran`, the string `skipped` when skipped, null when absent. |
| `quality_gate` | Same, for `quality_gate`. |
| `verdict_yes` | A `gate.recorded` for `merge_verdict` has decision YES. |
| `verdict_on_head` | `verdict_yes`, and the recorded sha is a prefix of some PR head sha (7 to 40 hex digits). Null when the lane has no PR. |
| `claim_to_pr_h` | Hours from `lane.claimed.at` to the first `pr.opened.at`. |
| `claim_to_handoff_h` | Hours from claim to the first real handoff. |
| `open_to_merge_h` | Hours from the first `pr.opened` to the latest `pr.merged`, and only when every PR is merged. Else null. |
| `merged` | Has a PR, and every PR has `pr.merged`. |
| `smoke_passed`, `smoke_skipped` | `evidence.recorded` with `kind = smoke` and that `status`. Distinct on lowercased command. |
| `checklist_items` | Count of `evidence.recorded` with `kind = checklist`. |
| `kept_test_lines` | Count of `evidence.recorded` with `kind = kept_test`. |
| `possible_escapes` | Report-time join. After `pr.merged`, within `escape_days`, a main-snapshot commit whose subject matches `escape-subject.v1`, whose files meet the lane's `pr.commit.files` after `escape-noise.v1`, which is not one of the lane's own PR numbers, and whose subject key is not some lane's key. Each hit keeps sha, time, subject, and up to five paths. The snapshot id is part of the result. |
| `same_key_followups` | Same window and snapshot. Subject key equals this lane, and the commit is not one of the lane's own PRs. |
| `findings` | Latest `finding.recorded` per id, plus `phase`: `before_handoff` when its review is in `jobs_before_handoff`, else `after_handoff`. |
| `transcript` | Import-only status of the old parser: `ok`, `n/a` (not Claude), `not found`, `skipped`. Event-native lanes use `lanes_with_usage` instead and leave this null. |
| `transcript_sessions` | Count of `session.started` with `source = parent`. |
| `transcript_shared_by` | Import-only. Event-native value is 1, or null when `lane_id` was missing. |
| `context_tokens` | Sum of parent `input_total`. Null when there is no parent usage. |
| `output_tokens` | Sum of parent `output_total`. |
| `turns` | Count of parent `usage.reported` (one per `message_id`). |
| `peak_context` | Max parent `input_total`. |
| `poll_turns` | Count of parent `tool.finished` with `poll` true under `poll.v1`, distinct on `tool_use_id`. |
| `blocked_coauthor` | Count of `guard.blocked` with `rule = coauthor` under `coauthor.v1`. |
| `subagent_context_tokens` | Sum of `input_total` where `source = subagent`. |
| `subagent_output_tokens` | Sum of `output_total` where `source = subagent`. |

### Per version

These are `summarize()` in the script, plus the coverage flag the skill requires.

| Field | Formula |
|---|---|
| `lanes`, `with_pr`, `no_pr` | Counts. `no_pr = lanes - with_pr`. |
| `too_few_lanes` | `lanes < 25`. |
| `lanes_with_transcripts` | Lanes whose `context_tokens` is non-null. The event-native name for the same count is `lanes_with_usage`. Keep the old name on imported rows. |
| `context_tokens_median` | Median `context_tokens`. |
| `context_tokens_per_merged_pr_median` | Median of `context_tokens / pr count` over lanes with usage, `merged`, and at least one PR. |
| `output_tokens_median` | Median `output_tokens`. |
| `subagent_context_tokens_median` | Median `subagent_context_tokens`. |
| `peak_context_median` | Median `peak_context`. |
| `turns_median` | Median `turns`. |
| `poll_turns` | **Sum** across lanes with usage, not a median. Null when none have usage. |
| `handoff_score_median`, `handoff_score_mean` | Median and mean of `handoff_score` on `with_pr` lanes that have one. |
| `handoff_scored` | How many of those scores exist. |
| `handoff_8_plus` | How many of those scores are 8 or more. |
| `ready_under_8` | `with_pr`, latest `next_action` starts with `ready for you`, and (`handoff_score` or 0) is under 8. A missing score counts as under 8. |
| `pr_first_score_median` | Median of the first `pr_first_scores` entry, `with_pr` lanes that have one. |
| `jobs_after_handoff` | Sum of the lane field, `with_pr`. |
| `lanes_with_commits_after_handoff` | `with_pr` lanes whose `commits_after_handoff` is positive. |
| `commits_after_handoff` | Sum, `with_pr`. |
| `flat_rounds` | Sum, `with_pr`. |
| `needs_you` | Sum over all lanes in the cohort, including lanes with no PR. |
| `lanes_with_possible_escapes` | `with_pr` lanes with a non-empty lead list. |
| `lanes_with_same_key_followups` | `with_pr` lanes with a non-empty same-key list. |
| `claim_to_handoff_h_median` | Median `claim_to_handoff_h`, all lanes. |
| `open_to_merge_h_median` | Median `open_to_merge_h`, `with_pr`. |
| `merged` | Count of `with_pr` lanes with `merged` true. |
| `additions_median` | Median `additions`, `with_pr`. |
| `big_lanes_over_1000` | `with_pr` lanes with `additions > 1000`. This is all lines, not code lines. |
| `code_additions_median` | Median `code_additions`, `with_pr`. |
| `test_additions_median` | Median `test_additions` where it is non-null. |
| `big_lanes_over_600_code` | `with_pr` lanes with `code_additions > 600`. |
| `lanes_head_reviewed` | `with_pr` lanes with `head_reviewed` true. |
| `lanes_with_open_threads_at_merge` | `with_pr` lanes with a positive thread count. Null when every lane's thread count is null (no observation yet). |
| `lanes_with_followups` | `with_pr` lanes with a non-empty `followup_issues`. |
| `lanes_with_planned_followups` | `with_pr` lanes with a non-empty `planned_followups`. |
| `lanes_with_smoke` | `with_pr` lanes with `smoke_passed > 0`. |
| `lanes_smoke_skipped_only` | `with_pr` lanes with `smoke_passed = 0` and `smoke_skipped > 0`. |
| `verified_findings` | Count of verified findings on all lanes in the cohort. |
| `merged_without_handoff` | Sum of the lane field, `with_pr`. |
| `reraised_findings` | Sum of the lane field, all lanes. |
| `lanes_plan_checked` | Lanes whose `plan_check` is an integer. A skip does not count. |
| `lanes_quality_gated` | `with_pr` lanes whose `quality_gate` is an integer. |
| `lanes_verdict_on_head` | `with_pr` lanes whose `verdict_on_head` is true. |
| `blocked_coauthor` | Sum of the lane field over lanes with usage. Null when none have usage. |

The text report prints a subset of these columns (scores, work after the handoff, flat rounds, smoke, needs-you, escapes, head, threads, follow-ups, times, code size, the v11 and v17 gates, and the token block). The JSON object is the full set. Both are defined above.

### Manual cohort

| Field | Event definition |
|---|---|
| `prs` | Count of imported non-lane PRs in the 21 days before the first pipeline id, by the configured author. |
| `pr_first_score_median` | Median of the first scored non-local review on those PRs. |
| `pr_first_scored` | How many of those scores exist. |
| `pr_first_8_plus` | How many are 8 or more. |
| `open_to_merge_h_median` | Median open-to-merge hours. |
| `additions_median` | Median additions. |

### Reviewer

Daisy's `reviews stats`, lifetime, not per cohort. R2: actionable rate 74.4% over 1,349 judged findings and 593 reviews, and the reviewer changed at v2, v4, v9, and v16.

| Field | Event definition |
|---|---|
| `actionable_rate` | `actionable / judged` over `finding.recorded` whose outcome is a judgement. Round to three decimals, as the script does. |
| `judged` | Findings that have an outcome. |
| `reviews` | Distinct `review.finished` rows included in that stats pass. |
| `by_lens`, `by_severity` | The same rate grouped by `lenses` and by `severity`. A finding with several lenses counts in each, which is why R2's lens counts sum past the total. |

Until `reviewer_version` is on `review.finished`, this rate stays one lifetime number and is not used to compare cohorts. The per-version rate is a new metric below.

### Run inputs the report prints

These are not quality numbers. A comparison that omits them is not interpretable.

| Field | Meaning |
|---|---|
| `generated_at` | When the query ran. |
| `versions` | Pipeline id and the time it became the default for new claims. |
| `github_mirror` | Freshness of the PR, comment, and thread mirror. |
| `origin_main_at` | Commit time of the main snapshot used for escapes and line counts. The query does not fetch. |
| `repo_path` | Which clone that snapshot came from. |
| `escape_days` | The window, default 14. |

## New metrics

Not in `lane-stats`. Each one closes a gap R2 names, or is required to keep the five above honest.

| Field | Definition | Why it exists |
|---|---|---|
| `usage_coverage` | Parent turns with non-null `input_total`, over parent turns. Also `lanes_with_usage`. | A cohort that stopped reporting usage must not look cheaper. |
| `usage_unassigned` | Usage events with null `lane_id`. | Replaces the even split. |
| `lanes_peak_over_500k` | Lanes whose `peak_context` is over 500,000. The 400,000 cut is the same query with a different threshold. | R2 criterion 8: zero lanes over 500k. v16 still had a lane at 525k. |
| `subagent_peak_context` | Max `input_total` where `source = subagent`. Median per cohort. | Parent peak hides a large child. v17's subagent median was already 7.4M against 1.6M in v16. |
| `verified_findings_after_handoff` | Verified findings whose phase is `after_handoff`, as a count and as a share of `verified_findings`. | R2 criterion 7. v16 was 72 of 179 (40%). The script stores the phase and does not aggregate it. |
| `confirmed_escapes` | `escape.judged` with `verdict = confirmed`. Leads stay `lanes_with_possible_escapes`. | Criterion 9. Leads are not the metric. |
| `plan_check_skipped`, `quality_gate_skipped` | Outcome `skipped`, with `reason`. | `lanes_plan_checked` ignores skips. A correct skip and a missing gate look the same in that column (R2 gap 6). |
| `actionable_rate` per `reviewer_version` | The reviewer table, grouped. | The lifetime rate mixes reviewer revisions. |
| `reraised_findings_same_path` | The re-raise sum keyed by title and path. | Reported beside the title-only number. It does not replace it. |
| `poll_turn_share` | `poll_turns / turns`. | The sum mixes a short cohort with a long one. v16 was 6.5% of turns, 6.7 per lane. |
| `cost_usd` | Sum of `cost_usd` on the lane. Median per merged PR. Null when any included usage lacks a price. | Dollars without a complete price are a partial bill. |
| `cache_read_tokens` | Sum of `cache_read`. | Separates a real prompt shrink from a cache hit. |
| `gate_denials` | Count of `gate.denied` by `gate_id`. | The refusal the engine writes and `lane-stats` cannot see. |
| `diagnose_ran` | A `gate.recorded` or `evidence.recorded` for diagnose, once R7 has a predicate. | R2's 0-of-40 figure is a transcript scan. It is not a `lane-stats` field. |

`subagent_output_tokens` already exists per lane and is not in `summarize()`. A cohort median can wait. Output did not change R2's ranking.

## First five

Implement these queries, and the events they need, before the rest of the catalog. `lane.claimed.pipeline_hash` ships with them. Poll turns wait for milestone 2 (R2: hold the line at 6.7 per lane, do not spend a criterion on it). Escape leads wait until someone writes `escape.judged`.

| # | Metric | v16 baseline (R2) | Pass on the next powered cohort | Events |
|---|---|---|---|---|
| 1 | `peak_context_median`, `context_tokens_per_merged_pr_median`, `lanes_peak_over_500k`, with `lanes_with_usage` | Peak median 240k. 19.1M per merged PR. One lane peaked at 525k. | Median context per merged PR ≤ 23M. Peak median ≤ 300k. Lanes over 500k = 0. Coverage printed. | `usage.reported` |
| 2 | `flat_rounds`, `reraised_findings` | 25 flat rounds on 12 lanes. 37 re-raised on 10 lanes. | ≤ 12 flat rounds and ≤ 18 re-raised, on a cohort about this size. | `review.finished`, `finding.recorded` |
| 3 | `lanes_head_reviewed` | 15 of 28 PR lanes. | Every PR lane. | `review.finished` (`head_sha`, `patch_id`), `pr.merged` |
| 4 | `lanes_with_commits_after_handoff`, `verified_findings_after_handoff` | 10 of 39 lanes, 43 commits. 72 of 179 verified findings (40%) after the handoff. | Lanes with later commits ≤ 10%. Verified findings after the handoff ≤ 20%. | `handoff.recorded`, `pr.commit`, `finding.recorded` |
| 5 | `lanes_plan_checked`, `lanes_quality_gated`, `lanes_verdict_on_head` | 6 of 28, 0 of 28, 0 of 28. Skips were not separable. | 100% of code PR lanes for plan check, 100% of PR lanes for the other two. Measure, investigation, and docs record `skipped` and the reason. | `gate.recorded` |

Row 1 is R2's largest cost (21 lanes over 400k held 38% of parent context) and criterion 8. Rows 2 and 4 are the rounds and the rework that dominate the rest of the token gap. Row 3 is half of v16's PR lanes merging with the newest review off the head. Row 5 is the milestone 1 gate line, and it is cheap once R7 writes `gate.recorded`.

A cohort that hits row 5 and misses row 1 has bought the stamps with context. R2's rule: the gates failed.

## What this leaves open

- The Codex and Cursor inclusion flags, until each adapter has a fixture from a real stream.
- The price table. No `price.published` rows, by decision. `cost_usd` is whatever the CLI attached.
- Whether a budget gate stops at 500k peak or at a dollar cap. The fact is `peak_context`. The threshold is R7's once someone picks it.
- `diagnose`, which has no predicate yet.
- Per-repo numbers. This catalog was checked against the `lane-stats` script and the R2 run on `naiss-ride/mobile-app`. Passage, strain, and shepherd have profiles and no rows in R2.
