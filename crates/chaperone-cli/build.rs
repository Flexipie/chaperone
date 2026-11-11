//! The commit this binary is built from, as `CHAPERONE_GIT_SHA` (empty
//! outside a git checkout), so `chaperone doctor --repo` can tell an
//! installed binary that lags main. And every file under the repo's
//! `plugin/`, as `OUT_DIR/plugin.rs`, for `chaperone install`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

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
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../plugin");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut src = String::from("&[\n");
    for path in files {
        let rel = path.strip_prefix(&root).unwrap().to_str().unwrap();
        let abs = path.to_str().unwrap();
        src += &format!("    ({rel:?}, include_bytes!({abs:?})),\n");
    }
    src += "]\n";
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("plugin.rs");
    std::fs::write(out, src).unwrap();
}
