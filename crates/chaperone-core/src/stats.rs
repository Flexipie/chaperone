//! `stats`: R11's lane and cohort numbers, read from the log (ROADMAP item
//! 7, R11 "First five"). Nothing is stored: one statement reads the
//! cohort's events in lane order and every number is folded from them.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::{Map, Value, json};

use crate::checks::ATTENTION_TYPES;
use crate::ledger::{Checked, Ledger, LedgerError, Refusal, canonical_at, check_schema};
use crate::pipeline::sha_prefix_eq;
use crate::work::IS_HANDOFF;

/// Which lanes `stats` reads.
#[derive(Clone, Debug)]
pub enum Cohort {
    /// Lanes with a `pr.merged` whose `merged_at` is at or after this:
    /// `YYYY-MM-DD` (00:00 UTC) or RFC 3339 UTC with milliseconds.
    MergedSince(String),
    /// These work items; each must exist.
    Work(Vec<i64>),
}

/// R11: a cohort under 25 lanes is flagged, not scored.
pub const TOO_FEW_LANES: usize = 25;
/// R11's `lanes_peak_over_500k`.
const PEAK_OVER: i64 = 500_000;
const TOKENS: [&str; 4] = ["input_total", "cache_read", "cache_write", "output_total"];
/// What a review number needs when the cohort has no recorded review.
const NO_REVIEW: &str = "check.finished local_review or pr_review";
/// What a token number needs when no lane in the cohort has usage.
const NO_USAGE: &str = "usage.reported";
/// Commits after the handoff are a git count (events.md, PR), never a
/// ledger read.
const NO_COMMITS: &str =
    "git rev-list --count <first real handoff commit>..<pr.merged head_sha>, not in the ledger";

#[derive(Default)]
struct Review {
    local: bool,
    /// Daisy's job id: reviews are ordered by it, never by when recorded.
    job: Option<i64>,
    event: i64,
    at: String,
    score: Option<f64>,
    head: Option<String>,
    /// Each finding's `finding_key`, title and whether it was verified.
    findings: Vec<(String, String, bool)>,
}

#[derive(Default)]
struct Lane {
    id: i64,
    title: String,
    kind: Option<String>,
    pipeline_id: Option<String>,
    /// The first claim: its `at` and its julian day.
    claimed: Option<(String, f64)>,
    /// Every PR the lane held, lowercased, and the event that set the first.
    prs: BTreeSet<String>,
    first_pr_event: Option<i64>,
    /// PR (lowercased) to its merged head, `merged_at` and julian day.
    merged: BTreeMap<String, (String, String, f64)>,
    /// Per source, the null-skipping sums of `TOKENS`.
    tokens: BTreeMap<String, [Option<i64>; 4]>,
    peak: Option<i64>,
    attention: BTreeMap<String, i64>,
    rounds: Option<i64>,
    plan_check: Value,
    quality_gate: Value,
    /// Verdicts that ran, in log order: head and whether it said yes.
    verdicts: Vec<(String, bool)>,
    /// Handoffs in log order: `at` and whether it is a real one (its
    /// `next_action` does not start with `reviewing`).
    handoffs: Vec<(String, bool)>,
    reviews: Vec<Review>,
}

fn same_commit(a: &str, b: &str) -> bool {
    sha_prefix_eq(a, b, 7..=40, true)
}

fn add(sum: &mut Option<i64>, n: Option<i64>) {
    if let Some(n) = n {
        *sum = Some(sum.unwrap_or(0) + n);
    }
}

fn median(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    let m = if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    };
    json!((m * 100.0).round() / 100.0)
}

fn token_fields(sums: &[Option<i64>; 4]) -> Value {
    let fields: Map<String, Value> = TOKENS
        .iter()
        .zip(sums)
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    fields.into()
}

fn hours(days: f64) -> f64 {
    (days * 24.0 * 100.0).round() / 100.0
}

