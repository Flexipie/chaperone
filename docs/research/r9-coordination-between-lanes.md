# R9: Coordination between lanes

Status: draft, 2026-11-02. Design for VISION bet 4 (lanes that know about each other). Storage is [R5](r5-state-and-storage.md). The phase graph is [R7](r7-process-engine.md). This is milestone 3 in [VISION.md](../VISION.md): after the ledger and after event wake-ups. It does not add gates to the v17 document.

## Recommendation

**Path and seam claims in the ledger, granted before `plan_to_build`, plus a closed inbox the harness writes and the lane pulls.** Two lanes do not negotiate. A file has one writer. A seam is a name with a revision a reader pins. A stack is one lane with ordered layers. A message exists only when a pin is stale, a declared base actually changed, a blocker merged, or the spec contradicts itself.

The channel R4 drafted as `lane.message` accepts four harness-emitted types and two agent replies (`seam.ack`, `seam.reject`). There is no free-text type. Sibling summaries, file lists, and transcripts stay out of the other lane's context. `lane.read` returns this lane's claims, pins, and inbox fold.

A2A does not carry this. [R4](r4-agent-cli-integration.md) already put it here: A2A is HTTP discovery of agent cards. These lanes are rows in one local SQLite file. A discovery server would add a network process (against bet 7) and a free-form message part (against the chatter bound below).

## What R2 shows, and what it does not count

[R2](r2-own-evidence.md) has no overlap counter. Coordination fails in ways the skills already describe and the ledger does not record. The numbers below are the cost of discovering a clash after the code exists, or of carrying it as prose.

Since v4 (`pipeline-changelog.md`, 2026-10-13) the rules have been prose:

| Check | When it runs | What it can see |
| --- | --- | --- |
| `/dispatch` schedule | Before filing, if the agent follows it | The table: disjoint files run in parallel; overlapping files, or "B needs A's code" without `pr.stack`, wait; with `gh-stack`, one lane builds A then B. Seams are a table of name, owner, shape. |
| `lane-brief` | `/issue` step 1 and `/ship` step 6, if the agent runs it | Newest handoff summary, a regex on `seams:` lines, `git diff --name-only` intersection with siblings (`~/.local/bin/lane-brief`). |
| `/issue` seam stop | Before coding, if the agent notices | A shape that disagrees with the spec or a sibling's reported line, or a sibling already editing the same files. |
| `/ship` sibling re-check | At handoff, after the branch exists | Shared files, or a seam this branch still calls in the old shape. That becomes `open_questions` and `needs you`. |
| `/merge` "next to it" | When a person is about to merge | Open PRs and commits on the base that touch the same paths. A fact in `facts.md`, not a refusal. |
| `/orchestrate` | On a wake | A seam conflict that changes another lane's spec is escalated to the person. Anything else is a natural-language prompt into the lane's terminal. |

v16 ran the plan check on 6 of 28 PR lanes ([R2](r2-own-evidence.md) criterion 1). A seam stop that lives in the same skill is skipped the same way. Problem 5 in R2 is this class: the step a script forces is the one that holds.

The review cluster that is a coordination miss when two writers are live, and a search miss when only one lane is live:

- R2 v16 title cluster "an existing helper was reimplemented": **11 findings, 6 lanes**.
- The 2026-11-02 v16 report, theme 4, "hand-rolled code where a repo helper exists": **7 lanes** (4165, 4235, 4246, 4264, 4300, 4303, 4370).

Claims stop a second writer of a **declared** path or seam name. They do not parse the repo for a helper nobody listed. That half stays a review finding (R10) and a plan-check input. Saying the lease solves theme 4 would over-claim.

Stacks and rebases, which R2 does count:

- **26 lanes** share a worktree transcript (20 across two keys, 6 across three). A stack is one session. Token sums for those lanes are split evenly, which is a measurement bug for R11, not a reason to message between the keys.
- Problem 3: merged with the newest review not on the head. v16 is 15 of 28. v17 lane `CORE-4387` recorded a verdict on the head and is still not `head_reviewed`: PR #3130 unreviewed, #3131 a clean rebase.
- `lane-stats` `lane_patch_id` already treats a clean rebase or a moved stack base as the same diff (`git patch-id --stable`, generated files left out). pstack tells the agent to do that comparison in prose ([R1](r1-landscape.md)).

