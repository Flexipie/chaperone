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
        (
            &["work", "merged"],
            include_str!("snapshots/work-merged.txt"),
        ),
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
        (&["check", "run"], include_str!("snapshots/check-run.txt")),
        (
            &["check", "waive"],
            include_str!("snapshots/check-waive.txt"),
        ),
        (
            &["check", "prune"],
            include_str!("snapshots/check-prune.txt"),
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
        (&["attention"], include_str!("snapshots/attention.txt")),
        (
            &["attention", "record"],
            include_str!("snapshots/attention-record.txt"),
        ),
        (&["dossier"], include_str!("snapshots/dossier.txt")),
        (
            &["dossier", "note"],
            include_str!("snapshots/dossier-note.txt"),
        ),
        (
            &["dossier", "get"],
            include_str!("snapshots/dossier-get.txt"),
        ),
        (&["evidence"], include_str!("snapshots/evidence.txt")),
        (
            &["evidence", "show"],
            include_str!("snapshots/evidence-show.txt"),
        ),
        (&["finding"], include_str!("snapshots/finding.txt")),
        (
            &["finding", "list"],
            include_str!("snapshots/finding-list.txt"),
        ),
        (
            &["finding", "dispute"],
            include_str!("snapshots/finding-dispute.txt"),
        ),
        (
            &["finding", "waive"],
            include_str!("snapshots/finding-waive.txt"),
        ),
        (
            &["finding", "accept"],
            include_str!("snapshots/finding-accept.txt"),
        ),
        (
            &["finding", "answer"],
            include_str!("snapshots/finding-answer.txt"),
        ),
        (&["usage"], include_str!("snapshots/usage.txt")),
        (
            &["usage", "report"],
            include_str!("snapshots/usage-report.txt"),
        ),
        (&["usage", "get"], include_str!("snapshots/usage-get.txt")),
        (&["overlap"], include_str!("snapshots/overlap.txt")),
        (&["stats"], include_str!("snapshots/stats.txt")),
        (&["impact"], include_str!("snapshots/impact.txt")),
        (&["verify"], include_str!("snapshots/verify.txt")),
        (&["pr"], include_str!("snapshots/pr.txt")),
        (&["pr", "open"], include_str!("snapshots/pr-open.txt")),
        (&["merge"], include_str!("snapshots/merge.txt")),
        (&["pipeline"], include_str!("snapshots/pipeline.txt")),
        (
            &["pipeline", "current"],
            include_str!("snapshots/pipeline-current.txt"),
        ),
        (
            &["pipeline", "limits"],
            include_str!("snapshots/pipeline-limits.txt"),
        ),
        (
            &["pipeline", "history"],
            include_str!("snapshots/pipeline-history.txt"),
        ),
        (&["doctor"], include_str!("snapshots/doctor.txt")),
        (&["migrate"], include_str!("snapshots/migrate.txt")),
        (&["install"], include_str!("snapshots/install.txt")),
        (&["uninstall"], include_str!("snapshots/uninstall.txt")),
        (&["hook"], include_str!("snapshots/hook.txt")),
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