/// One lane's numbers, before they are printed.
struct Row {
    with_pr: bool,
    merged: bool,
    claim_to_merge_h: Option<f64>,
    context_tokens: Option<i64>,
    verdict_on_head: Option<bool>,
    head_reviewed: Option<bool>,
    flat_rounds: i64,
    reraised: i64,
    verified: i64,
    verified_after: i64,
}

impl Lane {
    fn row(&self) -> Row {
        let with_pr = !self.prs.is_empty();
        let merged = with_pr && self.prs.iter().all(|pr| self.merged.contains_key(pr));
        let last_merge = self.merged.values().map(|m| m.2).reduce(f64::max);
        let claim_to_merge_h = match (merged, &self.claimed, last_merge) {
            (true, Some((_, claimed)), Some(merge)) => Some(hours(merge - claimed)),
            _ => None,
        };
        let heads: Vec<&str> = self.merged.values().map(|m| m.0.as_str()).collect();
        // Every merged head has a YES as the latest verdict on it. A lane
        // with a PR still open (a stack layer) has a head nobody can check.
        let verdict_on_head = merged.then(|| {
            heads.iter().all(|head| {
                self.verdicts
                    .iter()
                    .rev()
                    .find(|(sha, _)| same_commit(sha, head))
                    .is_some_and(|(_, yes)| *yes)
            })
        });
        let head_reviewed = merged.then(|| {
            heads.iter().all(|head| {
                self.reviews
                    .iter()
                    .any(|r| r.local && r.head.as_deref().is_some_and(|h| same_commit(h, head)))
            })
        });
        let mut after_pr: Vec<&Review> = self
            .reviews
            .iter()
            .filter(|r| r.local && self.first_pr_event.is_some_and(|pr| r.event > pr))
            .collect();
        after_pr.sort_by_key(|r| (r.job, r.event));
        let scores: Vec<f64> = after_pr.iter().filter_map(|r| r.score).collect();
        let flat_rounds = scores.windows(2).filter(|w| w[1] <= w[0]).count() as i64;
        // Verified titles raised again by another review.
        let mut by_title: BTreeMap<String, BTreeSet<Option<i64>>> = BTreeMap::new();
        for r in &self.reviews {
            for (_, title, _) in r.findings.iter().filter(|f| f.2) {
                by_title
                    .entry(title.trim().to_lowercase())
                    .or_default()
                    .insert(r.job);
            }
        }
        let reraised = by_title.values().map(|s| s.len() as i64 - 1).sum();
        // R11: a review is before the handoff when it was recorded before the
        // first real one (the earliest handoff when none is real).
        let handoff = self
            .handoffs
            .iter()
            .find(|h| h.1)
            .or(self.handoffs.first())
            .map(|h| h.0.as_str());
        // R11 counts a finding once, as its latest record (in job order)
        // says, with its phase from that record's review.
        let mut ordered: Vec<&Review> = self.reviews.iter().collect();
        ordered.sort_by_key(|r| (r.job, r.event));
        let mut latest: BTreeMap<&str, (&Review, bool)> = BTreeMap::new();
        for r in ordered {
            for (key, _, verified) in &r.findings {
                latest.insert(key, (r, *verified));
            }
        }
        let verified = latest.values().filter(|f| f.1).count() as i64;
        let verified_after = latest
            .values()
            .filter(|(r, v)| *v && handoff.is_none_or(|h| r.at.as_str() > h))
            .count() as i64;
        Row {
            with_pr,
            merged,
            claim_to_merge_h,
            context_tokens: self.tokens.get("parent").and_then(|t| t[0]),
            verdict_on_head,
            head_reviewed,
            flat_rounds,
            reraised,
            verified,
            verified_after,
        }
    }

