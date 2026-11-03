use std::process::Command;

/// Each command's `--help` against its snapshot. Regenerate one with:
/// `cargo run -q -p seamlane -- <args> --help > crates/seamlane/tests/snapshots/<file>`
#[test]
fn help_matches_snapshots() {
    for (args, snapshot) in [
        (&[][..], include_str!("snapshots/help.txt")),
        (&["work"], include_str!("snapshots/work.txt")),
        (
            &["work", "create"],
            include_str!("snapshots/work-create.txt"),
        ),
        (&["work", "get"], include_str!("snapshots/work-get.txt")),
        (&["work", "list"], include_str!("snapshots/work-list.txt")),
        (&["migrate"], include_str!("snapshots/migrate.txt")),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_seamlane"))
            .args(args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), snapshot, "{args:?}");
    }
}
