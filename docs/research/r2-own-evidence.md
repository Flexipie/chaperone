# R2: Our own evidence

Status: 2026-11-02. Baseline for milestone 1 in [VISION.md](../VISION.md) (ledger and enforced gates).

## Sources

| Input | What it is |
|---|---|
| `~/.local/bin/lane-stats --json` | Run 2026-11-02T13:03:57Z. 154 lanes, pipeline v1–v17. JSON kept only for this pass (`/tmp/lane-stats-r2.json`). The script is 719 lines at `~/dotfiles/bin/lane-stats` and defaults to `naiss-ride/mobile-app`. |
| GitHub mirror | Fresh, synced 2026-11-02T13:02:36Z (13s old at generation). |
| `origin/main` | Commit time 2026-11-02T13:12:42+01. Escapes are computed from that local ref. `lane-stats` does not fetch. |
| dotfiles | `d40f4dc` (`pipeline v17: plan check on every code lane...`, 2026-11-02). |
| Changelog | `~/.claude/pipeline-changelog.md` (same text as `~/dotfiles/claude/pipeline-changelog.md`), v1 2026-10-09 through v17 2026-11-01T17:30Z. |
| Prior reads | `~/dotfiles/claude/pipeline-reports/2026-11-02-v16.md` (run 2026-11-01T16:33Z, v16 at 40 lanes) and `2026-10-29-v16.md`. Cited where this run does not contain the fact (diagnose usage, escape adjudication). |
| Enforcement | `~/dotfiles/bin/ship-handoff` (161 lines) refuses a `ready for you` PR handoff that lacks `plan check:`, `quality gate:`, and `verdict: YES on <sha>` equal to `commit`. `~/dotfiles/bin/pipeline-profile` only selects the repo profile. It records no quality numbers. |

Versions come from `## vN` headings. A lane counts in the version active when it was claimed, or the version stamped on its handoff. v8 and v9 have 0 lanes: those windows were hours long and the lanes fell into v7 or v10. Every version except v16 has `too_few_lanes` (under 25). Under about 8 lanes, treat the row as an anecdote.

All 154 lanes are `agent: claude`. Transcripts: 152 `ok`, 2 missing (`CORE-4014`, `CORE-4054`). 26 lanes share a worktree transcript (20 split across 2, 6 across 3); token sums are already divided evenly. Parent-session `context_tokens` are input + cache read + cache write, summed over turns. Subagent context is a separate series. Output tokens are 15.2M against 4.99B parent context, so they do not change the ranking. There is no price in this evidence, so cost is in tokens, rounds, and handoffs.

`handoff_score` is the last review that finished before the first handoff. A later local score can be higher. `CORE-4394` is handoff score 3 with later local scores 4, 4, 3, 6, 4, 6.

## Baseline

Parent-session context, all versions: **4.99B**. Subagent context on top: **0.46B**. Poll turns: **1,969** of **22,847** turns (8.6%). Flat rounds: **116**. Re-raised findings: **119**. Verified findings: **836** across 108 lanes. PR lanes: **135**, of which **67** have the newest local review on the merged head. Merged with no handoff: **8**, all before v12. `needs you`: **26** lanes. Possible-escape leads: **192** entries on 65 lanes. Follow-up issues: **75** entries. Planned follow-ups: **34**. Same-key follow-ups: **0**.

Daisy reviewer, lifetime, not per version: actionable rate **74.4%** (1,349 judged findings, 593 reviews). By lens: correctness 81.2% (399), security 75.7% (181), tests 75.2% (508), intent 73.7% (460), improvement 64.9% (171). The reviewer changed between versions, so this rate is a mix.