    fn json(&self, row: &Row, reviews_known: bool) -> Value {
        let tokens = (!self.tokens.is_empty()).then(|| {
            let out: Map<String, Value> = self
                .tokens
                .iter()
                .map(|(source, sums)| (source.clone(), token_fields(sums)))
                .collect();
            out
        });
        let known = |v: Value| if reviews_known { v } else { Value::Null };
        json!({
            "id": self.id,
            "title": self.title,
            "kind": self.kind,
            "pipeline_id": self.pipeline_id,
            "claimed_at": self.claimed.as_ref().map(|c| &c.0),
            "merged_at": self.merged.values().map(|m| &m.1).max(),
            "claim_to_merge_h": row.claim_to_merge_h,
            "with_pr": row.with_pr,
            "merged": row.merged,
            "tokens": tokens,
            "peak_context": self.peak,
            "attention": self.attention,
            "send_back_rounds": self.rounds,
            "plan_check": self.plan_check,
            "quality_gate": self.quality_gate,
            "merge_verdict": self.verdicts.last().map(|(head, yes)| {
                json!({ "decision": if *yes { "yes" } else { "no" }, "head_sha": head })
            }),
            "verdict_on_head": row.verdict_on_head,
            "head_reviewed": known(json!(row.head_reviewed)),
            "flat_rounds": known(json!(row.flat_rounds)),
            "reraised_findings": known(json!(row.reraised)),
            "verified_findings": known(json!(row.verified)),
            "verified_findings_after_handoff": known(json!(row.verified_after)),
            "commits_after_handoff": null,
        })
    }

    fn fold(&mut self, e: &Ev) {
        let p = &e.payload;
        let text = |k: &str| p.get(k).and_then(Value::as_str);
        let pr = match e.kind.as_str() {
            "pr.opened" => text("pr"),
            _ => p.pointer("/changed/pr_ref/to").and_then(Value::as_str),
        };
        if let Some(pr) = pr {
            self.prs.insert(pr.to_lowercase());
            self.first_pr_event.get_or_insert(e.id);
        }
        if e.handoff {
            let next = p.pointer("/handoff/next_action").and_then(Value::as_str);
            let real = !next.is_some_and(|n| n.trim_start().starts_with("reviewing"));
            self.handoffs.push((e.at.clone(), real));
        }
        match e.kind.as_str() {
            "lane.claimed" if self.claimed.is_none() => {
                self.claimed = Some((e.at.clone(), e.day));
            }
            "usage.reported" => {
                let source = e.source.clone().unwrap_or_default();
                let sums = self.tokens.entry(source.clone()).or_default();
                for (sum, n) in sums.iter_mut().zip(e.tokens) {
                    add(sum, n);
                }
                if source == "parent" && e.tokens[0].is_some() {
                    self.peak = self.peak.max(e.tokens[0]);
                }
            }
            "attention.recorded" => {
                if let Some(kind) = text("type") {
                    *self.attention.entry(kind.into()).or_default() += 1;
                }
                if let Some(round) = p.get("round").and_then(Value::as_i64) {
                    self.rounds = self.rounds.max(Some(round));
                }
            }
            "pr.merged" => {
                if let (Some(pr), Some(head), Some(at)) =
                    (text("pr"), text("head_sha"), text("merged_at"))
                {
                    self.merged
                        .insert(pr.to_lowercase(), (head.into(), at.into(), e.day));
                }
            }
            "check.finished" => self.check(e),
            _ => {}
        }
    }

