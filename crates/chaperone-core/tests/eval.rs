//! The predicate evaluator. These live outside `eval.rs` so the asserts'
//! own branches do not count against its branch coverage.

use chaperone_core::pipeline::{Denied, Fact, FactBundle, Phase, Pipeline, Route};

const V17: &str = include_str!("../../../pipelines/v17.toml");
const V19: &str = include_str!("../../../pipelines/v19.toml");

/// A document whose `handoff` (code_pr, to done) requires gate `a`.
fn doc(gates: &str) -> Pipeline {
    Pipeline::load(&format!(
        r#"
schema = 1
id = "t"

[limits]
cap = 3
lo = 7
hi = 40

{gates}

[[transition]]
id = "handoff"
routes = ["code_pr"]
to = "done"
requires = ["a"]
"#
    ))
    .unwrap()
}

fn facts(pairs: &[(&str, Fact)]) -> FactBundle {
    let mut f = FactBundle::default();
    for (path, fact) in pairs {
        f.set(path, fact.clone());
    }
    f
}

fn run(p: &Pipeline, f: &[(&str, Fact)]) -> Result<(), Denied> {
    p.evaluate("handoff", Route::CodePr, Phase::Done, &facts(f))
}

fn denied(gate: &str, paths: &[&str]) -> Result<(), Denied> {
    Err(Denied {
        gate: gate.into(),
        fact_paths: paths.iter().map(|p| p.to_string()).collect(),
    })
}

fn s(v: &str) -> Fact {
    Fact::Str(v.into())
}

fn t(v: &str) -> Fact {
    Fact::Time(v.into())
}

fn gate(func: &str, args: &str) -> Pipeline {
    doc(&format!(
        "[[gate]]\nid = \"a\"\nfn = \"{func}\"\nargs = {args}"
    ))
}

