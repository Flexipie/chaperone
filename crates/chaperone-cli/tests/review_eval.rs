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
    git(&demo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let mut set = String::new();
    let entries = [
        ("hit", "GOAL-HIT", Some("hit.rs")),
        ("miss", "GOAL-MISS", Some("miss.rs")),
        ("alarm", "GOAL-ALARM", None),
        ("refuted", "GOAL-REFUTED", None),
        ("broken", "GOAL-BROKEN", None),
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
    set += "[[entry]]\nname = \"gone\"\nrepo = \"o/absent\"\nbase = \"a\"\nhead = \"b\"\ngoal = \"g\"\n";
    std::fs::write(dir.path().join("set.toml"), set).unwrap();
    let refuted = risk("refuted.rs", "refuted-title");
    let key = review::findings("intent", refuted.clone()).unwrap()[0]
        .finding_key
        .clone();
    let claude = json!([
        says(
            "refuted-title",
            json!({"rulings": [{"key": key, "verdict": "refuted", "why": "w"}]})
        ),
        says("# Verify the review's findings", json!({"rulings": []})),
        says("GOAL-HIT", risk("hit.rs", "Overflow in add")),
        says("GOAL-ALARM", risk("alarm.rs", "alarm-title")),
        says("GOAL-REFUTED", refuted),
        says("GOAL-BROKEN", json!({"bogus": 1})),
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
    let eval = |only: &[&str]| {
        let path = std::env::var("PATH").unwrap();
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(["review", "eval", "--repos"])
            .arg(dir.path().join("repos"))
            .arg("--set")
            .arg(dir.path().join("set.toml"))
            .args(only)
            .env("PATH", format!("{}:{path}", bin.display()))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    };
    let out = eval(&[]);
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
            ("broken", "error"),
            ("gone", "skipped"),
        ]
    );
    let one = json!({"hit": 1, "miss": 1, "false_alarm": 1, "clean": 1, "skipped": 1,
        "error": 1});
    assert_eq!((&out["totals"], &out["score"]), (&one, &json!("2/4")));
    assert_eq!(out["entries"][1]["bug"], "w");
    let only = eval(&["--only", "hit"]);
    assert_eq!(only["entries"].as_array().unwrap().len(), 1);
    assert_eq!(only["score"], "1/1");
    assert!(!db.exists(), "the eval opened the ledger");
}