    fn check(&mut self, e: &Ev) {
        let p = &e.payload;
        let text = |k: &str| p.get(k).and_then(Value::as_str);
        let ran = text("outcome") == Some("ran");
        match text("check") {
            Some(check @ ("plan_check" | "quality_gate")) => {
                let mut v = json!({ "outcome": text("outcome"), "head_sha": text("head_sha") });
                if ran {
                    v["findings"] = json!(p.get("findings"));
                } else {
                    v["reason"] = json!(text("reason"));
                }
                if check == "plan_check" {
                    self.plan_check = v;
                } else {
                    self.quality_gate = v;
                }
            }
            Some("merge_verdict") if ran => {
                if let Some(head) = text("head_sha") {
                    self.verdicts
                        .push((head.into(), text("decision") == Some("yes")));
                }
            }
            Some(check @ ("local_review" | "pr_review")) if ran => {
                let findings = p.get("findings").and_then(Value::as_array);
                let field = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).map(String::from);
                let findings = findings
                    .into_iter()
                    .flatten()
                    .filter_map(|f| {
                        let verified = f.get("verified").and_then(Value::as_bool) == Some(true);
                        Some((field(f, "finding_key")?, field(f, "title")?, verified))
                    })
                    .collect();
                self.reviews.push(Review {
                    local: check == "local_review",
                    job: p.get("job").and_then(Value::as_i64),
                    event: e.id,
                    at: e.at.clone(),
                    score: p.get("score").and_then(Value::as_f64),
                    head: text("head_sha").map(String::from),
                    findings,
                });
            }
            _ => {}
        }
    }
}

/// One event row as `stats` reads it.
struct Ev {
    id: i64,
    kind: String,
    at: String,
    /// The julian day of `at`, or of `merged_at` on a `pr.merged`.
    day: f64,
    /// Parsed for every kind but `usage.reported`, which reads `tokens`.
    payload: Value,
    handoff: bool,
    source: Option<String>,
    tokens: [Option<i64>; 4],
}

impl Ledger {
    /// R11's per-lane numbers and the cohort summary over `cohort`, from
    /// one read of the cohort's events. A number whose event nothing
    /// records yet is null, and `missing` names what it needs.
    pub fn stats(&self, cohort: &Cohort) -> Checked<Value> {
        let conn = self.conn();
        let (filter, arg) = match cohort {
            Cohort::MergedSince(since) => {
                let at = if since.len() == 10 {
                    format!("{since}T00:00:00.000Z")
                } else {
                    since.clone()
                };
                if !canonical_at(conn, &at)? {
                    return Ok(Err(Refusal::InvalidInput(format!(
                        "--merged-since {since:?} is not YYYY-MM-DD or RFC 3339 UTC with \
                         milliseconds (2026-11-07T00:00:00.000Z)"
                    ))));
                }
                (
                    "SELECT lane_id FROM events WHERE kind = 'pr.merged'
                     AND json_extract(payload_json, '$.merged_at') >= ?1",
                    at,
                )
            }
            Cohort::Work(ids) => ("SELECT value FROM json_each(?1)", json!(ids).to_string()),
        };
        let tok = |k: &str| format!("json_extract(payload_json, '$.{k}')");
        let sql = format!(
            "SELECT l.id, l.title, l.kind, l.pipeline_id, e.id, e.kind, e.at, e.day,
                    e.payload, e.handoff, e.source, e.t0, e.t1, e.t2, e.t3
             FROM lanes l JOIN (
                 SELECT lane_id, id, kind, at,
                        julianday(CASE kind WHEN 'pr.merged'
                                  THEN json_extract(payload_json, '$.merged_at') ELSE at END) AS day,
                        CASE kind WHEN 'usage.reported' THEN NULL ELSE payload_json END AS payload,
                        {IS_HANDOFF} AS handoff,
                        {} AS source, {} AS t0, {} AS t1, {} AS t2, {} AS t3
                 FROM events
             ) e ON e.lane_id = l.id
             WHERE l.id IN ({filter})
             ORDER BY l.id, e.id",
            tok("source"),
            tok(TOKENS[0]),
            tok(TOKENS[1]),
            tok(TOKENS[2]),
            tok(TOKENS[3]),
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([arg])?;
        let mut lanes: Vec<Lane> = Vec::new();
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            if lanes.last().is_none_or(|l| l.id != id) {
                lanes.push(Lane {
                    id,
                    title: r.get(1)?,
                    kind: r.get(2)?,
                    pipeline_id: r.get(3)?,
                    ..Lane::default()
                });
            }
            let kind: String = r.get(5)?;
            let usage = kind == "usage.reported";
            let payload: Option<String> = r.get(8)?;
            let ev = Ev {
                id: r.get(4)?,
                kind,
                at: r.get(6)?,
                day: r.get(7)?,
                payload: payload
                    .map(|p| serde_json::from_str(&p).map_err(std::io::Error::other))
                    .transpose()?
                    .unwrap_or(Value::Null),
                handoff: r.get(9)?,
                source: if usage { r.get(10)? } else { None },
                tokens: if usage {
                    [r.get(11)?, r.get(12)?, r.get(13)?, r.get(14)?]
                } else {
                    [None; 4]
                },
            };
            if let Some(lane) = lanes.last_mut() {
                lane.fold(&ev);
            }
        }
        if let Cohort::Work(ids) = cohort
            && let Some(&id) = ids.iter().find(|id| !lanes.iter().any(|l| l.id == **id))
        {
            return Ok(Err(Refusal::NotFound {
                entity: "work item",
                id,
            }));
        }
        Ok(Ok(summarize(&lanes)))
    }
}