| | manual | v7 | v12 | v14 | v15 | **v16** | v17 |
|---|---|---|---|---|---|---|---|
| lanes (with a PR) | 61 PRs | 16 (16) | 13 (13) | 13 (12) | 8 (6) | **39 (28)** | 6 (6) |
| enough lanes (≥25) | | no | no | no | no | **yes** | no |
| handoff median / at 8+ | first PR 6 | 7 / 5 of 12 | 9 / 11 of 12 | 7.5 / 4 of 8 | 9 / 5 of 6 | **9 / 22 of 28** | 9 / 4 of 6 |
| ready under 8 | | 1 | 1 | 5 | 1 | **5** | 1 |
| lanes with commits after handoff | | 6 (23 commits) | 3 (7) | 8 (28) | 4 (12) | **10 (43 commits, 27 jobs)** | 2 (13 commits, 6 jobs) |
| flat rounds | | 21 | 3 | 11 | 3 | **25** | 5 |
| re-raised findings | | 15 | 0 | 27 | 0 | **37** | 7 |
| needs you | | 7 | 2 | 3 | 0 | **1** | 0 |
| merged with no handoff | | 4 | 0 | 0 | 0 | **0** | 0 |
| head reviewed | | 11 of 16 | 5 of 13 | 4 of 12 | 4 of 6 | **15 of 28** | 3 of 6 |
| plan check / quality gate / verdict on head | | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 2 / 0 / 0 | **6 / 0 / 0** | 5 / 5 / 5 |
| median code / test lines | | 65 / 177 | 139 / 80 | 207 / 135 | 61 / 51 | **120 / 65** | 173 / 113 |
| lanes over 600 code lines | | 0 | 1 | 1 | 1 | **2** | 0 |
| claim to handoff, median | | 0.9h | 2.1h | 1.9h | 1.0h | **0.9h** | 1.4h |
| open to merge, median | **13.3h** | 10.5h | 5.1h | 11.2h | 1.9h | **3.9h** | 2.1h |
| context per merged PR, median | | 25.3M | 23.7M | 35.3M | 15.8M | **19.1M** | 32.5M |
| parent context, median | | 25.3M | 25.4M | 34.6M | 13.1M | **14.1M** | 35.6M |
| subagent context, median | | 0 | 1.1M | 1.5M | 2.7M | **1.6M** | 7.4M |
| peak context, median | | 321k | 285k | 282k | 194k | **240k** | 330k |
| poll turns (per lane) | | 316 (20) | 195 (15) | 226 (17) | 56 (7) | **263 (6.7)** | 63 (10.5) |
| verified findings | | 86 | 40 | 144 | 10 | **179** | 27 |
| lanes with escape leads | | 13 | 4 | 4 | 3 | **12** | 0 |

v16 parent context sums to 841M (21.6M per lane). v17 sums to 265M parent plus 77M subagent. The 2026-11-01 report counted 40 v16 lanes; this run counts 39, with 6 lanes now stamped v17.

Size against score, every lane that has both a code-line count and a handoff score:

| Code lines | Lanes | Handoff score, median | Parent context, median |
|---|---|---|---|
| 0–199 | 64 | 9.0 | 16.8M |
| 200–599 | 44 | 8.0 | 37.2M |
| 600–999 | 10 | 6.5 | 43.1M |
| 1,000+ | 4 scored (1 unscored) | 8.0 | 53.0M |

The dispatch skill still says lanes over 1,000 lines ended at median 5.5 against 8.5 for smaller ones. This run's scored 1,000+ lanes are median 8. The band that scores low is 600–999.

Already moved, and the next version has to hold these:

- Handoff median 9, with 22 of 28 v16 lanes at 8+.
- Tokens per merged PR 19.1M in v16, down from 35.3M in v14. v15's cost drop held once v16 reached 39 lanes.
- Test lines median 65 in v16, down from 223 in v10 (code median then was 75).
- `needs you` is 1 lane in v16, down from 7 in v7.
- Merged with no handoff is 0 since v12 (the 8 earlier cases are v1 ×1, v7 ×4, v10 ×3).
- Co-author trailer attempts: 14 blocked in v16, 0 in v17 after `attribution` was cleared. That is one wasted turn each, and it moved because a setting changed, which is the same shape as a gate.

## Ranked by cost

Association is not a cause. Lanes with flat rounds also tend to be larger. The token gaps below are real splits. The v16 counts are the baseline a gate can be scored against.

### 1. Sessions that run past any useful context size

21 lanes peaked over 400k tokens and hold **1.88B of 4.99B** parent context (38%). Their median context is **83.3M**, against **18.0M** for the other 131 lanes with a transcript.

