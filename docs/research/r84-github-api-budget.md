# R84: GitHub API calls from parallel lanes and the watcher

Status: 2026-11-07. Evidence for issue [#84](https://github.com/Flexipie/chaperone/issues/84).

## Answer

- **The watcher makes about 95% of the calls, and it makes them in bursts.** One `lanes --json --all` pass (what
  `lanes-watch` runs) made 136 GitHub calls. It started 135 of them in the first 10 seconds and had up to 51 running
  at once. 132 of the 136 were for lanes already done: 120 `gh issue view` (one per done GitHub-tracked work item,
  to show a tracker state that can no longer change) and 12 `compare` calls on merged PRs.
- During the incident two orchestrators (chaperone and mobile-app CORE-4547) each ran `lanes-watch --interval 300`.
  Each pass covers every work item in both ledgers, whatever `--keys` says, and a watcher runs one pass right away
  each time it is restarted after a wake.
- **Lanes are small.** About 30 calls per lane-hour (14 to 97 across 15 lanes), and CI waits already poll once a
  minute (`gh pr checks --watch --interval 60`, from the `ship` and `merge` skills).
- **At 5 lanes**, before: 2 watchers x at least 12 passes an hour x 136 calls, about 3,300 calls an hour, plus about
  150 from lanes. Each pass is a burst of up to 51 concurrent requests. After the proposed change: one shared pass
  per interval, 9 calls, at most 3 at once (measured), about 110 an hour, plus the same 150 from lanes.
- **The fix is in dotfiles, not in Chaperone.** Chaperone makes no GitHub calls. The two patches below change
  `lanes.sh` and `lanes-watch`. The owner applies them (issue comment, 2026-11-07).

## Why the secondary limit and not the hourly one

GitHub's [secondary rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#about-secondary-rate-limits)
allow at most 100 concurrent requests (REST and GraphQL together), 2,000 GraphQL points a minute (1 per query
without a mutation) and 60 seconds of GraphQL CPU time per 60 seconds. The hourly budget is 5,000. On 2026-11-06,
GraphQL refused calls from 18:36Z to about 19:41Z with about 4,000 of the hour still left. REST still worked.

One pass alone stays under 2,000 points a minute (136) and under 100 concurrent requests (51). Two watchers whose
passes overlap, plus the lanes' own calls, reach about 100 concurrent GraphQL requests. That fits a refusal on
GraphQL only with the hourly budget left. GitHub does not say which secondary limit it applied, so this is the most
likely trigger, not a proven one. The change removes both the burst and the volume, so it does not depend on which
limit it was.

## How it was measured

**Watcher pass.** A copy of `lanes.sh` with a logging `gh` shim first on its `PATH` (the script prepends
`/opt/homebrew/bin`, so the shim has to go inside its `PATH` line). Each call logs start, end, exit code and
arguments. The pass only reads; the real watchers run the same calls every 5 minutes. Ledgers at the time: 199
daisy work items (106 GitHub, 93 Linear) and 14 Chaperone items, not counting cancelled ones. 205 of the 213 rows
were `done`.

| | Calls | `issue view` | `compare` | `pr view` + threads | Started in first 10 s | Peak concurrent | Pass time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `lanes.sh` today | 136 | 120 | 12 | 4 | 135 | 51 | 41 s |
| with the patch | 9 | 4 | 1 | 4 | 9 | 3 | 17 s |

The two outputs differ only where intended: `tracker_state` becomes null on the 114 done GitHub rows and
`commits_since_review` becomes null on 8 merged PRs. `lane` and `why` are the same on every done row. The other
differences were lanes that moved between the two runs.

**Lanes.** Every `gh` call in Bash commands in Claude transcripts of the worktree sessions with activity from
2026-11-06 17:00Z to 20:30Z (11 chaperone lanes, 4 mobile-app lanes, 15.3 lane-hours). Background CI waits are
counted as their duration divided by their poll interval, timed from launch to the task notification.

| | Calls | Per lane-hour |
| --- | ---: | ---: |
| direct `gh` calls | 339 | 22 |
| polls inside background waits | 116 | 8 |
| total | 455 | 30 (14 to 97 by lane) |

The busiest lane (#80, 97 per hour) polled `gh api .../actions/runs` through `wait-for` every 30 seconds after
GraphQL refused it. Lanes that hit the limit moved to REST, which still answered.

**Not measured:** the daisy app's own GitHub mirror sync and its reviewer jobs use the same account. They have no
request log to read from outside, so they are not in these numbers.

## Proposed change (dotfiles, for the owner to apply)

Both apply cleanly to `~/dotfiles` at `5214444` with its uncommitted edits (`git apply --check`).

**1. `lanes.sh`: no calls for lanes that are done, and at most 4 lanes at once.** A done work item's issue state is
not asked for again (it is display only: the lane verdict comes from the PR and the work item). `compare` runs only
for an open PR. The per-lane jobs wait for a free slot (`wait -n`, bash 5, which `lanes` already runs under).

```diff
--- a/herdr/local-plugins/next-ticket/lanes.sh
+++ b/herdr/local-plugins/next-ticket/lanes.sh
@@ -125,13 +125,14 @@
     '[.data.items[] | select(.source == "local" and .branch == $b and .review_score != null)][0].head_sha // empty' "$tmp/jobs.json")"
   head="$(jq -r '.headRefOid // empty' <<<"$pr")"
   ahead=null
-  if [ -n "$job_sha" ] && [ -n "$head" ] && [ "$job_sha" != "$head" ]; then
+  if [ -n "$job_sha" ] && [ -n "$head" ] && [ "$job_sha" != "$head" ] && [ "$(jq -r .state <<<"$pr")" = OPEN ]; then
     ahead="$(gh api "repos/$repo/compare/$job_sha...$head" --jq .ahead_by 2>/dev/null)" || ahead=null
   fi
   jq -c --arg repo "$repo" --argjson ahead "${ahead:-null}" '. + {repo: $repo, commits_since_review: $ahead}' <<<"$pr"
 }
 
-# GitHub and Linear per lane, in parallel: gh is the slow part.
+# GitHub and Linear per lane, in parallel: gh is the slow part. At most 4 lanes at once:
+# a burst of concurrent calls trips GitHub's secondary rate limit (chaperone#84).
 items="$(jq -c '.data.items[] | select(.status != "cancelled")' <<<"$work"
   [ -z "$sl_work" ] || jq -c '.data.items[] | select(.status != "cancelled") | . + {ledger: "chaperone"}' <<<"$sl_work")"
 # A row's ledger CLI and file key: daisy rows keep their id, Chaperone rows chaperone-<id>.
@@ -146,6 +147,7 @@
   dkey="$key"
   [ "$src" = issue ] && dkey="$(jq -r --arg id "$key" '.[$id].key // $id' "$tmp/issues.json")"
   lname="$(lane_name "$dkey")"
+  while [ "$(jobs -rp | wc -l)" -ge 4 ]; do wait -n; done
   (
     # The lane's PRs: its pr_ref plus every mirror PR whose branch names the key,
     # opened after the work item (an hour of slack).
@@ -172,7 +174,10 @@
     done <"$tmp/m-$id.jsonl"
     jq -s -c . "$tmp/prs-$id.jsonl" >"$tmp/prs-$id.json"
     case "$src" in
-      github) state="$(gh issue view "${key##*#}" -R "${key%#*}" --json state --jq '.state | ascii_downcase' 2>/dev/null)" ;;
+      # A done lane's issue is not asked for again: most rows are done, one call each per pass.
+      github) state=""
+        [ "$(jq -r .status <<<"$item")" = done ] \
+          || state="$(gh issue view "${key##*#}" -R "${key%#*}" --json state --jq '.state | ascii_downcase' 2>/dev/null)" ;;
       issue) state="$(jq -r --arg id "$key" '.[$id].status // empty' "$tmp/issues.json")" ;;
       *) state="$("$daisy" linear get "$key" 2>/dev/null | jq -r '.data.issue.state_name // empty')" ;;
     esac
```

**2. `lanes-watch`: one pass shared by every watcher.** A pass younger than the watcher's own `--interval` is read
from `~/.local/state/lanes-watch/pass.json` instead of running `lanes` again, so data is never older than each
watcher's interval. Only a full pass (both ledgers) is written, by atomic rename. Two watchers starting in the
same second can still both run a pass; that is at most one extra pass.

```diff
--- a/bin/lanes-watch
+++ b/bin/lanes-watch
@@ -18,7 +18,9 @@
 0 on a change, when every watched lane is done, or after --max-wait seconds with no
 change (timed_out, a heartbeat). Exit 5 when daisy stays closed, 1 on other errors
 (in a Chaperone repo, lanes leaving Chaperone's rows out is one).
-Read-only except FILE.
+Every watcher shares one pass: a `lanes --json --all` younger than --interval, from any
+watcher, is read from PASS instead of run again (GitHub's secondary rate limit,
+chaperone#84). Read-only except FILE and PASS.
 """
 import argparse
 import json
@@ -30,6 +32,7 @@
 from datetime import datetime, timezone
 
 DAISY = "/Applications/daisy.app/Contents/MacOS/daisy-cli"
+PASS = os.path.expanduser("~/.local/state/lanes-watch/pass.json")
 FIELDS = ("lane", "why", "status", "next_action", "agent", "pr", "pr_state", "ci", "mergeable", "draft",
           "people_threads", "daisy_threads", "local_score", "local_head", "pr_score", "has_handoff",
           "tracker_state", "blocker", "pending_reviews")
@@ -39,14 +42,27 @@
     return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
 
 
-def lanes(kind):
+def lanes(kind, max_age):
+    try:
+        if time.time() - os.path.getmtime(PASS) < max_age:
+            return json.load(open(PASS)), 0, ""
+    except (OSError, json.JSONDecodeError):
+        pass
     p = subprocess.run(["lanes", "--json", "--all"], capture_output=True, text=True)
     if p.returncode != 0:
         return None, p.returncode, p.stderr.strip()
     # Outside daisy, lanes left this repo's rows out (its stderr says why): a failed call.
     if kind != "daisy" and "showing daisy lanes only" in p.stderr:
         return None, p.returncode, p.stderr.strip()
-    return json.loads(p.stdout or "[]"), 0, ""
+    rows = json.loads(p.stdout or "[]")
+    # Only a pass with every ledger's rows is shared.
+    if "showing daisy lanes only" not in p.stderr:
+        os.makedirs(os.path.dirname(PASS), exist_ok=True)
+        tmp = f"{PASS}.{os.getpid()}"
+        with open(tmp, "w") as f:
+            json.dump(rows, f)
+        os.replace(tmp, PASS)
+    return rows, 0, ""
 
 
 def ledger():
@@ -95,7 +111,7 @@
     led = ledger()
     start, fails = time.time(), 0
     while True:
-        rows, rc, err = lanes(led["kind"])
+        rows, rc, err = lanes(led["kind"], max(30, o.interval))
         if rows is None:
             fails += 1
             if fails >= 6:
```

Checked on the copies with the pass file under a temporary directory: two watchers one after the other ran `lanes`
once (9 GitHub calls in total), and a pass file older than `--interval` made the next watcher run `lanes` again.

**3. Global `CLAUDE.md`.** Its "Waiting" line gives `gh pr checks N --watch` with no interval, and `gh` then polls
every 10 seconds. Make it `gh pr checks N --watch --interval 60`, as `ship` and `merge` already say.

After the patch, the first pass of each running watcher reports done lanes it watches as changed once
(`tracker_state` goes from `closed` to null). That is one extra wake per watcher.

## What this leaves out, and why

- **PR facts in the ledger.** Chaperone makes no GitHub calls today. Storing PR facts would add a ledger table (a
  schema migration) and a GitHub poller, which is [Phase 2](../ROADMAP.md) "Event-driven waiting" (CI green,
  review done subscriptions). The shared pass file solves today's problem without it.
- **Lanes reading the watcher's pass instead of `gh`.** A lane checks its own PR when it is about to act (CI on
  the head, mergeable before a merge). A pass up to 5 minutes old is the wrong input for that, and lanes make about
  5% of the calls.
- **Backoff on CI waits.** Waits already poll once a minute, about 1 call a minute per waiting lane. Backing off
  further would delay merges and save little.
- **After the owner applies the patches:** measuring that 5 lanes stay well under the limits takes a real run.
  That goes in a `measure:` follow-up issue.
