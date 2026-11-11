//! What `chaperone impact` runs: the tree and its blob ids from git, and
//! the entry rules from the repo's pipeline profile, then [`impact`].
//! Reads the worktree and git, never the ledger. The profile's `[tests]`
//! for `check run tests` is read here too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chaperone_core::git::{BASES, Failure, git, git_bin};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Entries, File, Impact, Rules, cache_file, impact};

/// For every git call of one run, together.
const DEADLINE: Duration = Duration::from_secs(5);
/// `ls-files` of a large repo; a cut list would drop files silently, so
/// past it the call refuses.
const OUT_BYTES: usize = 64 * 1024 * 1024;
/// Dependent files listed; the rest are counted.
const LIMIT: usize = 500;
/// The repo's own profile: TOML, read from its base branch's remote ref and
/// never from the worktree, so an edit in a lane's change is not used. The
/// ref lives in the lane's clone, so this is a speed bump, not a guarantee.
pub const REPO_PROFILE: &str = ".chaperone/profile.toml";
/// A profile past this is refused, not cut.
const PROFILE_BYTES: usize = 1024 * 1024;

/// Why [`impact_in`] gave no answer: the caller's input (the CLI's
/// `invalid_input`), or this side's failure (`internal`).
#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Internal(String),
}

/// `impact` as `chaperone impact` answers it, for the repo `dir` is in;
/// `targets` are relative to `dir`, a relative `cache` or `profile` to
/// the process's working directory. `declared` is compared only with
/// `entry_points` (it needs the profile's areas).
pub fn impact_in(
    dir: &Path,
    targets: Vec<String>,
    cache: Option<PathBuf>,
    entry_points: bool,
    profile: Option<PathBuf>,
    declared: Option<Vec<String>>,
) -> Result<Impact, Error> {
    let deadline = Instant::now() + DEADLINE;
    let bin = git_bin(deadline);
    // From here, `:/` is the whole tree, so the calls are independent.
    let call = |args: &[&str]| match git(&bin, dir, args, deadline, OUT_BYTES) {
        Ok((out, false)) => Ok(out),
        Ok((_, true)) => Err(Error::Internal(format!(
            "git {}: output past {OUT_BYTES} bytes",
            args[0]
        ))),
        Err(Failure::TimedOut) => Err(Error::Internal(format!("git {}: timed out", args[0]))),
        // Only the `HEAD` calls take a revision: a repo with no commits.
        Err(Failure::BadRevision) => Err(Error::Invalid("entry points: no commit yet".into())),
        Err(Failure::Other(e)) => Err(Error::Invalid(e)),
    };
    let joined = |h: std::thread::ScopedJoinHandle<'_, Result<Vec<u8>, Error>>| {
        h.join()
            .unwrap_or_else(|_| Err(Error::Internal("git thread panicked".into())))
    };
    let (out, origin, head, index, status, staged) = std::thread::scope(|s| {
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
        // Staged changes: `ls-files -s` keys them by the index's blob, whose
        // lines are not `HEAD`'s.
        let index = s.spawn(|| match entry_points {
            true => call(&["diff-index", "--cached", "--name-only", "-z", "HEAD", "--"]),
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
            joined(index),
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
        true => Some(rules_in(&bin, dir, deadline, profile, origin.as_deref())?),
        false => None,
    };
    let head = String::from_utf8_lossy(&head?).trim().to_string();
    let index = index?;

    // One cache per origin, so every worktree and clone of it shares one.
    let given = cache.is_some();
    let cache = match cache {
        Some(c) => Some(c),
        None => {
            // git names the common dir relative to `dir`.
            let origin = origin.or_else(|| {
                std::fs::canonicalize(dir.join(&common))
                    .ok()
                    .map(|p| p.display().to_string())
            });
            dirs::cache_dir()
                .zip(origin)
                .map(|(d, o)| cache_file(&d.join("chaperone").join("graph"), &o))
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
    let entries = match &rules {
        Some(rules) => {
            // A target need not be in the tree (deleted, or not written
            // yet), and is reported all the same.
            tree.extend(targets.iter().cloned());
            let (unmatched, total) = rules
                .check(tree.iter().map(String::as_str))
                .map_err(|e| Error::Invalid(format!("entry points: {e}")))?;
            Some(Entries {
                rules,
                unmatched,
                total,
                head,
                staged: index
                    .split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(|p| String::from_utf8_lossy(p).to_string())
                    .collect(),
                declared,
            })
        }
        None => None,
    };
    Ok(impact(
        &files,
        read,
        ids,
        cache.as_deref(),
        given,
        &targets,
        LIMIT,
        entries,
    ))
}

/// The entry rules of the repo `dir` is in: the file `profile` names, else
/// the base branch's [`REPO_PROFILE`], else [`rules`] by `origin` (a repo
/// that does not carry its profile yet).
pub fn rules_in(
    bin: &Path,
    dir: &Path,
    deadline: Instant,
    profile: Option<PathBuf>,
    origin: Option<&str>,
) -> Result<Rules, Error> {
    if profile.is_none()
        && let Some(found) = base_profile(bin, dir, deadline)?
    {
        let at = found.at();
        return Rules::from_toml(&found.text)
            .map_err(|e| Error::Invalid(format!("profile {at}: {e}")));
    }
    rules(profile, origin)
}

/// The profile's `[tests]` or `[smoke]`: what `chaperone check run tests`
/// or `smoke` runs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tests {
    /// Run in order, each an argv: no shell reads it.
    pub commands: Vec<Vec<String>>,
    /// The variable (`CARGO_TARGET_DIR`) the harness points at the lane's
    /// own build directory outside the checkout, kept between runs.
    pub target_dir: Option<String>,
    /// For the checkout and every command together, 1 to 86400 (default
    /// 1800).
    pub timeout_secs: Option<u64>,
}

/// The `[tests]` or `[smoke]` of a base branch's profile, the sha256 of the profile's
/// text, and the base and the commit it pointed at, so a reader can compare
/// them with the real remote: the lane's own clone holds that ref.
#[derive(Debug)]
pub struct BaseTests {
    pub tests: Tests,
    pub hash: String,
    pub base: &'static str,
    pub commit: String,
}

/// The `[<table>]` (`tests` or `smoke`) of the base branch's
/// [`REPO_PROFILE`] for the repo `dir` is in, never the worktree's copy.
/// No profile, no such table, no command, a timeout out of range or a
/// `target_dir` that is no plain variable name (or is `PATH` or `HOME`)
/// refuses, so "not run" never reads as a pass.
pub fn commands_in(
    bin: &Path,
    dir: &Path,
    table: &str,
    deadline: Instant,
) -> Result<BaseTests, Error> {
    let Some(found) = base_profile(bin, dir, deadline)? else {
        return Err(Error::Invalid(format!(
            "no {REPO_PROFILE} on the base branch ({})",
            BASES.join(", ")
        )));
    };
    let at = found.at();
    let bad = |why: &str| Err(Error::Invalid(format!("profile {at}: {why}")));
    let mut profile: toml::Table = match toml::from_str(&found.text) {
        Ok(p) => p,
        Err(e) => return bad(e.message()),
    };
    let tests = profile.remove(table).map(toml::Value::try_into::<Tests>);
    let tests = match tests.transpose() {
        Ok(t) => t,
        Err(e) => return bad(e.message()),
    };
    let Some(tests) =
        tests.filter(|t| !t.commands.is_empty() && t.commands.iter().all(|c| !c.is_empty()))
    else {
        return bad(&format!("no [{table}] commands (each a non-empty argv)"));
    };
    if tests
        .timeout_secs
        .is_some_and(|t| !(1..=86_400).contains(&t))
    {
        return bad(&format!("[{table}] timeout_secs must be 1 to 86400"));
    }
    let name = |v: &str| {
        v.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
            && v.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !matches!(v, "PATH" | "HOME")
    };
    if tests.target_dir.as_deref().is_some_and(|v| !name(v)) {
        return bad(&format!(
            "[{table}] target_dir must be a variable name, [A-Z_][A-Z0-9_]*, not PATH or HOME"
        ));
    }
    Ok(BaseTests {
        tests,
        hash: format!("{:x}", Sha256::digest(&found.text)),
        base: found.base,
        commit: found.commit,
    })
}

/// Whether the base branch's [`REPO_PROFILE`] for the repo `dir` is in has
/// a `[<table>]`, whatever it holds: a declared table that does not load
/// still counts, so a check that needs its result fails closed. No profile
/// declares none.
pub fn declares(bin: &Path, dir: &Path, table: &str, deadline: Instant) -> Result<bool, Error> {
    let Some(found) = base_profile(bin, dir, deadline)? else {
        return Ok(false);
    };
    match toml::from_str::<toml::Table>(&found.text) {
        Ok(profile) => Ok(profile.contains_key(table)),
        Err(e) => Err(Error::Invalid(format!(
            "profile {}: {}",
            found.at(),
            e.message()
        ))),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    code: Vec<String>,
}

/// The `[diff] code` globs of a base branch's profile (a file matching one
/// is code even under docs or in Markdown), with the profile's hash, base
/// and commit as [`BaseTests`] has them.
#[derive(Debug)]
pub struct BaseCode {
    pub code: Vec<String>,
    pub hash: String,
    pub base: &'static str,
    pub commit: String,
}

/// The [`BaseCode`] of the base branch's [`REPO_PROFILE`] for the repo
/// `dir` is in; None when there is none. No `[diff]` names no globs. A
/// profile that is a link or no file, is not TOML, or whose `[diff]` does
/// not load refuses.
pub fn code_globs(bin: &Path, dir: &Path, deadline: Instant) -> Result<Option<BaseCode>, Error> {
    let Some(found) = base_profile(bin, dir, deadline)? else {
        return Ok(None);
    };
    let bad =
        |e: toml::de::Error| Error::Invalid(format!("profile {}: {}", found.at(), e.message()));
    let mut profile: toml::Table = toml::from_str(&found.text).map_err(bad)?;
    let diff = profile.remove("diff").map(toml::Value::try_into::<Diff>);
    let code = diff
        .transpose()
        .map_err(bad)?
        .map_or_else(Vec::new, |d| d.code);
    Ok(Some(BaseCode {
        code,
        hash: format!("{:x}", Sha256::digest(&found.text)),
        base: found.base,
        commit: found.commit,
    }))
}

/// A profile as a base branch holds it: the base, the commit it pointed
/// at and the text.
struct BaseProfile {
    base: &'static str,
    commit: String,
    text: String,
}

impl BaseProfile {
    fn at(&self) -> String {
        format!("{}:{REPO_PROFILE}", self.base)
    }
}

/// [`REPO_PROFILE`] at the commit the first of [`BASES`] this clone has
/// points at; None when there is no base or it has no such file. The
/// bases are read as remote refs by their full names, so a local branch
/// called `origin/main` is not one, and what the worktree or the index
/// holds at the path changes nothing.
fn base_profile(bin: &Path, dir: &Path, deadline: Instant) -> Result<Option<BaseProfile>, Error> {
    let call = |args: &[&str]| match git(bin, dir, args, deadline, PROFILE_BYTES) {
        Ok((out, false)) => Ok(String::from_utf8_lossy(&out).into_owned()),
        Ok((_, true)) => Err(Error::Invalid(format!(
            "profile: git {} output past {PROFILE_BYTES} bytes",
            args[0]
        ))),
        Err(Failure::TimedOut) => Err(Error::Internal(format!("git {}: timed out", args[0]))),
        Err(Failure::BadRevision) => Err(Error::Invalid(format!(
            "git {}: a commit this clone lacks",
            args[0]
        ))),
        Err(Failure::Other(e)) => Err(Error::Invalid(e)),
    };
    let refs = BASES.map(|base| format!("refs/remotes/{base} "));
    let mut args = vec!["for-each-ref", "--format=%(refname) %(objectname)"];
    args.extend(refs.iter().map(|r| r.trim_end()));
    let have = call(&args)?;
    // The commit is read once and used below, so the profile is the one at
    // the commit recorded even if the ref moves meanwhile.
    let found = BASES.iter().zip(&refs).find_map(|(base, full)| {
        let commit = have.lines().find_map(|l| l.strip_prefix(full.as_str()))?;
        Some((*base, commit.to_string()))
    });
    let Some((base, commit)) = found else {
        return Ok(None);
    };
    // `<mode> <type> <id>\t<path>\0`, or nothing when the base has none.
    let tree = call(&["ls-tree", "-z", &commit, "--", REPO_PROFILE])?;
    let mut cols = tree.split([' ', '\t']);
    let (mode, kind, id) = (cols.next(), cols.next(), cols.next());
    let Some(id) = id else {
        return Ok(None);
    };
    let at = format!("{base}:{REPO_PROFILE}");
    if kind != Some("blob") || mode == Some("120000") {
        return Err(Error::Invalid(format!("profile {at}: not a file")));
    }
    let text = call(&["cat-file", "blob", id])?;
    Ok(Some(BaseProfile { base, commit, text }))
}

/// The pipeline profile of repo `slug` (`owner/repo`), where [`rules`]
/// looks for it when it is not passed.
pub fn profile_file(slug: &str) -> Option<PathBuf> {
    let file = format!("{}.md", slug.replace('/', "-").to_lowercase());
    Some(
        dirs::home_dir()?
            .join("dotfiles/claude/pipeline/profiles")
            .join(file),
    )
}

/// The entry rules: from `profile`, or the origin's pipeline profile, by
/// the origin URL in git config (an `insteadOf` alias is not expanded). No
/// rules is a refusal, never an empty answer.
pub fn rules(profile: Option<PathBuf>, origin: Option<&str>) -> Result<Rules, Error> {
    let refuse = Error::Invalid;
    let (path, slug) = match profile {
        Some(p) => (p, None),
        None => {
            let slug = origin.and_then(slug).ok_or_else(|| {
                refuse(
                    "entry points: no GitHub origin to find the profile by; pass --profile".into(),
                )
            })?;
            let path = profile_file(&slug)
                .ok_or_else(|| refuse("entry points: no home directory".into()))?;
            (path, Some(slug))
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    const PROFILE: &str = "[[entry_points]]\nkind = \"command\"\npath = \"src/main.rs\"\n";

    /// The repo's profile is its base branch's: an edit in the worktree or
    /// a later commit does not change it, a broken one refuses by name,
    /// and a repo that carries none is looked up by its origin.
    #[test]
    fn rules_come_from_the_base_branch() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(wt)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        let commit = |text: &str| {
            std::fs::create_dir_all(wt.join(".chaperone")).unwrap();
            std::fs::write(wt.join(REPO_PROFILE), text).unwrap();
            run(&["add", "-A"]);
            run(&["commit", "-q", "-m", "profile"]);
        };
        let rules = || {
            let deadline = Instant::now() + DEADLINE;
            rules_in(Path::new("git"), wt, deadline, None, None)
        };
        let kinds = |r: Rules| {
            r.at("src/main.rs")
                .map(|(k, _)| k.to_string())
                .collect::<Vec<_>>()
        };
        run(&["init", "-q"]);
        run(&["commit", "-q", "--allow-empty", "-m", "one"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // The lane adds a profile, and a local branch takes the base's
        // name: the base has none, so the origin's is looked for.
        commit(PROFILE);
        run(&["branch", "origin/main"]);
        let Err(Error::Invalid(why)) = rules() else {
            panic!("a profile the base does not have was read");
        };
        assert!(why.contains("no GitHub origin"), "{why}");

        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // The lane rewrites its rules, committed and not: the base's hold.
        commit(&PROFILE.replace("command", "weakened"));
        std::fs::write(wt.join(REPO_PROFILE), "not toml").unwrap();
        assert_eq!(kinds(rules().unwrap()), ["command"]);

        commit("[areas]\n");
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let Err(Error::Invalid(why)) = rules() else {
            panic!("a profile with no entry points answered");
        };
        let named = "profile origin/main:.chaperone/profile.toml: no [[entry_points]]";
        assert_eq!(why, named);

        // A link is not followed: its target would be the lane's to choose.
        std::fs::remove_file(wt.join(REPO_PROFILE)).unwrap();
        std::os::unix::fs::symlink("elsewhere.toml", wt.join(REPO_PROFILE)).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "link"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let Err(Error::Invalid(why)) = rules() else {
            panic!("a link answered");
        };
        assert_eq!(
            why,
            "profile origin/main:.chaperone/profile.toml: not a file"
        );
    }
}