#[test]
fn present() {
    let p = gate("present", r#"{ field = "x" }"#);
    assert_eq!(run(&p, &[("x", Fact::Bool(false))]), Ok(()));
    assert_eq!(run(&p, &[]), denied("a", &["x"]));
}

#[test]
fn eq_literal_and_limit() {
    let p = gate("eq", r#"{ field = "x", value = 1 }"#);
    assert_eq!(run(&p, &[("x", Fact::Int(1))]), Ok(()));
    assert_eq!(run(&p, &[("x", Fact::Int(2))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[("x", s("1"))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[]), denied("a", &["x"]));
    let p = gate("eq", r#"{ field = "x", limit = "cap" }"#);
    assert_eq!(run(&p, &[("x", Fact::Int(3))]), Ok(()));
    let p = gate("eq", r#"{ field = "x", value = true }"#);
    assert_eq!(run(&p, &[("x", Fact::Bool(true))]), Ok(()));
    assert_eq!(run(&p, &[("x", Fact::Bool(false))]), denied("a", &["x"]));
}

#[test]
fn eq_value_from() {
    let p = gate("eq", r#"{ field = "x", value_from = "y" }"#);
    assert_eq!(run(&p, &[("x", s("abc")), ("y", s("abc"))]), Ok(()));
    assert_eq!(
        run(&p, &[("x", s("abc")), ("y", s("abd"))]),
        denied("a", &["x", "y"])
    );
    assert_eq!(run(&p, &[("x", s("abc"))]), denied("a", &["x", "y"]));
    // Two absent facts are not equal.
    assert_eq!(run(&p, &[]), denied("a", &["x", "y"]));
}

#[test]
fn lte_and_gt() {
    let p = gate("lte", r#"{ field = "x", limit = "cap" }"#);
    assert_eq!(run(&p, &[("x", Fact::Int(3))]), Ok(()));
    assert_eq!(run(&p, &[("x", Fact::Int(4))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[("x", s("3"))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[]), denied("a", &["x"]));
    let p = gate("gt", r#"{ field = "x", value = 3 }"#);
    assert_eq!(run(&p, &[("x", Fact::Int(4))]), Ok(()));
    assert_eq!(run(&p, &[("x", Fact::Int(3))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[]), denied("a", &["x"]));
}

#[test]
fn one_of() {
    let p = gate("one_of", r#"{ field = "x", values = ["green", "none"] }"#);
    assert_eq!(run(&p, &[("x", s("none"))]), Ok(()));
    assert_eq!(run(&p, &[("x", s("failing"))]), denied("a", &["x"]));
    assert_eq!(run(&p, &[]), denied("a", &["x"]));
}

#[test]
fn sha_prefix_eq() {
    let p = gate(
        "sha_prefix_eq",
        r#"{ left = "v", right = "h", min_limit = "lo", max_limit = "hi" }"#,
    );
    let head = "abc1234def567890abc1234def567890abc12345";
    let check = |v: Fact| run(&p, &[("v", v), ("h", s(head))]);
    assert_eq!(check(s("abc1234")), Ok(()));
    assert_eq!(check(s(head)), Ok(()));
    let no = denied("a", &["v", "h"]);
    assert_eq!(check(s("abc123")), no, "under the floor");
    assert_eq!(check(s(&format!("{head}0"))), no, "over the ceiling");
    assert_eq!(check(s("abc123x")), no, "not hex");
    assert_eq!(check(s("fff9999a")), no, "not a prefix");
    assert_eq!(check(Fact::Int(1234567)), no, "not a string");
    assert_eq!(check(Fact::Absent), no);
    assert_eq!(run(&p, &[("v", s("abc1234"))]), no);
}

#[test]
fn sha_same_commit() {
    let args = r#"{ left = "v", right = "h", min_limit = "lo", max_limit = "hi" }"#;
    let p = gate("sha_same_commit", args);
    let full = "abc1234def567890abc1234def567890abc12345";
    let check = |v: &str, h: &str| run(&p, &[("v", s(v)), ("h", s(h))]);
    assert_eq!(check("abc1234", full), Ok(()));
    assert_eq!(check(full, "abc1234"), Ok(()), "short commit, full verdict");
    let no = denied("a", &["v", "h"]);
    assert_eq!(check(full, "abc1235"), no, "a mismatch");
    assert_eq!(check(full, "abc123"), no, "under the floor");
    // v17's one-way fn still refuses a full verdict on a short commit.
    let p = gate("sha_prefix_eq", args);
    assert_eq!(run(&p, &[("v", s(full)), ("h", s("abc1234"))]), no);
}

#[test]
fn matches_lane() {
    for (func, check) in [
        ("plan_check_matches_lane", "plan_check"),
        ("quality_gate_matches_lane", "quality_gate"),
    ] {
        let p = gate(func, r#"{ reasons = ["docs", "tests"] }"#);
        let outcome = format!("{check}.outcome");
        let reason = format!("{check}.reason");
        let paths = [outcome.as_str(), "lane.changes_code", reason.as_str()];
        let lane = |o: Option<Fact>, code: Option<bool>, r: Option<Fact>| {
            let mut f = FactBundle::default();
            if let Some(o) = o {
                f.set(&outcome, o);
            }
            if let Some(code) = code {
                f.set("lane.changes_code", code);
            }
            if let Some(r) = r {
                f.set(&reason, r);
            }
            p.evaluate("handoff", Route::CodePr, Phase::Done, &f)
        };
        let no = denied("a", &paths);
        assert_eq!(lane(Some(s("ran")), Some(true), None), Ok(()));
        assert_eq!(lane(Some(s("ran")), Some(false), None), Ok(()), "may run");
        assert_eq!(
            lane(Some(s("skipped")), Some(false), Some(s("tests"))),
            Ok(())
        );
        assert_eq!(lane(Some(s("skipped")), Some(true), Some(s("tests"))), no);
        assert_eq!(lane(Some(s("skipped")), Some(false), Some(s("busy"))), no);
        assert_eq!(
            lane(Some(s("skipped")), Some(false), Some(Fact::Int(1))),
            no
        );
        assert_eq!(lane(Some(s("maybe")), Some(false), Some(s("tests"))), no);
        assert_eq!(lane(Some(Fact::Bool(true)), Some(true), None), no);
        assert_eq!(lane(None, Some(true), None), no);
        assert_eq!(lane(Some(s("skipped")), None, Some(s("tests"))), no);
        assert_eq!(lane(Some(s("skipped")), Some(false), None), no);
    }
}

#[test]
fn event_before() {
    let p = gate(
        "event_before",
        r#"{ earlier = "quality_gate", later = "pr.opened" }"#,
    );
    let paths = denied("a", &["quality_gate.at", "pr.opened_at"]);
    let qg = ("quality_gate.at", t("2026-11-03T05:35:23.321Z"));
    let pr = |at: &str| ("pr.opened_at", t(at));
    assert_eq!(
        run(&p, std::slice::from_ref(&qg)),
        Ok(()),
        "vacuous with no PR"
    );
    assert_eq!(run(&p, &[]), Ok(()), "vacuous with no PR");
    assert_eq!(
        run(&p, &[qg.clone(), pr("2026-11-03T05:35:23.322Z")]),
        Ok(())
    );
    assert_eq!(
        run(&p, &[qg.clone(), pr("2026-11-03T05:35:23.321Z")]),
        paths
    );
    assert_eq!(
        run(&p, &[qg.clone(), pr("2026-11-03T05:35:23.320Z")]),
        paths
    );
    assert_eq!(run(&p, &[pr("2026-11-03T05:35:23.322Z")]), paths);
    assert_eq!(
        run(&p, &[qg, ("pr.opened_at", s("2026-11-04T00:00:00.000Z"))]),
        paths,
        "a PR time that is not a time fails, never vacuous"
    );
}

const LEAVES: &str = r#"
[[gate]]
id = "x1"
fn = "eq"
args = { field = "x", value = 1 }

[[gate]]
id = "y1"
fn = "eq"
args = { field = "y", value = 1 }

[[gate]]
id = "x_small"
fn = "lte"
args = { field = "x", value = 1 }
"#;

fn combinator(func: &str, of: &str) -> Pipeline {
    doc(&format!(
        "[[gate]]\nid = \"a\"\nfn = \"{func}\"\nargs = {{ of = {of} }}\n{LEAVES}"
    ))
}

fn xy(x: Option<i64>, y: Option<i64>) -> Vec<(&'static str, Fact)> {
    let mut f = Vec::new();
    if let Some(x) = x {
        f.push(("x", Fact::Int(x)));
    }
    if let Some(y) = y {
        f.push(("y", Fact::Int(y)));
    }
    f
}

#[test]
fn all_names_its_first_failing_member() {
    let p = combinator("all", r#"["x1", "y1"]"#);
    assert_eq!(run(&p, &xy(Some(1), Some(1))), Ok(()));
    assert_eq!(run(&p, &xy(Some(1), Some(2))), denied("y1", &["y"]));
    assert_eq!(run(&p, &xy(Some(2), None)), denied("x1", &["x"]));
    assert_eq!(run(&p, &xy(None, Some(1))), denied("x1", &["x"]));
}

#[test]
fn any_holds_on_one_member() {
    let p = combinator("any", r#"["x1", "y1"]"#);
    assert_eq!(run(&p, &xy(Some(2), Some(1))), Ok(()));
    assert_eq!(run(&p, &xy(None, Some(1))), Ok(()));
    assert_eq!(run(&p, &xy(Some(2), Some(2))), denied("a", &["x", "y"]));
    assert_eq!(run(&p, &xy(Some(2), None)), denied("a", &["x", "y"]));
    let p = combinator("any", r#"["x1", "x_small"]"#);
    assert_eq!(run(&p, &xy(Some(2), None)), denied("a", &["x"]));
}

#[test]
fn not_over_an_absent_fact_denies() {
    let p = combinator("not", r#""x1""#);
    assert_eq!(run(&p, &xy(Some(2), None)), Ok(()));
    assert_eq!(run(&p, &xy(Some(1), None)), denied("a", &["x"]));
    assert_eq!(run(&p, &xy(None, None)), denied("a", &["x"]));

    // not(any(absent, false)) is absent too, so it still denies.
    let p = doc(&format!(
        "[[gate]]\nid = \"a\"\nfn = \"not\"\nargs = {{ of = \"either\" }}\n\n\
         [[gate]]\nid = \"either\"\nfn = \"any\"\nargs = {{ of = [\"x1\", \"y1\"] }}\n{LEAVES}"
    ));
    assert_eq!(run(&p, &xy(Some(2), None)), denied("a", &["x", "y"]));
    assert_eq!(run(&p, &xy(Some(2), Some(2))), Ok(()));
    // not(all(..)) with a missing member denies in either order.
    let p = doc(&format!(
        "[[gate]]\nid = \"a\"\nfn = \"not\"\nargs = {{ of = \"both\" }}\n\n\
         [[gate]]\nid = \"both\"\nfn = \"all\"\nargs = {{ of = [\"x1\", \"y1\"] }}\n{LEAVES}"
    ));
    assert_eq!(run(&p, &xy(Some(2), None)), denied("a", &["x", "y"]));
    assert_eq!(run(&p, &xy(None, Some(2))), denied("a", &["x", "y"]));
    assert_eq!(run(&p, &xy(Some(2), Some(2))), Ok(()));
}

#[test]
fn loaded_hash_comes_from_the_document_not_the_facts() {
    let p = gate(
        "eq",
        r#"{ field = "pipeline.hash", value_from = "loaded.hash" }"#,
    );
    let paths = denied("a", &["pipeline.hash", "loaded.hash"]);
    assert_eq!(run(&p, &[("pipeline.hash", s(&p.hash))]), Ok(()));
    assert_eq!(run(&p, &[("pipeline.hash", s("forged"))]), paths);
    assert_eq!(
        run(
            &p,
            &[("pipeline.hash", s("forged")), ("loaded.hash", s("forged"))]
        ),
        paths
    );
}

#[test]
fn no_row_is_denied_as_unstamped() {
    let p = gate("present", r#"{ field = "x" }"#);
    let f = facts(&[("x", Fact::Int(1))]);
    let no = denied("stamp_ok", &["lane.pipeline_hash"]);
    assert_eq!(
        p.evaluate("handoff", Route::CodePr, Phase::Done, &f),
        Ok(())
    );
    assert_eq!(p.evaluate("handof", Route::CodePr, Phase::Done, &f), no);
    assert_eq!(p.evaluate("handoff", Route::Direct, Phase::Done, &f), no);
    assert_eq!(p.evaluate("handoff", Route::CodePr, Phase::Ready, &f), no);
}

/// Every `ready_evidence` fact set so it holds, with `verdict.sha` as given
/// (R7 "One gate the engine refuses").
fn ready_lane(p: &Pipeline, verdict_sha: &str) -> FactBundle {
    let head = "abc1234def567890";
    let mut f = FactBundle::default();
    f.set("pipeline.id", "v17")
        .set("pipeline.hash", p.hash.as_str())
        .set("lane.changes_code", true)
        .set("plan_check.outcome", "ran")
        .set("quality_gate.outcome", "ran")
        .set("quality_gate.at", t("2026-11-03T05:00:00.000Z"))
        .set("pr.opened_at", t("2026-11-03T06:00:00.000Z"))
        .set("verdict.decision", "yes")
        .set("verdict.sha", verdict_sha)
        .set("commit.sha", head)
        .set("review.local_head", head)
        .set("review.local_rounds", 1)
        .set("smoke.recorded", true)
        .set("kept_tests.recorded", true)
        .set("kept_tests.bar_ok", true)
        .set("verification.unexplained_failures", 0)
        .set("brief.oversize", false)
        .set("ci.state", "green")
        .set("threads.human_open", 0)
        .set("daisy_review.state", "finished")
        .set("pr.is_draft", false)
        .set("pr.conflicting", false)
        .set("diff.code_lines", 200);
    f
}

#[test]
fn r7_to_ready_is_denied_by_verdict_on_head_and_its_mirror_passes() {
    let p = Pipeline::load(V17).unwrap();
    let to_ready = |f: &FactBundle| p.evaluate("to_ready", Route::CodePr, Phase::Ready, f);
    assert_eq!(
        to_ready(&ready_lane(&p, "fff9999a")),
        denied("verdict_on_head", &["verdict.sha", "commit.sha"])
    );
    assert_eq!(to_ready(&ready_lane(&p, "abc1234")), Ok(()));

    // No verdict fails earlier, at verdict_is_yes.
    let mut f = ready_lane(&p, "abc1234");
    f.set("verdict.decision", Fact::Absent);
    assert_eq!(
        to_ready(&f),
        denied("verdict_is_yes", &["verdict.decision"])
    );
}

#[test]
fn v17_reviewed_head_ok() {
    let p = Pipeline::load(V17).unwrap();
    let to_ready = |f: &FactBundle| p.evaluate("to_ready", Route::CodePr, Phase::Ready, f);
    let mut f = ready_lane(&p, "abc1234");
    f.set("review.local_head", "0123456");
    assert_eq!(
        to_ready(&f),
        denied(
            "reviewed_head_ok",
            &[
                "review.local_head",
                "commit.sha",
                "review.code_changed_since"
            ]
        )
    );
    f.set("review.code_changed_since", false);
    assert_eq!(to_ready(&f), Ok(()));
}

#[test]
fn v17_investigation_handoff_ready_is_denied() {
    let p = Pipeline::load(V17).unwrap();
    let f = ready_lane(&p, "abc1234");
    assert_eq!(
        p.evaluate("handoff", Route::Investigation, Phase::Ready, &f),
        denied("stamp_ok", &["lane.pipeline_hash"])
    );
    assert_eq!(
        p.evaluate("handoff", Route::Investigation, Phase::Done, &f),
        Ok(())
    );
}

/// v19: CI is not a gate. With no `ci.state`, v17 denies at `ci_not_red`;
/// v19's verdict and ready rows pass on both PR routes.
#[test]
fn v19_reads_no_ci_state() {
    let v17 = Pipeline::load(V17).unwrap();
    let mut f = ready_lane(&v17, "abc1234");
    f.set("ci.state", Fact::Absent);
    assert_eq!(
        v17.evaluate("to_ready", Route::CodePr, Phase::Ready, &f),
        denied("ci_not_red", &["ci.state"])
    );

    let v19 = Pipeline::load(V19).unwrap();
    let mut f = ready_lane(&v19, "abc1234");
    f.set("pipeline.id", "v19").set("ci.state", Fact::Absent);
    for route in [Route::CodePr, Route::Direct] {
        assert_eq!(
            v19.evaluate("ask_verdict", route, Phase::Verdict, &f),
            Ok(())
        );
        assert_eq!(v19.evaluate("to_ready", route, Phase::Ready, &f), Ok(()));
    }
}
