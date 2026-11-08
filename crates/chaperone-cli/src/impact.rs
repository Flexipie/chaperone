//! `chaperone impact <files>`: the files that depend on them, nearest
//! first, from the code graph (`chaperone-graph`), and with
//! `--entry-points` the entry points they reach, from the pipeline
//! profile. Reads the worktree and git, never the ledger.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::git::{Failure, git, git_bin};
use chaperone_core::ledger::Refusal;
use chaperone_graph::{File, Rules};

use crate::cli::Format;
use crate::output::{self, CliError, Meta, Outcome};

/// For every git call of one run, together.
const DEADLINE: Duration = Duration::from_secs(5);
/// `ls-files` of a large repo; a cut list would drop files silently, so
/// past it the command refuses.
const OUT_BYTES: usize = 64 * 1024 * 1024;
/// Dependent files listed; the rest are counted.
const LIMIT: usize = 500;

pub fn run(
    targets: Vec<String>,
    cache: Option<PathBuf>,
    entry_points: bool,
    profile: Option<PathBuf>,
    meta: &Meta,
    format: Format,
) -> Result<Outcome, CliError> {
    let deadline = Instant::now() + DEADLINE;
    let bin = git_bin(deadline);
    // From here, `:/` is the whole tree, so the four calls are independent.
    let call = |args: &[&str]| match git(&bin, Path::new("."), args, deadline, OUT_BYTES) {
        Ok((out, false)) => Ok(out),
        Ok((_, true)) => Err(CliError::Internal(format!(
            "git {}: output past {OUT_BYTES} bytes",
            args[0]
        ))),
        Err(Failure::TimedOut) => Err(CliError::Internal(format!("git {}: timed out", args[0]))),
        Err(Failure::BadRevision) => {
            Err(CliError::Internal(format!("git {}: bad revision", args[0])))
        }
        Err(Failure::Other(e)) => Err(Refusal::InvalidInput(e).into()),
    };
    let joined = |h: std::thread::ScopedJoinHandle<'_, Result<Vec<u8>, CliError>>| {
        h.join()
            .unwrap_or_else(|_| Err(CliError::Internal("git thread panicked".into())))
    };
    let (out, origin, head, status, staged) = std::thread::scope(|s| {
        let rev = s.spawn(|| {
            call(&[
                "rev-parse",
                "--show-toplevel",
                "--show-prefix",
                "--git-common-dir",
            ])
        });
        let origin = s.spawn(|| match cache {
            Some(_) if profile.is_some() || !entry_points => Ok(Vec::new()),
            _ => call(&["config", "--get", "remote.origin.url"]),
        });
        // Its own call: `HEAD` fails in a repo with no commits yet.
        let head = s.spawn(|| match entry_points {
            true => call(&["rev-parse", "HEAD"]),
            false => Ok(Vec::new()),
        });
        let status = s.spawn(|| {
            call(&[
                "ls-files",
                "-z",
                "-t",
                "-m",
                "-d",
                "-o",
                "--exclude-standard",
                "--full-name",
                "--",
                ":/",
            ])
        });
        let staged = call(&["ls-files", "-z", "-s", "--full-name", "--", ":/"]);
        (
            joined(rev),
            joined(origin),
            joined(head),
            joined(status),
            staged,
        )
    });
    let out = out?;
    let out = String::from_utf8_lossy(&out);
    let mut lines = out.lines();
    let (top, prefix, common) = (
        PathBuf::from(lines.next().unwrap_or_default()),
        lines.next().unwrap_or_default().to_string(),
        lines.next().unwrap_or_default().to_string(),
    );
    let targets: Vec<String> = targets
        .iter()
        .map(|t| repo_path(t, &top, &prefix))
        .collect();

    let origin = origin
        .map(|o| String::from_utf8_lossy(&o).trim().to_string())
        .ok()
        .filter(|o| !o.is_empty());
    let rules = match entry_points {
        true => Some(rules(profile, origin.as_deref())?),
        false => None,
    };
    let head = String::from_utf8_lossy(&head?).trim().to_string();

    // One cache per origin, so every worktree and clone of it shares one.
    let given = cache.is_some();
    let cache = match cache {
        Some(c) => Some(c),
        None => {
            let origin = origin.or_else(|| {
                std::fs::canonicalize(Path::new(&common))
                    .ok()
                    .map(|p| p.display().to_string())
            });
            dirs::cache_dir()
                .zip(origin)
                .map(|(d, o)| chaperone_graph::cache_file(&d.join("chaperone").join("graph"), &o))
        }
    };

    // Tracked files by git's blob id, less deleted ones; changed and
    // untracked ones are hashed from the worktree.
    let (mut changed, mut deleted) = (HashSet::new(), HashSet::new());
    let mut files = Vec::new();
    // Every file of the tree, for `Rules::check`.
    let mut tree = HashSet::new();
    let out = status?;
    for e in out.split(|b| *b == 0).filter(|e| e.len() > 2) {
        let path = String::from_utf8_lossy(&e[2..]).to_string();
        if rules.is_some() && e[0] == b'?' {
            tree.insert(path.clone());
        }
        match e[0] {
            b'R' => {
                deleted.insert(path);
            }
            b'C' if wanted(&path) => {
                changed.insert(path);
            }
            b'?' if wanted(&path) => files.push(File { path, key: None }),
            _ => {}
        }
    }
    files.extend(changed.difference(&deleted).map(|path| File {
        path: path.clone(),
        key: None,
    }));
    let out = staged?;
    for e in out.split(|b| *b == 0) {
        // `<mode> <blob> <stage>\t<path>`; a conflict lists stages 1 to 3.
        let Some((head, path)) = String::from_utf8_lossy(e)
            .split_once('\t')
            .map(|(h, p)| (h.to_string(), p.to_string()))
        else {
            continue;
        };
        let mut f = head.split(' ');
        let (blob, stage) = (f.nth(1), f.next());
        if rules.is_some() && !deleted.contains(&path) {
            tree.insert(path.clone());
        }
        if wanted(&path) && !changed.contains(&path) && stage == Some("0") {
            files.push(File {
                path,
                key: blob.map(String::from),
            });
        }
    }

    let read = |p: &str| std::fs::read(top.join(p));
    // What `git add` would store for those paths now, with its filters.
    let ids = |paths: &[&str]| {
        let mut out = Vec::with_capacity(paths.len());
        for chunk in paths.chunks(1000) {
            let start = out.len();
            let args = [&["hash-object", "--"], chunk].concat();
            if let Ok((o, false)) = git(&bin, &top, &args, deadline, OUT_BYTES) {
                out.extend(
                    String::from_utf8_lossy(&o)
                        .lines()
                        .map(|l| Some(l.to_string())),
                );
            }
            // Unknown on a failed call: those files are keyed by their bytes.
            out.resize(start + chunk.len(), None);
        }
        out
    };
    if let Some(rules) = &rules {
        rules
            .check(tree.iter().map(String::as_str))
            .map_err(|e| Refusal::InvalidInput(format!("entry points: {e}")))?;
    }
    let entries = rules.as_ref().map(|r| (r, head));
    let data = chaperone_graph::impact(
        &files,
        read,
        ids,
        cache.as_deref(),
        given,
        &targets,
        LIMIT,
        entries,
    );
    output::write_one(&data, meta, format)
}