The worst lane is `CORE-4013` (v7): **271M** context, **954** turns, peak **628k**. Next are `CORE-4053` (v7, 208M, peak 589k, 614 turns) and `CORE-4370` (v16, 118M, peak 525k, 389 turns).

v16, the only powered version: peak median **240k**, and **15 of 39** lanes still peaked over 300k. Two peaked over 400k: `CORE-4264` (442k, 68M, 242 turns) and `CORE-4370` (525k, 118M). `CORE-4370` also shipped **2,188** code lines, handoff score **6**, **13** re-raised findings, **6** flat rounds, **6** commits after the handoff, and it is the only v16 `needs you`.

The context hook warns at 300k, 500k, and 700k. It does not stop the lane. Nothing in the v16 numbers is a budget.

### 2. Review rounds that do not raise the score

54 lanes have at least one flat round (a local round that left the score where it was). Median parent context **41.9M**, against **16.0M** with none. 35 lanes have a re-raised finding. Median **49.6M**, against **16.5M** with none.

All-time: **116** flat rounds, **119** re-raised findings, **836** verified findings. The largest verified lens is intent (266), then tests (171), then correctness (98).

v16: **25** flat rounds on **12** lanes, **37** re-raised findings on **10** lanes, **179** verified findings. **72 of 179** (40%) are after the handoff. Of the verified risk and blocker findings, **34 of 107** are after the handoff. Severity mix of the 179: risk 88, nit 30, convention 23, blocker 19, improvement 17, question 2. Intent is on 97 of them, correctness 58, tests 41.

Title clusters in v16 verified findings (a finding can match more than one):

| Cluster | Findings | Lanes |
|---|---|---|
| PR description or claims | 28 | 12 |
| Admission, access, or auth | 14 | 4 |
| An existing helper was reimplemented | 11 | 6 |
| Test missing or too weak to fail | 10 | 5 |
| Comment or stale name | 8 | 5 |
| Budget, sizing, or worst case | 6 | 3 |

`CORE-4370`, `CORE-4246` (862 code lines, score 6, no plan check, review not on the head, 46M), `CORE-4235` (599 code lines, score 7, 6 re-raises, 4 flat rounds, 8 commits after handoff, 43M), and `CORE-4165` (243 code lines, score 8, 5 re-raises, no plan check, review not on the head, 51M) are the lanes where those clusters pile up. The 2026-11-02 report's transcript scan said the same four skipped the plan check. This run agrees they have `plan_check` null.

About one judged finding in four is not actionable (74.4% of 1,349). That rate is lifetime and mixed across reviewer revisions.

### 3. Merged with the newest review not on the head

**68 of 135** PR lanes (50%) are not `head_reviewed`. Median parent context on those lanes is **35.8M**, against **20.8M** on the 67 that are. Same size confound as problem 2.

v16: **15 of 28** (54%). Of the 22 that handed off `ready for you`, 11 were not on the head, and 9 of those 11 also had no plan check.

v17 is already 3 of 6 off the head, and a verdict does not close the gap:

