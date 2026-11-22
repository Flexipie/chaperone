//! `review eval` through the binary, with `chaperone-fake` as `claude` and
//! `codex`. The fake is the workspace's binary beside this one, so these
//! tests need the workspace built (`cargo nextest run --workspace` builds it).

use std::path::Path;
use std::process::Command;

use chaperone_core::review;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

fn git(wt: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(wt)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A Claude session's answer to the prompt that has `when` in it.
fn says(when: &str, answer: Value) -> Value {
    let result = json!({"type": "result", "subtype": "success", "is_error": false,
        "structured_output": answer, "total_cost_usd": 0.01, "session_id": when});
    json!({"when": when, "stdout": result.to_string()})
}

fn risk(path: &str, title: &str) -> Value {
    json!({"findings": [{"severity": "risk", "title": title, "body": "b", "path": path,
        "line": null}]})
}

/// Each entry's head adds its own file to a demo clone; only the intent
/// lens is shown an entry's goal, so its fake answer is keyed on it, and
/// the verifier on the title of the finding it rules on.
#[test]
fn each_outcome_is_scored() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, demo) = (dir.path().join("bin"), dir.path().join("repos/demo"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&demo).unwrap();
    git(&demo, &["init", "-q", "-b", "main"]);
    // The base profile requires the `loss` lens of a change to req.rs.
    let profile = "[review]\ndaisy = false\n[[review.required]]\nname = \"loss\"\n\
                   prompt = \"Can a line be lost?\"\nwhen = [\"req.rs\"]\n";
    std::fs::create_dir(demo.join(".chaperone")).unwrap();
    std::fs::write(demo.join(".chaperone/profile.toml"), profile).unwrap();
    git(&demo, &["add", "."]);
    git(&demo, &["commit", "-qm", "base"]);
    git(&demo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let mut set = String::new();
    let entries = [
        ("hit", "GOAL-HIT", Some("hit.rs")),
        ("miss", "GOAL-MISS", Some("miss.rs")),
        ("alarm", "GOAL-ALARM", None),
        ("refuted", "GOAL-REFUTED", None),
        ("unproven", "GOAL-UNPROVEN", None),
        ("broken", "GOAL-BROKEN", None),
        ("req", "GOAL-REQ", Some("req.rs")),
    ];
    for (name, goal, bug) in entries {
        let base = git(&demo, &["rev-parse", "HEAD"]);
        std::fs::write(demo.join(format!("{name}.rs")), "fn f() {}\n").unwrap();
        git(&demo, &["add", "."]);
        git(&demo, &["commit", "-qm", name]);
        let head = git(&demo, &["rev-parse", "HEAD"]);
        set += &format!(
            "[[entry]]\nname = \"{name}\"\nrepo = \"o/demo\"\nbase = \"{base}\"\n\
             head = \"{head}\"\ngoal = \"{goal}\"\n"
        );
        if let Some(path) = bug {
            set += &format!(
                "bug = {{ paths = [\"{path}\"], words = [\"overflow\"], what = \"w\" }}\n"
            );
        }
    }
    let entry = |name: &str, repo: &str| {
        format!(
            "[[entry]]\nname = \"{name}\"\nrepo = \"{repo}\"\nbase = \"a\"\nhead = \"b\"\ngoal = \"g\"\n"
        )
    };
    set += &(entry("gone", "o/absent") + &entry("nocommit", "o/demo"));
    std::fs::write(dir.path().join("set.toml"), set).unwrap();
    // A finding the verifier rules `verdict` on, by its title.
    let ruled = |name: &str, verdict: &str| {
        let finding = risk(&format!("{name}.rs"), &format!("{name}-title"));
        let key = review::findings("intent", finding.clone()).unwrap()[0]
            .finding_key
            .clone();
        let ruling = json!({"rulings": [{"key": key, "verdict": verdict, "why": "w"}]});
        (finding, says(&format!("{name}-title"), ruling))
    };
    let (refuted, refuted_ruling) = ruled("refuted", "refuted");
    let (unproven, unproven_ruling) = ruled("unproven", "unproven");
    let claude = json!([
        refuted_ruling,
        unproven_ruling,
        says("# Verify the review's findings", json!({"rulings": []})),
        says("GOAL-HIT", risk("hit.rs", "Overflow in add")),
        says("GOAL-ALARM", risk("alarm.rs", "alarm-title")),
        says("GOAL-KNOWN", risk("known.rs", "A stale todo")),
        says("GOAL-REFUTED", refuted),
        says("GOAL-UNPROVEN", unproven),
        says("GOAL-BROKEN", json!({"bogus": 1})),
        says("# Review: loss", risk("req.rs", "Overflow loses a line")),
        says("# Review:", json!({"findings": []})),
    ]);
    let codex = json!([{"when": "# Review:", "write": {"-o": r#"{"findings": []}"#}}]);
    let fake = Path::new(BIN).with_file_name("chaperone-fake");
    assert!(fake.is_file(), "no {}: build the workspace", fake.display());
    for (cli, runs) in [("claude", claude), ("codex", codex)] {
        std::os::unix::fs::symlink(&fake, bin.join(cli)).unwrap();
        let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
        std::fs::write(bin.join(format!("{cli}.json")), script).unwrap();
    }
    let db = dir.path().join("ledger.db");
    let eval = |set: &str, only: &[&str]| {
        let path = std::env::var("PATH").unwrap();
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(["review", "eval", "--repos"])
            .arg(dir.path().join("repos"))
            .arg("--set")
            .arg(dir.path().join(set))
            .args(only)
            .env("PATH", format!("{}:{path}", bin.display()))
            .output()
            .unwrap();
        let ok = out.status.success();
        let v: Value = serde_json::from_slice(if ok { &out.stdout } else { &out.stderr }).unwrap();
        (ok, v[if ok { "data" } else { "error" }].clone())
    };
    let (ok, out) = eval("set.toml", &[]);
    assert!(ok, "{out}");
    let got: Vec<_> = out["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["outcome"].as_str().unwrap()))
        .collect();
    assert_eq!(
        got,
        [
            ("hit", "hit"),
            ("miss", "miss"),
            ("alarm", "false_alarm"),
            ("refuted", "clean"),
            ("unproven", "clean"),
            ("broken", "error"),
            ("req", "hit"),
            ("gone", "skipped"),
            ("nocommit", "skipped"),
        ]
    );
    let totals = json!({"hit": 2, "miss": 1, "false_alarm": 1, "clean": 2, "skipped": 2,
        "error": 1, "convention": 0, "fallbacks": 0});
    assert_eq!((&out["totals"], &out["score"]), (&totals, &json!("4/6")));
    assert_eq!(out["entries"][1]["bug"], "w");
    assert!(
        out["entries"][8]["why"]
            .as_str()
            .unwrap()
            .contains("has no commit a")
    );
    let (_, only) = eval("set.toml", &["--only", "hit"]);
    assert_eq!(only["entries"].as_array().unwrap().len(), 1);
    assert_eq!(only["score"], "1/1");
    let (_, twice) = eval("set.toml", &["--runs", "2"]);
    assert_eq!(twice["entries"].as_array().unwrap().len(), 18);
    assert_eq!(twice["entries"][1]["run"], 2);
    assert_eq!(twice["per_entry"]["alarm"], json!({"false_alarm": 2}));
    assert_eq!(twice["totals"]["hit"], 4);
    assert_eq!(
        (&twice["score"], &twice["mean"]),
        (&json!("8/12"), &json!(4.67))
    );
    // Refused, before any session: no such entry, a set that does not
    // parse, a name that is not a plain directory name.
    std::fs::write(dir.path().join("bad.toml"), "entry = 1\n").unwrap();
    std::fs::write(dir.path().join("dots.toml"), entry("../x", "o/demo")).unwrap();
    let pathless =
        entry("p", "o/demo") + "known = [{ paths = [], words = [\"w\"], what = \"w\" }]\n";
    std::fs::write(dir.path().join("pathless.toml"), pathless).unwrap();
    let twice = entry("t", "o/demo") + &entry("t", "o/demo");
    std::fs::write(dir.path().join("twice.toml"), twice).unwrap();
    let k = "{ paths = [\"a\"], words = [\"w\"], what = \"same\" }";
    let whats = entry("w", "o/demo") + &format!("known = [{k}, {k}]\n");
    std::fs::write(dir.path().join("whats.toml"), whats).unwrap();
    for (set, only, why) in [
        ("set.toml", &["--only", "nope"][..], "no entry \"nope\""),
        ("bad.toml", &[], "is not a review eval set"),
        ("dots.toml", &[], "entry name \"../x\""),
        ("pathless.toml", &[], "with no paths"),
        ("twice.toml", &[], "repeats a name"),
        ("whats.toml", &[], "repeats a name or a known's what"),
    ] {
        let (ok, err) = eval(set, only);
        assert!(!ok && err["code"] == "invalid_input", "{err}");
        assert!(err["message"].as_str().unwrap().contains(why), "{err}");
    }
    let (ok, err) = eval("set.toml", &["--runs", "0"]);
    assert!(!ok && err["code"] == "usage", "{err}");
    let base = git(&demo, &["rev-parse", "HEAD"]);
    std::fs::write(demo.join("known.rs"), "fn f() {}\n").unwrap();
    git(&demo, &["add", "."]);
    git(&demo, &["commit", "-qm", "known"]);
    let head = git(&demo, &["rev-parse", "HEAD"]);
    let known = format!(
        "[[entry]]\nname = \"known\"\nrepo = \"o/demo\"\nbase = \"{base}\"\nhead = \"{head}\"\n\
         goal = \"GOAL-KNOWN\"\nknown = [{{ paths = [\"known.rs\"], words = [\"STALE\"], what = \"kw\" }}, {{ paths = [\"known.rs\"], words = [\"never\"], what = \"nw\" }}]\n"
    );
    std::fs::write(dir.path().join("known.toml"), known).unwrap();
    let (ok, out) = eval("known.toml", &[]);
    assert!(ok, "{out}");
    let row = &out["entries"][0];
    assert_eq!(
        (&row["outcome"], &row["findings"]),
        (&json!("clean"), &json!([]))
    );
    assert_eq!(
        (&row["known"][0]["title"], &row["known"][0]["known"]),
        (&json!("A stale todo"), &json!("kw"))
    );
    assert_eq!(
        out["unused_known"],
        json!([{"name": "known", "what": "nw"}])
    );
    let base = git(&demo, &["rev-parse", "HEAD"]);
    std::fs::write(demo.join("big.rs"), "x\n".repeat(200)).unwrap();
    git(&demo, &["add", "."]);
    git(&demo, &["commit", "-qm", "big"]);
    let head = git(&demo, &["rev-parse", "HEAD"]);
    let big = format!(
        "[[entry]]\nname = \"big\"\nrepo = \"o/demo\"\nbase = \"{base}\"\nhead = \"{head}\"\ngoal = \"g\"\n"
    );
    std::fs::write(dir.path().join("big.toml"), big).unwrap();
    std::fs::remove_file(bin.join("codex")).unwrap();
    std::fs::write(bin.join("codex"), "not a program").unwrap();
    let (ok, fell) = eval("big.toml", &["--runs", "2"]);
    assert!(ok, "{fell}");
    assert_eq!(
        fell["per_entry"]["big"],
        json!({"clean": 2, "fallbacks": 2})
    );
    assert_eq!(
        (
            &fell["entries"][0]["fallbacks"],
            &fell["totals"]["fallbacks"]
        ),
        (&json!(1), &json!(2))
    );
    assert!(!db.exists(), "the eval opened the ledger");
}

/// Eval skips the Codex lens on #223's measure, removed lines too: an entry
/// removing 100 lines and adding 150 is 250, past `codex_min_code_lines`
/// 200, so Codex runs; on its added lines alone it would be skipped.
#[test]
fn eval_skips_codex_on_added_plus_removed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, demo) = (dir.path().join("bin"), dir.path().join("repos/demo"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&demo).unwrap();
    git(&demo, &["init", "-q", "-b", "main"]);
    std::fs::write(demo.join("old.rs"), "x\n".repeat(100)).unwrap();
    git(&demo, &["add", "."]);
    git(&demo, &["commit", "-qm", "base"]);
    let base = git(&demo, &["rev-parse", "HEAD"]);
    std::fs::remove_file(demo.join("old.rs")).unwrap();
    std::fs::write(demo.join("new.rs"), "y\n".repeat(150)).unwrap();
    git(&demo, &["add", "-A"]);
    git(&demo, &["commit", "-qm", "swap"]);
    let head = git(&demo, &["rev-parse", "HEAD"]);
    let set = format!(
        "[[entry]]\nname = \"swap\"\nrepo = \"o/demo\"\nbase = \"{base}\"\nhead = \"{head}\"\ngoal = \"g\"\n"
    );
    std::fs::write(dir.path().join("set.toml"), set).unwrap();
    let claude = json!([says("# Review:", json!({"findings": []}))]);
    let codex = json!([{"when": "# Review:", "write": {"-o": r#"{"findings": []}"#}}]);
    let fake = Path::new(BIN).with_file_name("chaperone-fake");
    for (cli, runs) in [("claude", claude), ("codex", codex)] {
        std::os::unix::fs::symlink(&fake, bin.join(cli)).unwrap();
        let script = json!({"version": "1 (fake)", "runs": runs}).to_string();
        std::fs::write(bin.join(format!("{cli}.json")), script).unwrap();
    }
    let path = std::env::var("PATH").unwrap();
    let out = Command::new(BIN)
        .arg("--db")
        .arg(dir.path().join("ledger.db"))
        .args(["review", "eval", "--repos"])
        .arg(dir.path().join("repos"))
        .arg("--set")
        .arg(dir.path().join("set.toml"))
        .env("PATH", format!("{}:{path}", bin.display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let codex = std::fs::read_to_string(bin.join("codex.log")).unwrap_or_default();
    assert_eq!(codex.lines().count(), 1, "codex ran once");
}
