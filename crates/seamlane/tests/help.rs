use std::process::Command;

#[test]
fn help_matches_snapshot() {
    let out = Command::new(env!("CARGO_BIN_EXE_seamlane"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        include_str!("snapshots/help.txt"),
        "regenerate with: cargo run -q -p seamlane -- --help > crates/seamlane/tests/snapshots/help.txt"
    );
}