/// The entry rules: from `profile`, or the origin's pipeline profile,
/// found the way `pipeline-profile` finds it. No rules is a refusal, never
/// an empty answer.
fn rules(profile: Option<PathBuf>, origin: Option<&str>) -> Result<Rules, CliError> {
    let refuse = |e: String| CliError::from(Refusal::InvalidInput(e));
    let (path, slug) = match profile {
        Some(p) => (p, None),
        None => {
            let slug = origin.and_then(slug).ok_or_else(|| {
                refuse(
                    "entry points: no GitHub origin to find the profile by; pass --profile".into(),
                )
            })?;
            let file = format!("{}.md", slug.replace('/', "-").to_lowercase());
            let dir =
                dirs::home_dir().ok_or_else(|| refuse("entry points: no home directory".into()))?;
            (
                dir.join("dotfiles/claude/pipeline/profiles").join(file),
                Some(slug),
            )
        }
    };
    let at = path.display();
    let text = std::fs::read_to_string(&path).map_err(|e| refuse(format!("profile {at}: {e}")))?;
    let rules = Rules::from_profile(&text).map_err(|e| refuse(format!("profile {at}: {e}")))?;
    if let Some(slug) = slug
        && rules.slug.as_deref().map(str::to_lowercase) != Some(slug.to_lowercase())
    {
        return Err(refuse(format!(
            "profile {at} says slug {:?}, the origin is {slug}",
            rules.slug.as_deref().unwrap_or_default()
        )));
    }
    Ok(rules)
}

/// `owner/repo` of a GitHub remote URL, ssh or https, `.git` dropped.
fn slug(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("github.com")?;
    let rest = rest.strip_prefix([':', '/'])?.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}"))
}

/// The files the graph reads: Rust sources and the manifests that name
/// their crates.
fn wanted(path: &str) -> bool {
    path.ends_with(".rs") || path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

/// A target as the repo names it: its real path (git's top level is one,
/// and `/tmp` is `/private/tmp` on macOS) less the top level. Outside the
/// tree, or under no directory that exists, it stays as given.
fn repo_path(t: &str, top: &Path, prefix: &str) -> String {
    let p = top.join(prefix).join(t);
    p.parent()
        .and_then(|d| std::fs::canonicalize(d).ok())
        .zip(p.file_name())
        .and_then(|(d, n)| Some(d.join(n).strip_prefix(top).ok()?.display().to_string()))
        .unwrap_or_else(|| t.to_string())
}