Seam shape moves as text. `lane-brief` pulls a `seams:` line out of the handoff summary with a regex. The next `/dispatch` run copies those lines into the spec for lanes that have not started. A lane already building keeps the old shape until its next `lane-brief`, which the skill asks for twice, at plan and at handoff. The mid-flight change is invisible between those pulls, and the pull pastes up to 400 characters of the sibling's summary into the reader.

Chatter already has a bill. Poll turns are 1,969 of 22,847 (8.6%), v16 is 6.7 per lane. `/orchestrate` reads the terminal and sends a prompt. A second prose channel between lanes would add turns on top of that. Milestone 2's bound is to hold 6.7 poll turns per lane. This design's waits are inbox rows and the `blocked` overlay, so an idle nudge is not how a lane hears that a file came free.

## Prior art

| | What it actually does | What this design takes | What it leaves |
| --- | --- | --- | --- |
| **GitHub merge queue** | On entry, builds a temporary `merge_group` of the base plus PRs ahead, runs required checks, and evicts a PR whose group fails, conflicts, or times out. Later groups are rebuilt without it. | A result that depended on a neighbour is invalid when that neighbour drops out. Overlap is checked at intent time, which is the edit-time version of "do not discover the clash after both are green". | The queue itself. Landing stays `gh pr merge` or the repo's own queue. The harness does not batch, speculate, or retest. |
| **Not-rocket-science / Bors** | Merge only a commit that was tested as the merge result, not as the PR against an older base. | Same rule, applied to a pin: a reader who built against seam revision N is not "current" after revision N+1. | A bot conversation (`bors r+`) as the interface. |
| **Graphite, `gh stack`, Gerrit relation chains, ghstack** | Ordered dependent branches. Restack rebases each layer onto its parent. `gh stack rebase` is a cascading rebase; a conflict stops the cascade. Gerrit models the same chain as related changes. ghstack submits the chain as GitHub PRs. | A stack is ordered layers, one writer, each PR's base is the previous branch. Restack rewrites shas. `patch-id` separates a clean restack from a real change. This is already the `pr.stack = "gh-stack"` path in `/dispatch` and `/ship` step 7. | Downstack review comments, a merge bot per layer, and a second lane per layer. Graphite's `gt` tracking is the stack record; the harness stores that record in the ledger so a restack is an event. |
| **Claude agent teams and cross-session messaging** | Experimental teams: a lead, a shared task list, task claim via a file lock, and a JSON mailbox of plain text. Idle teammates notify the lead with their final answer. Docs say teams do not put teammates in worktrees, so the person partitions files. Cross-session `SendMessage` is text, and Claude may send it without being asked. | A mailbox, a claim, and "two writers of one file is a bug". The claim is on the path, in the ledger, and the worktree split already exists (one lane, one worktree). | Plain text, lead notifications, and the model choosing to talk. [R1](r1-landscape.md) found the same shape in Superset's orchestrator, Factory Mission Control, and Devin's coordinator: a parent prompt that resolves conflicts. Rebuilding that prompt is the part R1 called already done, and the part that costs turns. |
| **CRDTs (Automerge and the JSON CRDT line)** | Concurrent edits converge without a lock. A revision vector answers "have I seen this update". | The revision counter on a seam. "Stale" is a compare of hashes, which is why a matching publish sends nothing. | Merging two edits of one source file. The converged text has no single author, and the clash is the bug the schedule was written to prevent. |
| **Leases (Gray and Cheriton 1989, Chubby, etcd, R5)** | A lock with an expiry the holder refreshes. Clients cache until expiry or an explicit invalidation. A dead holder does not keep the lock. Chubby is that idea plus consensus; etcd exposes the same expiry as a lease id. | Path claims and seam ownership expire with the lane lease from [R5](r5-state-and-storage.md). Heartbeat renewal updates the expiry and writes no event. One SQLite `BEGIN IMMEDIATE` is the lock service. Published seam revisions remain after the lease expires (the code is in git). | A consensus protocol. One machine, one writer, already decided in R5. |
| **CODEOWNERS** | A static path-to-reviewer map in the repo. | The spec's per-slice file list and seam owner are a reservation written at dispatch. A reservation outlives a crashed session, so a sibling cannot take the file during a lease gap. | A file in the repo as the source of truth. The reservation is a ledger row projected from the approved spec. Review assignment stays R10. |

## Objects

Four records, all folded in the same transaction as their event ([R5](r5-state-and-storage.md): one write function, refused work writes no phase change).

