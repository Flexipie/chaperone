//! The commit this binary is built from, as `CHAPERONE_GIT_SHA` (empty
//! outside a git checkout), so `chaperone doctor --repo` can tell an
//! installed binary that lags main.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let sha = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=CHAPERONE_GIT_SHA={sha}");
    // In a worktree `.git` is a file: watch the paths git resolves, the
    // branch's ref under the common dir and packed-refs included.
    let mut watch = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(head);
    }
    for p in watch {
        // A missing path would rerun the script on every build.
        if let Some(path) = git(&["rev-parse", "--git-path", &p])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
