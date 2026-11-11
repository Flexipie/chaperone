//! `pipeline limits` through the binary.

use std::process::Command;

use serde_json::Value;

/// Each lane prints its own pipeline's table: v20's pilot, v19's default.
#[test]
fn limits_are_the_lanes_pipelines() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.db");
    let run = |args: &[&str]| -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_chaperone"))
            .arg("--db")
            .arg(&db)
            .args(args)
            .envs([("DAISY_AGENT", "lane"), ("DAISY_SESSION", "lane")])
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    };
    run(&["migrate"]);
    run(&["work", "create", "--title", "t"]);
    run(&["work", "create", "--title", "u"]);
    run(&["work", "claim", "1", "--pipeline", "v20"]);
    run(&["work", "claim", "2"]);
    let v20 = run(&["pipeline", "limits", "--work", "1"]);
    assert_eq!(v20["pipeline"]["id"], "v20");
    assert_eq!(v20["limits"]["code_lines"], 400);
    let v19 = run(&["pipeline", "limits", "--work", "2"]);
    assert_eq!(v19["pipeline"]["id"], "v19");
    assert_eq!(v19["limits"]["code_lines"], 750);
}