/// What `chaperone-ui` draws beside the board: `stats` for every open
/// lane and for the cohort merged since `since`.
#[derive(Clone, Debug)]
pub struct BoardStats {
    /// The ledger's `data_version` these were read at.
    pub data_version: i64,
    /// The cohort's date as asked, or the default: 7 days ago (UTC).
    pub since: String,
    /// `stats --work <open lanes>`'s `lanes`.
    pub lanes: Vec<Value>,
    /// `stats --merged-since <since>`, or why it was refused.
    pub cohort: Result<Value, Refusal>,
}

impl Ledger {
    /// [`BoardStats`] as of one snapshot, its schema re-checked inside it,
    /// as `lane_board` does.
    pub fn board_stats(&self, since: Option<&str>) -> Result<BoardStats, LedgerError> {
        let tx = Transaction::new_unchecked(self.conn(), TransactionBehavior::Deferred)?;
        check_schema(&tx)?;
        let data_version = self.data_version()?;
        let since = match since {
            Some(s) => s.to_string(),
            None => tx.query_row("SELECT date('now', '-7 days')", [], |r| r.get(0))?,
        };
        let mut stmt =
            tx.prepare("SELECT id FROM lanes WHERE status NOT IN ('done', 'cancelled')")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        // Every lane has its lane.created, so no open lane is refused.
        let lanes = match self.stats(&Cohort::Work(ids))? {
            Ok(mut v) => {
                let Value::Array(lanes) = v["lanes"].take() else {
                    unreachable!("summarize prints lanes as an array")
                };
                lanes
            }
            Err(r) => return Err(std::io::Error::other(format!("stats: {r:?}")).into()),
        };
        let cohort = self.stats(&Cohort::MergedSince(since.clone()))?;
        Ok(BoardStats {
            data_version,
            since,
            lanes,
            cohort,
        })
    }
}

