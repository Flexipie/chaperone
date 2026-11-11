//! `evidence show` through the binary: the stored bytes piped as they are,
//! and each refusal's code (docs/cli-contract.md).

use std::path::Path;
use std::process::Command;

use chaperone_core::ledger::Evidence;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_chaperone");

/// The exit code, stdout, and the error envelope's `error` on a refusal.
fn show(db: &Path, reference: &str) -> (i32, Vec<u8>, Value) {
    let out = Command::new(BIN)
        .arg("--db")
        .arg(db)
        .args(["evidence", "show", reference])
        .output()
        .unwrap();
    let error =
        serde_json::from_slice::<Value>(&out.stderr).map_or(Value::Null, |e| e["error"].clone());
    (out.status.code().unwrap(), out.stdout, error)
}

#[test]
fn show_prints_the_bytes_and_refuses_by_code() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.db");
    let text = "answer \x1b[31mRED\n";
    let hash = Evidence::beside(&db).put(text).unwrap().hash;
    for reference in [&hash[..], &hash[..12]] {
        let (code, out, _) = show(&db, reference);
        assert_eq!((code, out.as_slice()), (0, text.as_bytes()));
    }
    let refused = |reference: &str| {
        let (code, out, error) = show(&db, reference);
        assert!(out.is_empty());
        (
            code,
            error["code"].clone(),
            error["details"]["entity"].clone(),
        )
    };
    let unknown = "0".repeat(64);
    assert_eq!(
        refused(&unknown),
        (1, "not_found".into(), "evidence".into())
    );
    assert_eq!(refused(&hash[..11]).1, "invalid_input");

    let file = dir.path().join("evidence/sha256").join(&hash[..2]);
    let twin = format!("{}{}", &hash[..12], "0".repeat(52));
    std::fs::write(file.join(&twin), "x").unwrap();
    assert_eq!(refused(&hash[..12]).1, "invalid_input");
    std::fs::write(file.join(&hash), "edited").unwrap();
    assert_eq!(refused(&hash).1, "internal");
}