| Lane | Handoff | Plan / gate / verdict on head | Newest review on head |
|---|---|---|---|
| CORE-4383 | `done:` | none | no (#3126 unreviewed) |
| CORE-4387 | `done:` | 5 / 5 / yes | no (#3130 unreviewed, #3131 clean rebase) |
| CORE-4394 | `ready for you` | 5 / 4 / yes | yes |
| CORE-4218 | `done:` | 4 / 6 / yes | yes |
| CORE-4376 | `done:` | 5 / 6 / yes | yes |
| CORE-4398 | `done:` | 5 / 6 / yes | no (#3142 unreviewed) |

`ship-handoff` checks that the verdict sha equals `commit`. It does not check that the newest daisy review is on that commit. `CORE-4387` and `CORE-4398` pass the verdict check and still fail `head_reviewed`.

### 4. A handoff that was not the end of the work

v16: **10 of 39** lanes took more commits after the handoff (**43** commits, **27** jobs). **72 of 179** verified findings came in after that handoff. `ready_under_8` is 5. `needs you` is 1.

All-time: **56** lanes, **206** commits, **225** jobs after the handoff.

Calendar time is the part that already shrank. Manual open-to-merge median is **13.3h** (61 PRs, no lane). v16 open-to-merge median is **3.9h** (sum 141h) and claim-to-handoff median is **0.9h** (sum 39h). The merge clock mostly records when a person merged. The cost that is still large is the rework after the lane said it was done.

### 5. The checks that are still instructions

v16 PR lanes: plan check on **6 of 28**, quality gate on **0 of 28**, verdict on the head on **0 of 28**. The 11 lanes with no PR are measure or investigation titles (`measure: ...`, plus two incident notes) and have no plan check, which matches the rule that those lanes skip it.

The 2026-11-02 transcript scan, not this JSON, found `diagnose` on **0 of 40** v16 lanes. `lane-stats` has no diagnose field, so that number was not refreshed here.

The step a script enforces is the one that holds. `merged_without_handoff` has been **0** since v12. From v13, `ship-handoff` also locks the handoff shape (smoke line, kept-test line, `ready` agreeing with `next_action`). v17 added three more lines, and only on `ready for you`.

v17 ended **5 of 6** PR lanes as `done:` (merged in the pane) and **1 of 6** as `ready for you`. The lane that missed every new line is `CORE-4383`: `done:`, no plan check, no quality gate, no verdict, PR #3126 unreviewed, 170 code lines, local score 9, 29.6M context. The changelog says a "merge it" in the pane merges with no new verdict. The data says that path also skips the plan check and the quality gate whenever the recorded handoff is `done:`.

Early cost of the stamps, on 6 lanes only: median context per merged PR **32.5M** against v16's **19.1M**, subagent median **7.4M** against **1.6M**, peak median **330k** against **240k**, turns median **159** against **87**, poll turns **10.5** per lane against **6.7**. `CORE-4394` is the outlier: **105.5M** parent context, **44.6M** of it subagent, **444** turns, handoff score **3**, **12** commits after the handoff, **4** re-raises, and it did record a plan check, a quality gate, and a verdict on the head. Stamps can land on a lane that is still expensive and still being rewritten. Six lanes cannot say the regression is real. The next powered version has to show it is not.

### Next, outside the top five

Polling is **1,969** turns, **8.6%** of all turns. v16 is **263** turns, **6.7 per lane**, **6.5%** of its 4,071 turns, down from v7's 316 (20 per lane). The 2026-11-02 scan said the v16 waits were mostly sleeps of 1–8s or the `reviews local --wait` that `/ship` asks for. Waiting is milestone 2 in the vision. The do-not-regress line is 6.7 poll turns per lane.

Confirmed escaped bugs are not in this ranking. The lead count is in the gaps below.

## Milestone 1 success criteria

Milestone 1 is the ledger and the gates, with today's skill CLI left working. Score it on the first version after that ships which reaches **25 lanes**, against v16 (39 lanes, 28 with a PR). v17's 6 lanes are the dry run, not the score.

| # | Criterion | v16 baseline | Pass |
|---|---|---|---|
| 1 | Plan check recorded on every PR lane that changes code | 6 of 28 PR lanes. 0 of 11 no-PR lanes, which are measure or investigation | 100% of code PR lanes. Measure, investigation, and docs-only stay skipped, and the handoff says `plan check: skipped (<which>)` |
| 2 | Quality gate recorded on every PR lane | 0 of 28 | 100% of PR lanes |
| 3 | `verdict: YES` on the handed-off sha, including a lane that finishes `done:` after merging in the pane | 0 of 28. v17 is 5 of 6, and the miss is a `done:` handoff (`CORE-4383`) | 100% of PR lanes, both `ready for you` and `done:` |
| 4 | Newest local review on every merged PR's head (`head_reviewed`) | 15 of 28 (54%). v17 is 3 of 6, with 2 verdict-on-head lanes still unreviewed (`CORE-4387`, `CORE-4398`) | 100% of PR lanes. A matching verdict sha is a separate check |
| 5 | No PR lane merges without a handoff | 0 of 28, and 0 since v12 | Stay 0 |
| 6 | Re-raised findings, and flat rounds | 37 re-raised (10 lanes), 25 flat rounds (12 lanes) | ≤18 re-raised and ≤12 flat rounds, on a version of about this size |
| 7 | Work still happening after the handoff | 10 of 39 lanes, 43 commits, 27 jobs. 72 of 179 verified findings (40%) after the handoff | Lanes with commits after the handoff ≤10% (≤4 in a 39-lane version). Verified findings after the handoff ≤20% |
| 8 | Token guard, so the new agents do not buy the stamps with context | 19.1M context per merged PR, median. Peak median 240k. Subagent median 1.6M. One lane peaked at 525k (`CORE-4370`). v17 early median is 32.5M per merged PR and 7.4M subagent | Median context per merged PR ≤23M. Peak median ≤300k. Lanes with peak over 500k = 0. If stamps are at 100% and this row fails, the gates failed |
| 9 | Confirmed escapes | 0 confirmed in the 2026-11-02 read of 34 v16 leads | Stay 0. Leads are not the metric |
| 10 | `needs you` | 1 of 39 (`CORE-4370`, an admission decision on a 2,188-line lane) | ≤1 per 25 lanes, and none of them caused by a missing gate |

Hold, and do not spend a criterion on: handoff median 9 and the 22-of-28 share at 8+; test-line median at or under the code median (v16 is 65 against 120); `needs you` already near the floor; poll turns per lane at or under 6.7 (milestone 2 moves this, milestone 1 must not give it back).

A 600-line code cap is already in `/dispatch` and `/ship`. v16 broke it twice (`CORE-4370` at 2,188, `CORE-4246` at 862), and both scored 6. Criterion 8's peak cap is the cost-side version of that rule. A lane over 600 code lines in the scored version is a miss of the existing cap, reported beside criterion 8.

## Data gaps

1. **One powered version.** v16 is the only row with ≥25 lanes. v1–v15 and v17 are under 25. v8 and v9 have 0 lanes. v17's token jump (32.5M vs 19.1M, subagent 7.4M vs 1.6M) and its 5-of-6 gate rate are six lanes from the first day.
2. **One repo, one CLI.** All 154 lanes are Claude on `naiss-ride/mobile-app`. `lane-stats` defaults to that clone (`DEFAULT_REPO` in `~/dotfiles/bin/lane-stats`). Profiles also exist for passage, strain, and shepherd (`~/dotfiles/claude/pipeline/profiles/`). This file has no numbers for them, and none for Codex or Cursor.
3. **Escapes are leads.** 192 leads on 65 lanes in this run (v16: 12 lanes, 28 lead entries). This pass did not read the diffs. The 2026-11-02 report read 34 v16 leads and confirmed 0, and called one v15 case likely and cost-only (`CORE-4373` from `CORE-4282`, extra CI shard runs). `origin/main` was not fetched. Follow-up issues (75) were not re-read either. v16 same-key follow-ups are 0.
4. **Tokens are not dollars and not unique prompt tokens.** Parent context includes cache reads. Subagent context (0.46B all-time, 256M of it in v16) is counted apart, and some of it also lands back in the parent. No model price is stored on the lane.
5. **Confound.** The 41.9M vs 16.0M flat-round split and the 35.8M vs 20.8M head-review split move with lane size. The 600–999 code-line band is median score 6.5 and median 43.1M (n=10), against score 9.0 and 16.8M under 200 lines (n=64).
6. **`plan_check` null means absent, not "skipped on purpose",** until v17's handoff line. v16 cannot separate a correct skip from a miss except by the no-PR titles above.
7. **`diagnose` is not in `lane-stats`.** The 0-of-40 figure is the 2026-11-01 transcript scan.
8. **Poll turns are matching tool calls, not minutes.** They mix prescribed review waits with leftover sleeps.
9. **Two missing transcripts** (`CORE-4014`, `CORE-4054`), and 26 lanes whose token totals are shares of a stack session.
10. **Reviewer rate is lifetime** (593 reviews), while the reviewer changed at v2, v4, v9, and v16. Per-version actionable rate is not in the JSON.
11. **v16's count moved** from 40 lanes in the 2026-11-01 report to 39 in this run, because v17 had started and taken its own lanes.