fn summarize(lanes: &[Lane]) -> Value {
    let rows: Vec<Row> = lanes.iter().map(Lane::row).collect();
    let reviews_known = lanes.iter().any(|l| !l.reviews.is_empty());
    let usage_known = rows.iter().any(|r| r.context_tokens.is_some());
    let known = |v: Value| if reviews_known { v } else { Value::Null };
    let count =
        |f: &dyn Fn(&Lane, &Row) -> bool| lanes.iter().zip(&rows).filter(|(l, r)| f(l, r)).count();
    let sum = |f: &dyn Fn(&Row) -> i64| rows.iter().map(f).sum::<i64>();
    let outcome = |v: &Value| v.get("outcome").and_then(Value::as_str).map(String::from);
    let skipped = |pick: &dyn Fn(&Lane) -> &Value| {
        let mut by: BTreeMap<String, i64> = BTreeMap::new();
        for l in lanes {
            let v = pick(l);
            if outcome(v).as_deref() == Some("skipped") {
                let reason = v["reason"].as_str().unwrap_or_default();
                *by.entry(reason.into()).or_default() += 1;
            }
        }
        by
    };
    let mut tokens: BTreeMap<&str, [Option<i64>; 4]> = BTreeMap::new();
    let mut attention: BTreeMap<&str, i64> = BTreeMap::new();
    for l in lanes {
        for (source, sums) in &l.tokens {
            let total = tokens.entry(source).or_default();
            for (t, n) in total.iter_mut().zip(sums) {
                add(t, *n);
            }
        }
        for (kind, n) in &l.attention {
            *attention.entry(kind).or_default() += n;
        }
    }
    for kind in ATTENTION_TYPES {
        attention.entry(kind).or_default();
    }
    let tokens: Map<String, Value> = tokens
        .into_iter()
        .map(|(source, sums)| (source.to_string(), token_fields(&sums)))
        .collect();
    let per_pr: Vec<f64> = lanes
        .iter()
        .zip(&rows)
        .filter(|(_, r)| r.merged)
        .filter_map(|(l, r)| r.context_tokens.map(|c| c as f64 / l.prs.len() as f64))
        .collect();
    let mut missing = Map::new();
    missing.insert("lanes_with_commits_after_handoff".into(), NO_COMMITS.into());
    if !usage_known {
        for field in [
            "peak_context_median",
            "context_tokens_per_merged_pr_median",
            "lanes_peak_over_500k",
        ] {
            missing.insert(field.into(), NO_USAGE.into());
        }
    }
    if !reviews_known {
        for field in [
            "lanes_head_reviewed",
            "flat_rounds",
            "reraised_findings",
            "verified_findings",
            "verified_findings_after_handoff",
        ] {
            missing.insert(field.into(), NO_REVIEW.into());
        }
    }
    let cohort = json!({
        "lanes": lanes.len(),
        "with_pr": count(&|_, r| r.with_pr),
        "merged": count(&|_, r| r.merged),
        "too_few_lanes": lanes.len() < TOO_FEW_LANES,
        "claim_to_merge_h_median": median(rows.iter().filter_map(|r| r.claim_to_merge_h).collect()),
        "tokens": tokens,
        "attention": attention,
        "lanes_with_usage": count(&|_, r| r.context_tokens.is_some()),
        "peak_context_median": median(lanes.iter().filter_map(|l| l.peak.map(|p| p as f64)).collect()),
        "context_tokens_per_merged_pr_median": median(per_pr),
        "lanes_peak_over_500k": usage_known
            .then(|| count(&|l, _| l.peak.is_some_and(|p| p > PEAK_OVER))),
        "lanes_reviewed": count(&|l, _| !l.reviews.is_empty()),
        "flat_rounds": known(json!(sum(&|r| r.flat_rounds))),
        "reraised_findings": known(json!(sum(&|r| r.reraised))),
        "lanes_head_reviewed": known(json!(count(&|_, r| r.with_pr && r.head_reviewed == Some(true)))),
        "lanes_with_commits_after_handoff": null,
        "verified_findings": known(json!(sum(&|r| r.verified))),
        "verified_findings_after_handoff": known(json!(sum(&|r| r.verified_after))),
        "lanes_plan_checked": count(&|l, _| outcome(&l.plan_check).as_deref() == Some("ran")),
        "plan_check_skipped": skipped(&|l| &l.plan_check),
        "lanes_quality_gated": count(&|l, r| r.with_pr && outcome(&l.quality_gate).as_deref() == Some("ran")),
        "quality_gate_skipped": skipped(&|l| &l.quality_gate),
        "lanes_verdict_on_head": count(&|_, r| r.with_pr && r.verdict_on_head == Some(true)),
        "missing": missing,
    });
    let lanes: Vec<Value> = lanes
        .iter()
        .zip(&rows)
        .map(|(l, r)| l.json(r, reviews_known))
        .collect();
    json!({ "cohort": cohort, "lanes": lanes })
}
