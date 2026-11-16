//! `git::patch_id` on a real repository: the lane's diff keeps its id
//! across a moved base and loses it on any change of its own, whatever
//! the repository's diff config.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use chaperone_core::git;

fn git(wt: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    let cmd = cmd.arg("-C").arg(wt);
    let out = cmd
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args);
    let out = out.output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit(wt: &Path, files: &[(&str, &str)]) -> String {
    for (path, text) in files {
        std::fs::write(wt.join(path), text).unwrap();
    }
    git(wt, &["add", "-A"]);
    git(wt, &["commit", "-qm", "c"]);
    git(wt, &["rev-parse", "HEAD"])
}

fn id(bin: &Path, wt: &Path, base: &str, head: &str) -> Option<String> {
    capped(bin, wt, base, head, 1 << 20).ok().unwrap()
}

fn capped(
    bin: &Path,
    wt: &Path,
    base: &str,
    head: &str,
    cap: usize,
) -> Result<Option<String>, git::Failure> {
    let deadline = Instant::now() + Duration::from_secs(10);
    git::patch_id(bin, wt, base, head, deadline, cap)
}

/// A repository whose config hides submodules and context from a plain
/// `git diff`, its base, and the lane's commit on it.
fn repo() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path();
    git(wt, &["init", "-q", "-b", "main"]);
    git(wt, &["config", "diff.ignoreSubmodules", "all"]);
    git(wt, &["config", "diff.submodule", "log"]);
    git(wt, &["config", "diff.context", "0"]);
    let base = commit(wt, &[("f", "1\n2\n3\n4\n5\n6\n7\n8\n9\n")]);
    let lane = commit(wt, &[("f", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n")]);
    (dir, base, lane)
}

#[test]
fn a_moved_base_keeps_the_id_and_any_lane_change_gets_a_new_one() {
    let (dir, base, lane) = repo();
    let (wt, bin) = (dir.path(), Path::new("git"));
    let reviewed = id(bin, wt, &base, &lane).unwrap();

    git(wt, &["checkout", "-q", "-b", "next", &base]);
    let moved = commit(wt, &[("f", "0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n"), ("g", "g\n")]);
    git(wt, &["checkout", "-q", "-b", "merged", &lane]);
    git(wt, &["merge", "-q", "--no-edit", "next"]);
    let merged = git(wt, &["rev-parse", "HEAD"]);
    assert_eq!(id(bin, wt, &moved, &merged), Some(reviewed.clone()));

    let space = commit(wt, &[("f", "0\n1\n2\n3\n4\n5\n6\n7\n8\n9\nten \n")]);
    git(wt, &["reset", "-q", "--hard", &merged]);
    git(wt, &["update-index", "--chmod=+x", "f"]);
    git(wt, &["commit", "-qm", "mode"]);
    let mode = git(wt, &["rev-parse", "HEAD"]);
    git(wt, &["reset", "-q", "--hard", &merged]);
    let empty = commit(wt, &[("e", "")]);
    git(wt, &["reset", "-q", "--hard", &merged]);
    let gitlink = format!("160000,{base},sub");
    git(wt, &["update-index", "--add", "--cacheinfo", &gitlink]);
    git(wt, &["commit", "-qm", "sub"]);
    let sub = git(wt, &["rev-parse", "HEAD"]);
    git(wt, &["reset", "-q", "--hard", &merged]);
    let line_moved = commit(wt, &[("f", "0\n1\n2\nten\n3\n4\n5\n6\n7\n8\n9\n")]);
    for head in [space, mode, empty, sub, line_moved] {
        assert_ne!(id(bin, wt, &moved, &head), Some(reviewed.clone()), "{head}");
    }
}

/// A git whose patch-id leaves file modes out, as before 2.41: a mode
/// change or an added empty file still gets a new id.
#[test]
fn modes_count_where_patch_id_leaves_them_out() {
    let (dir, base, lane) = repo();
    let wt = dir.path();
    let shim = wt.join(".git/old-git");
    let script = "#!/bin/sh\ncase \" $* \" in *' patch-id '*) cat >/dev/null; echo 1 0;; *) exec git \"$@\";; esac\n";
    std::fs::write(&shim, script).unwrap();
    git(wt, &["update-index", "--chmod=+x", "f"]);
    git(wt, &["commit", "-qm", "mode"]);
    let mode = git(wt, &["rev-parse", "HEAD"]);
    let empty = commit(wt, &[("e", "")]);
    let chmod = Command::new("chmod").arg("+x").arg(&shim).status();
    assert!(chmod.unwrap().success());
    let reviewed = id(&shim, wt, &base, &lane).unwrap();
    let mode = id(&shim, wt, &base, &mode);
    assert_ne!(mode, Some(reviewed));
    assert_ne!(id(&shim, wt, &base, &empty), mode);
}

/// No id for a diff past the cap, which would hash only its start; a
/// patch-id that fails is an error, never an id.
#[test]
fn a_cut_diff_has_no_id_and_a_failed_patch_id_is_an_error() {
    let (dir, base, lane) = repo();
    let wt = dir.path();
    assert!(matches!(
        capped(Path::new("git"), wt, &base, &lane, 8),
        Ok(None)
    ));
    let shim = wt.join(".git/failing-git");
    let script = "#!/bin/sh\ncase \" $* \" in *' patch-id '*) exit 1;; *) exec git \"$@\";; esac\n";
    std::fs::write(&shim, script).unwrap();
    let chmod = Command::new("chmod").arg("+x").arg(&shim).status();
    assert!(chmod.unwrap().success());
    assert!(capped(&shim, wt, &base, &lane, 1 << 20).is_err());
}