**Reservation.** Written when a spec is approved, not by the lane. Unique on `(repo, path)` and on `(repo, seam name)`. The owner is the slice the spec names. `shared_paths` from the repo profile are excluded (generated trees, lockfiles the profile lists). A lane intent cannot declare a prefix. A prefix is how two lanes each believe they own a disjoint file inside `src/`. Exact paths only. The profile may list a prefix such as `_generated/`; a lane may not.

**Live claim.** The reservation's owner, while its lane lease is unexpired, holds the write. Unreserved paths (a file nobody's slice listed) are first-come: the grant inserts the claim. Expiry matches the lane lease. Renewing the lease renews the claim in that same heartbeat update, with no event. Takeover of that lane (`lane.taken_over`) keeps the claims. A different lane does not inherit them. When the lease expires, an unreserved path is free. A reserved path stays reserved until the slice is `done`, `cancelled`, or a person reassigns it. A dead session does not hand its slice's files to a neighbour, and it also does not pin an unreserved path forever.

**Seam.** A spec name (`rides.listForViewer`), an owner lane, a shape string of at most 500 characters (the brief cap in `/ship`), and `shape_hash`. Revisions are monotonic. One publish per `(seam, head sha)`. The same hash again is an idempotent no-op, no event, same rule as a gate recorded twice with the same status.

**Pin.** A lane that `reads` a seam stores the revision and hash it compiled against. Status is `current`, `stale`, `acked`, or `rejected`. Revision 0 means "the spec shape, nothing published yet".

**Stack.** One row per stack lane, not per layer-as-lane. Layers are ordered positions: issue key, PR, base ref, head sha, `patch_id`. Path claims belong to the lane, so layer 2 does not contend with layer 1.

**Inbox.** R5's `messages` table is the log of `message.sent`. The fold the agent sees is one open row per `(to_lane_id, type, subject)`. A newer event for the same subject supersedes the previous. `lane.read` reads the fold. The log stays for the count R2 does not have.

```sql
-- Reservations: the schedule. Do not expire with the lease.
CREATE TABLE path_reservations (
  repo     TEXT NOT NULL,
  path     TEXT NOT NULL,
  lane_id  INTEGER NOT NULL,
  spec_id  INTEGER NOT NULL,
  event_id INTEGER NOT NULL,
  PRIMARY KEY (repo, path)
);

CREATE TABLE seam_reservations (
  repo     TEXT NOT NULL,
  name     TEXT NOT NULL,
  lane_id  INTEGER NOT NULL,
  spec_id  INTEGER NOT NULL,
  shape    TEXT NOT NULL,
  shape_hash TEXT NOT NULL,
  event_id INTEGER NOT NULL,
  PRIMARY KEY (repo, name)
);

-- Live writes. expires_at tracks the lane lease.
CREATE TABLE path_claims (
  repo       TEXT NOT NULL,
  path       TEXT NOT NULL,
  lane_id    INTEGER NOT NULL,
  spec_id    INTEGER,
  expires_at TEXT NOT NULL,
  event_id   INTEGER NOT NULL,
  PRIMARY KEY (repo, path)
);

CREATE TABLE seams (
  id              INTEGER PRIMARY KEY,
  repo            TEXT NOT NULL,
  name            TEXT NOT NULL,
  owner_lane_id   INTEGER NOT NULL,
  owner_expires_at TEXT NOT NULL,
  UNIQUE (repo, name)
);

CREATE TABLE seam_revisions (
  seam_id    INTEGER NOT NULL,
  revision   INTEGER NOT NULL,
  shape      TEXT NOT NULL,
  shape_hash TEXT NOT NULL,
  head_sha   TEXT NOT NULL,
  event_id   INTEGER NOT NULL,
  at         TEXT NOT NULL,
  PRIMARY KEY (seam_id, revision)
);

CREATE TABLE seam_pins (
  lane_id    INTEGER NOT NULL,
  seam_id    INTEGER NOT NULL,
  revision   INTEGER NOT NULL,
  shape_hash TEXT NOT NULL,
  status     TEXT NOT NULL, -- current | stale | acked | rejected
  PRIMARY KEY (lane_id, seam_id)
);

CREATE TABLE stacks (
  id      INTEGER PRIMARY KEY,
  repo    TEXT NOT NULL,
  lane_id INTEGER NOT NULL UNIQUE
);

CREATE TABLE stack_layers (
  stack_id  INTEGER NOT NULL,
  position  INTEGER NOT NULL,
  lane_key  TEXT NOT NULL,
  pr        TEXT,
  base_ref  TEXT NOT NULL,
  head_sha  TEXT,
  patch_id  TEXT,
  PRIMARY KEY (stack_id, position)
);
```

