//! `work claim` through the binary, with a fake `gh` first on `PATH` that
//! logs its argv and answers issue o/r#1 only.

use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// A github lane's first claim stores the issue as gh reads it, or why gh
/// could not; a lane of another source starts no gh; a later claim reads nothing again and keeps the
/// first claim's text.
#[test]
fn the_first_claim_stores_the_issue_text() {
    let dir = tempfile::tempdir().unwrap();
    let (db, bin, log) = (
        dir.path().join("ledger.db"),
        dir.path().join("bin"),
        dir.path().join("gh.log"),
    );
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    let issue = r#"{"title":"ISSUE","body":"done when: X","url":"u"}"#;
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> {log:?}\n\
             [ \"$3\" = 1 ] && echo '{issue}' && exit 0\necho 'no such issue' >&2\nexit 1\n"
        ),
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&gh, mode).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |args: &[&str]| -> Value {
        let out = Command::new(BIN)
            .arg("--db")
            .arg(&db)
            .args(args)
            .envs([("DAISY_AGENT", "lane"), ("DAISY_SESSION", "lane")])
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    };
    let texts = |id: &str| -> Vec<Value> {
        let events = run(&["work", "events", id]);
        let claims = events["items"].as_array().unwrap().iter();
        let claims = claims.filter(|e| e["action"] == "claim");
        let detail = |e: &Value| serde_json::from_str::<Value>(e["detail_json"].as_str().unwrap());
        claims
            .map(|e| detail(e).unwrap()["source_text"].clone())
            .collect()
    };
    run(&["migrate"]);
    for (id, args) in [
        ("1", &["--source", "github", "--source-id", "o/r#1"][..]),
        ("2", &["--source", "github", "--source-id", "o/r#2"]),
        ("3", &["--source", "manual"]),
    ] {
        run(&[&["work", "create", "--title", "t"][..], args].concat());
        run(&["work", "claim", id]);
    }
    run(&["work", "release", "1"]);
    run(&["work", "claim", "1"]);
    let got: Vec<_> = ["1", "2", "3"].map(texts).into();
    let want = [
        vec![
            json!({"title": "ISSUE", "body": "done when: X"}),
            Value::Null,
        ],
        vec![json!({"error": "gh issue view: no such issue"})],
        vec![Value::Null],
    ];
    assert_eq!(got, want);
    let calls = std::fs::read_to_string(&log).unwrap();
    let want = "issue view 1 -R o/r --json title,body\nissue view 2 -R o/r --json title,body\n";
    assert_eq!(calls, want);
}
