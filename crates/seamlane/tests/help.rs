use std::process::Command;

fn run(arg: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_seamlane"))
        .arg(arg)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn version() {
    assert_eq!(run("--version"), "seamlane 0.1.0\n");
}

#[test]
fn help_matches_snapshot() {
    assert_eq!(
        run("--help"),
        include_str!("snapshots/help.txt"),
        "regenerate with: cargo run -q -p seamlane -- --help > crates/seamlane/tests/snapshots/help.txt"
    );
}