`messages.subject` (a path, a seam name, or a layer id) is the coalesce key. `to_lane_id` is required except for `overlap.needs_human`, whose audience is the human queue (R12 decides whether it interrupts).

Event kinds added to R5's list: `reservation.written`, `intent.granted`, `intent.denied`, `path.released`, `seam.published`, `seam.acked`, `seam.rejected`, `message.sent`, `stack.rebased`, `stack.layer_opened`. `intent.denied` is a `gate.denied` on `plan_to_build` (R7 already appends that and does not move the phase). A retry with the same idempotency key returns the same denial and does not append again.

## Grant: two lanes, one file

`plan_to_build` requires `intent_granted`. The intent lists exact write paths, seam names to own, seam names to read, and any `blocked_by` lane. The transaction is all or nothing: one conflicting path grants nothing. A partial grant is a lane that has started writing and then stalls mid-diff.

The grant, inside the `BEGIN IMMEDIATE` from R5:

1. A path on this lane's reservation, unclaimed or claimed by this lane: grant.
2. A path on another lane's reservation: deny, even if that lane has not started. The schedule wins over a race.
3. A path with no reservation and no live claim: grant, and insert the claim. The first commit wins.
4. A path with a live claim by this lane: grant (idempotent).
5. A path with a live claim by another lane: deny.
6. A seam `owns` where this lane holds the reservation: grant.
7. A seam `owns` where another lane holds the reservation, or a live owner holds the name: deny.
8. A seam `reads`: pin the current revision. If nothing is published, pin revision 0 and the spec hash.
9. Open PRs that are not this lane's own layers count as reservations owned by `pr:<n>`, from the path list `/merge` already collects. A human PR and a lane share the rule.
10. `shared_paths`: ignored by every step above.

A deny of kind "held" or "reserved" uses R7's `blocked` overlay: `resume_phase` is `planning`, the blocker is `path:<repo>:<path>:lane:<id>` (or `pr:<n>`). The phase returns to `planning` when the blocker clears. The holder receives no row. The waiter sees the blocker on `lane.read` because it is its own denial, not because anyone sent it mail.

A deny of kind "spec lists two owners" or "this path is on two slices' file lists" grants nothing to either lane and writes one `overlap.needs_human`. That is a slice error from dispatch. First-come would pick a winner the spec did not name.

An amendment later in `building` (a new file) is a second intent for the new paths only. Previous grants stay. If the new path conflicts, the amendment is denied and the lane blocks before writing it. If the path is already in the diff, `ask_verdict` fails `no_undeclared_overlap` until that path is gone from the diff or the lane holds the claim.

`ask_verdict` and `to_ready` also require `no_undeclared_overlap`: every path in `git diff --name-only <merge-base>...HEAD`, minus `shared_paths`, is either claimed by this lane or reserved for it. The intersection includes other non-terminal lanes and open PRs. This is the backstop for a commit that skipped the intent (migration, R15, a lane that edited past its grant). It is the `/ship` sibling re-check and the `/merge` "next to it" list, run as a gate before ready, while the other lane still has time to be the only writer.

`base_moved_under` on the same transitions: a path this lane holds that has new commits on the default branch since the merge-base fails the gate until this head contains them. The lane rebases. The lane that already merged is not notified.

What the matching publish and the idle holder do not get: a message. The common case, two lanes with disjoint reservations, writes zero inbox rows. That is the chatter test.

## Seams: a change mid-flight

The owner publishes with `seam.publish` and the shape string. The harness writes the revision. Agents do not write `seam.revised` themselves.

| Publish | Pins that already match the new hash | Pins on an older hash | Lanes that have not declared `reads` |
| --- | --- | --- | --- |
| First publish, hash equals the spec reservation | Advance to this revision, status `current`. No inbox row. | Not possible if they pinned the spec. | No row. Their later intent pins this revision in the grant response. |
| Hash changed | n/a | Status `stale`. One inbox row `seam.revised`: name, from revision, to revision, shape, hash, owner lane. | No row. They never held the old pin. |
| Same hash as the latest revision | No event. | No event. | No event. |

A second publish before ack replaces the open inbox row for that `(lane, seam)`. The log keeps both `message.sent` events. `lane.read` returns one. Ack of a revision older than the latest is refused.

