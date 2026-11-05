use std::process::Command;

/// Each command's `--help` against its snapshot. Regenerate one with:
/// `cargo run -q -p chaperone-cli -- <args> --help > crates/chaperone-cli/tests/snapshots/<file>`
#[test]
fn help_matches_snapshots() {
    for (args, snapshot) in [
        (&[][..], include_str!("snapshots/help.txt")),
        (&["work"], include_str!("snapshots/work.txt")),
        (
            &["work", "create"],
            include_str!("snapshots/work-create.txt"),
        ),
        (&["work", "claim"], include_str!("snapshots/work-claim.txt")),
        (
            &["work", "heartbeat"],
            include_str!("snapshots/work-heartbeat.txt"),
        ),
        (
            &["work", "update"],
            include_str!("snapshots/work-update.txt"),
        ),
        (
            &["work", "release"],
            include_str!("snapshots/work-release.txt"),
        ),
        (
            &["work", "cancel"],
            include_str!("snapshots/work-cancel.txt"),
        ),
        (&["work", "get"], include_str!("snapshots/work-get.txt")),
        (&["work", "list"], include_str!("snapshots/work-list.txt")),
        (
            &["work", "handoff"],
            include_str!("snapshots/work-handoff.txt"),
        ),
        (
            &["work", "handoffs"],
            include_str!("snapshots/work-handoffs.txt"),
        ),
        (
            &["work", "events"],
            include_str!("snapshots/work-events.txt"),
        ),
        (&["work", "ready"], include_str!("snapshots/work-ready.txt")),
        (&["work", "board"], include_str!("snapshots/work-board.txt")),
        (&["spec"], include_str!("snapshots/spec.txt")),
        (
            &["spec", "create"],
            include_str!("snapshots/spec-create.txt"),
        ),
        (&["spec", "get"], include_str!("snapshots/spec-get.txt")),
        (&["spec", "list"], include_str!("snapshots/spec-list.txt")),
        (
            &["spec", "update"],
            include_str!("snapshots/spec-update.txt"),
        ),
        (
            &["spec", "approve"],
            include_str!("snapshots/spec-approve.txt"),
        ),
        (
            &["spec", "decide"],
            include_str!("snapshots/spec-decide.txt"),
        ),
        (&["check"], include_str!("snapshots/check.txt")),
        (
            &["check", "record"],
            include_str!("snapshots/check-record.txt"),
        ),
        (&["reviews"], include_str!("snapshots/reviews.txt")),
        (
            &["reviews", "record"],
            include_str!("snapshots/reviews-record.txt"),
        ),
        (
            &["reviews", "get"],
            include_str!("snapshots/reviews-get.txt"),
        ),
        (&["jobs"], include_str!("snapshots/jobs.txt")),
        (&["jobs", "get"], include_str!("snapshots/jobs-get.txt")),
        (&["usage"], include_str!("snapshots/usage.txt")),
        (
            &["usage", "report"],
            include_str!("snapshots/usage-report.txt"),
        ),
        (&["usage", "get"], include_str!("snapshots/usage-get.txt")),
        (&["migrate"], include_str!("snapshots/migrate.txt")),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_chaperone"))
            .args(args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), snapshot, "{args:?}");
    }
}