The reader keeps building. A stale pin does not enter `blocked`. `ask_verdict` and `to_ready` require `seams_current`: every pin is `current` or `acked` on the latest revision. `seam.ack` records that this lane now compiles against that revision. The owner is not told. `seam.reject` sets the pin to `rejected`, moves this lane to `needs_you`, and writes nothing to the owner. A reject means the new shape changes the other lane's spec, which `/orchestrate` already escalates to a person. A reply to the owner would be the debate the inbox is here to avoid.

A publish nobody reads writes a revision and zero messages. A reader who starts afterwards receives the current shape inside the grant. That replaces the v4 loop where dispatch copies `seams:` lines into the spec on a later run, too late for anyone already in `building`. The spec's seam section becomes a projection of `seam_revisions` plus reservations. The handoff line `seams: <name>: <shape>` is then a migration input (R15 parses it once into a publish), not the channel.

Limit, on purpose: v1 does not parse the file. The shape is the string the owner submits. A behavior change that leaves the signature alone does not bump the hash and does not wake readers. Dispatch defines a seam as a name and an agreed shape, and that is the bug this channel is for (a sibling still calling the old arguments). A silent behavior change inside a stable signature is a review finding. R10 can later check the submitted shape against the tree. Until then, `seams_current` checks that a publish exists for this head sha on every seam this lane owns. A missing publish on a sha that touched a seam file the lane owns fails `ask_verdict`. The file-touch check is path-level (the seam's declared file, stored on the reservation), not an AST diff.

## Stacks

Dispatch's rule stands: if B needs A's code and the profile has `pr.stack = "gh-stack"`, they are layers of one lane. The harness does not invent a cross-lane stack to be helpful. Two lanes that both want the same path fall under the grant above. The waiter stays blocked until `dependency.cleared` (below), which is "A merged", not "rebase onto my branch" as a sentence from A.

Inside the stack:

- One claim set for the lane. Layers do not message each other.
- Each layer has its own PR, head sha, review, and verdict. The base of layer N is layer N-1's branch, or the default branch for layer 1. `/ship` step 7 (handoff this layer, `gh stack add`, claim the next key, run `/issue`) is the same sequence. The ledger records `stack.layer_opened` so the order is not only in the session.
- `gh stack rebase` rewrites shas. The harness records `stack.rebased` with the old sha, the new sha, and the new `patch_id` per layer whose sha changed. Recipients of that event, as an inbox row, are empty when the only lane involved is the stack's own lane. The upper layer is the same session, or a later session of the same lane (`lane.read` shows the new shas).
- `patch_id` is `git patch-id --stable` on the layer diff against its parent, generated files excluded, which is `lane_patch_id` in `lane-stats`. A clean rebase (same patch-id, new sha) sends no `seam.revised` when the seam hash is unchanged, and no `base.moved`. Gate results stay keyed by sha ([R5](r5-state-and-storage.md), [R7](r7-process-engine.md) `verdict_on_head`). `CORE-4387` (#3131 a clean rebase, verdict recorded, review not on that sha) stays a failed `reviewed_head_ok`. This design does not add a patch-id exception to that predicate. It only refuses to wake other lanes for a restack that did not change a pin.
- A lower layer whose seam hash changes notifies readers of that seam, through the publish rule. The upper layer of the same lane is the owner, so it is not a reader of its own seam.
- `stack_base_live` on `ask_verdict` and `merge_on_yes`: the PR base is the default branch, or an open PR of a lower layer in this stack. A base whose PR already merged, which `/merge` calls a dead stack base, fails the gate. The lane restacks (`gh stack sync` / rebase onto the new trunk). The denial is the signal. There is no inbox type for "your own base merged".
- A stack longer than three layers stays one lane. `/dispatch` already warns that a longer chain grows one session's context. Splitting it into chatting lanes would spend the tokens the warning exists to save.

`base.moved` is the rare row: a different lane declared `stacks_on` a layer, and that layer's patch-id changed. Dispatch's table does not create that relationship (the other lane waits for merge, or joins this stack). The type exists so a recovered accident has a typed signal. If nobody declares `stacks_on`, it is never sent.

`dependency.cleared` is the common wake. A lane whose spec says `blocked_by` A gets one inbox row when A reaches `done`, carrying A's lane id and merge sha. A does not send it. The harness does, once. The waiter pulls it and retries `plan_to_build`. No reply type.

## Inbox

| Type | Who writes it | Who receives it | Subject | Body |
| --- | --- | --- | --- | --- |
| `seam.revised` | Harness, on publish, and only for a pin whose hash differs | That reader | seam name | revisions, shape (≤500), hash, owner lane |
| `base.moved` | Harness, when a layer's patch-id changes | A different lane that declared `stacks_on` it | layer id | old sha, new sha |
| `dependency.cleared` | Harness, when the blocker lane is `done` | The waiter | blocker lane id | merge sha |
| `overlap.needs_human` | Harness, when the spec reserves one path or seam for two lanes | Human queue (`to_lane_id` null) | path or seam name | the two lane ids |

Agent-callable, and they are events, not inbox types: `intent.submit`, `seam.publish` (owner of that seam only), `seam.ack`, `seam.reject`.

Refused, no row: any other `lane.message` type, a publish by a non-owner, an ack of a stale revision, a second `overlap.needs_human` for a subject that already has an open one, a broadcast to every lane on the spec.

Pull, not push. Delivery is the next `lane.read`. Writing the same text into the terminal, or having `/orchestrate` paste it, would bill the row twice. The orchestrator stops prompting a lane about a seam once the inbox has the row. Idle nudges do not apply to a lane in `blocked` with a coordination blocker; the wake is `dependency.cleared` or `path.released` (the release is an event the waiter already filters for, not a message to the holder).

Bounds:

- One open inbox row per subject. Newer supersedes.
- Zero rows when the hash matches, when the patch-id matches, when the reader has not pinned, and when the two intents were disjoint.
- The owner never observes ack.
- Reject and a contradictory spec go to the human queue. They do not go back to the other lane.
- Shape text over 500 characters is a refused publish. The seam is a signature, not an essay.
- One publish per seam per head sha.

R4's MCP tool `lane.message` shrinks to ack and reject. The other four types are not on the tool. A model that can call a free-form send will use it (Claude's cross-session messaging is documented as allowed to fire unasked). Removing the call is the bound. Context injected for coordination is the fold, not `lane-brief`'s sibling summary.

## Gates on the v17 graph

These predicates are host functions, added in the pipeline version that turns coordination on. They are not retrofitted into the v17 document R7 specifies. A lane stamped v17 does not run them.

| Predicate | Holds when | Named on |
| --- | --- | --- |
| `intent_granted` | A granted intent covers the declared paths and seams, and every `blocked_by` lane is `done`. | `plan_to_build` |
| `no_undeclared_overlap` | Diff paths, minus `shared_paths`, are claimed or reserved for this lane. No other live lane or foreign open PR holds them. | `ask_verdict`, and therefore `to_ready` via `ready_evidence` once that version adds it |
| `seams_current` | Every pin is `current` or `acked` on the latest revision. Every seam this lane owns has a publish on this head sha if the diff touches that seam's file. | `ask_verdict` |
| `base_moved_under` | No held path has commits on the default branch since the merge-base that this head lacks. | `ask_verdict` |
| `stack_base_live` | Not a stacked layer, or the base is the default branch or an open lower layer of this stack. | `ask_verdict`, `merge_on_yes` |

Facts the projection adds for those predicates: `intent.granted`, `overlap.paths` (the conflicting list, empty when clean), `seams.stale` (count), `stack.base_live`, `stack.patch_id`, `base.paths_moved`.

`enter_blocked` stays the overlay R7 already defines. Coordination blockers use it from `planning` (cannot start) and from `building` (amendment denied). `needs_you` is the existing target for `seam.reject` and for `overlap.needs_human`.

## Cases

Each row is a case the grant, the publish rule, or a gate has to accept in a test. The fixture is facts in, one result out, the same shape as R7's `to_ready` denial.

| # | Case | Result | R2 / today's process |
| --- | --- | --- | --- |
| 1 | Two lanes, one path, spec reserves it for A. B submits intent. | No claim for B. B `blocked`, resume `planning`, blocker names A and the path. A's inbox empty. No human row. | `/dispatch` "files overlap, no logical dependency" and `/issue`'s sibling-file stop. Today B finds out by running `lane-brief`, or at `/ship` after both have written. |
| 2 | Same path on both slices' lists (spec contradicts itself). Both intents in either order. | Zero claims. One `overlap.needs_human`. | A dispatch miss. First-come would hide it. |
| 3 | Disjoint reservations, both intents. | Both granted. Inbox empty. | The parallel row of the schedule. The chatter test: a clean dispatch produces no messages. |
| 4 | Unreserved path, A granted, then A's lease expires, B requests it. | B is granted. A's later resume is denied and blocks. Seam revisions A published stay. | R5 lease expiry. A reservation would have kept B out; this row is the unreserved path only. |
| 5 | A holds the claim and is alive. B requests the path. | B denied. A is not notified. | The race `lane-brief` sees only after both diffs exist. |
| 6 | B already committed the path (no grant). `ask_verdict`. | `gate.denied` / `no_undeclared_overlap`. Phase unchanged. | `/ship` step 6 sibling re-check, moved before ready. |
| 7 | Owner publishes the spec hash. Reader pinned revision 0 at that hash. | Pin becomes `current`. No inbox row. | First publish of the shape dispatch already agreed. The `seams:` copy on the next dispatch run is unnecessary. |
| 8 | Owner publishes a new hash. Reader pinned the old one and has commits. | One `seam.revised`. Reader stays in `building`. `ask_verdict` fails `seams_current` until ack. Owner's inbox empty. | "Seam changed mid-flight." Today the reader sees it at the next `lane-brief`, often at handoff, as `needs you`. |
| 9 | A further publish arrives before ack. | The open row is replaced. Ack of the older revision is refused. Log has both sends. `lane.read` returns one. | The anti-chatter rule on a noisy owner. |
| 10 | Reader calls `seam.reject`. | Pin `rejected`, lane `needs_you`. Owner is not a recipient. | `/orchestrate`: a seam conflict that changes the other lane's spec goes to the person. |
| 11 | Nobody has pinned. Owner publishes twice, different hashes. | Two revisions, zero inbox rows. A later reader's grant returns the latest shape. | In-flight lanes that have not started do not get a history of shapes they never used. |
| 12 | Stack of three layers, one lane. Layer 1 restacks, patch-id unchanged, seam hash unchanged. | `stack.rebased` recorded. Inbox empty, including the upper layers. Verdict on the old sha does not satisfy `verdict_on_head`. | `CORE-4387` #3131. 26 lanes share a stack session. `lane_patch_id` already equates the clean rebase. |
| 13 | Layer 1 changes a seam hash. A different lane had pinned it. The upper layer is the same lane. | The other lane gets one `seam.revised`. The upper layer gets none. | `/ship` step 7 builds the next layer in-session. It does not need a message from itself. |
| 14 | Layer 1's PR merged, layer 2 still targets that branch. | `stack_base_live` denies `ask_verdict` and `merge_on_yes`. No inbox row to this lane. | `/merge` hard gate "dead stack base", earlier than merge time. |
| 15 | Spec says B is `blocked_by` A. A merges. | One `dependency.cleared` to B, then B's intent can grant. A sends nothing. | "B waits for A to merge." The vision's "rebase on mine", as a harness event with a sha, once. |
| 16 | Foreign open PR touches a path B wants. | B denied, blocker `pr:<n>`. | `/merge` "next to it", applied at intent time. |
| 17 | Profile `shared_paths` contains the path both lanes list. | Both granted. Intersection ignores it. | Generated files and lockfiles. Default list is empty. A lane cannot add to it. |
| 18 | Agent calls `lane.message` with a free-text type. | Refused. No event, no row. | Claude cross-session messaging and agent-team mailboxes. The cost is the poll and nudge turns in R2. |
| 19 | Lane owns seam S, diff touches S's file, no publish on this sha. | `ask_verdict` fails `seams_current`. | Closes the hole where the shape channel is optional prose on the handoff. |
| 20 | Two parents, one repo, same path reserved for a lane on the other spec. | The reservation is `(repo, path)`. The second spec's lane is denied. | Claims are not per spec. R2's lanes are one repo; the rule is for the next repo anyway. |

Cases 1, 5, and 6 are "two lanes, one file" (before code, at a race, and after both wrote). Cases 7 through 11 and 19 are "a seam changed mid-flight". Cases 12 through 15 are "a stack".

## What this does not do

- It does not catch a lane that reimplements a helper in a file only it touches, when the spec never named that helper. R2's 11 findings and the v16 theme-4 lanes stay a plan-check and review problem unless the seam was declared.
- It does not merge concurrent edits. A CRDT on the source file is rejected above.
- It does not run a merge queue, and it does not replace `gh stack`.
- It does not treat a clean rebase as a reviewed head. R7 keeps that rule.
- It does not wake a lane by prompting its terminal. Milestone 2 delivers the inbox by subscription. Until that subscription exists, the row still waits in `lane.read`, and `/orchestrate` must not paste it.
- It does not version these gates into v17. Lanes already stamped v17 finish on the R7 document.

## What this leaves open

- R10: checking a submitted shape against the tree, and whether a behavior change inside a stable signature ever becomes a revision. Not a v1 coordination message.
- R11: counting `intent.denied`, `seam.revised` (acked vs rejected), `no_undeclared_overlap`, and inbox rows per lane. The median parallel lane should be zero inbox rows. Stack sessions that share a transcript (26 lanes in R2) need a token split that is not "divide by the number of keys".
- R12: when `overlap.needs_human` and `seam.reject` may interrupt. This note only writes the row.
- R15: one adapter pass that turns existing `seams:` summary lines and spec seam tables into reservations and revisions, then stops parsing them on the hot path.
- The pipeline version that names the five predicates. Writing that TOML is a process change with a cohort, not part of this design.

## Sources

- [RESEARCH.md](../RESEARCH.md) R9, [VISION.md](../VISION.md) bet 4 and milestone 3
- [R1](r1-landscape.md) bet 4 (ownership and a typed channel unserved; coordinator prompts already exist), [R2](r2-own-evidence.md) (v16 clusters, `CORE-4387`, stack transcripts, poll turns), [R4](r4-agent-cli-integration.md) (`lane.message`, A2A parked), [R5](r5-state-and-storage.md) (leases, `messages`, one write transaction), [R7](r7-process-engine.md) (`plan_to_build`, `ask_verdict`, `to_ready`, `blocked`)
- `~/.claude/skills/dispatch/SKILL.md` (schedule table, seams), `issue/SKILL.md` (seam stop, `lane-brief`), `ship/SKILL.md` (sibling re-check, `seams:` line, stack step 7), `merge/SKILL.md` (dead stack base, overlapping PRs), `orchestrate/SKILL.md` (what may be prompted)
- `~/.local/bin/lane-brief`, `~/.local/bin/lane-stats` (`lane_patch_id`), `~/.claude/pipeline-changelog.md` `## v4`
- `~/dotfiles/claude/pipeline-reports/2026-11-02-v16.md` theme 4
- GitHub merge queue: [Managing a merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue) (temporary `merge_group`, eviction, rebuild without the failed PR)
- Graydon Hoare, [The Not Rocket Science Rule of Software Engineering](https://graydon2.dreamwidth.org/1597.html)
- Graphite restack: [Restack branches](https://graphite.com/docs/restack-branches). GitHub CLI stacks: [Managing stacked pull requests](https://docs.github.com/en/pull-requests/how-tos/create-pull-requests/managing-stacked-pull-requests). ghstack: [ezyang/ghstack](https://github.com/ezyang/ghstack). Gerrit relation chains: [Changes](https://gerrit-review.googlesource.com/Documentation/concept-changes.html)
- Claude agent teams: [Orchestrate teams of Claude Code sessions](https://code.claude.com/docs/en/agent-teams) (task-claim file lock, plain-text mailbox, no worktree isolation). Cross-session text: [Message your other Claude Code sessions](https://code.claude.com/docs/en/cross-session-messaging)
- Automerge (convergent JSON, the property source files should not have here): [automerge.org](https://automerge.org/)
- Leases: C. Gray and D. Cheriton, *Leases: An Efficient Fault-Tolerant Mechanism for Distributed File Cache Consistency*, SOSP 1989. Chubby: [Burrows, OSDI 2006](https://www.usenix.org/legacy/event/osdi06/tech/full_papers/burrows/burrows.pdf). etcd: [Why leases](https://etcd.io/docs/latest/learning/why/)
- `git patch-id --stable`: [git-patch-id](https://git-scm.com/docs/git-patch-id)
- CODEOWNERS, as the static version of a reservation: [About code owners](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-security/customizing-your-repository/about-code-owners)
- A2A, rejected for this channel: [A2A and MCP](https://a2a-protocol.org/latest/topics/a2a-and-mcp/)

## Decision outputs (for `docs/decisions/`)

1. Coordination is **claims, reservations, seam revisions, and a four-type inbox**. It is not a coordinator agent, a CRDT on source files, a merge queue, or A2A.
2. **Two lanes never both hold a write path.** The spec reservation beats a race. A contradictory spec pages a person once. The holder is not told that someone else was denied.
3. **A seam message fires only when a pin's hash is stale,** one open row per seam, replaced on a newer publish. Ack is silent. Reject goes to the person.
4. **A stack is one lane.** A clean rebase (same patch-id) notifies nobody and does not satisfy a sha-keyed verdict. A dead stack base fails `stack_base_live`.
