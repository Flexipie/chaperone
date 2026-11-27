//! The base branch's profile, read from git: what `check run tests` and
//! `smoke`, `pr open` and the diff check keep to. Never the worktree's copy.

use std::path::Path;
use std::time::Instant;

use chaperone_core::git::{BASES, Failure, git};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The repo's own profile: TOML, read from its base branch's remote ref and
/// never from the worktree, so an edit in a lane's change is not used. The
/// ref lives in the lane's clone, so this is a speed bump, not a guarantee.
pub const REPO_PROFILE: &str = ".chaperone/profile.toml";
/// A profile past this is refused, not cut.
const PROFILE_BYTES: usize = 1024 * 1024;

/// Why a profile read gave no answer: the caller's input (the CLI's
/// `invalid_input`), or this side's failure (`internal`).
#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Internal(String),
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

/// The `[pr]` rules of a base branch's profile that `chaperone pr open`
/// keeps to. Its other keys (`flow`, `create`, `address`, `merge`,
/// `stack`) are accepted and not read.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrRules {
    /// The title template: `{type}` one of `types`, `{key}` the lane's
    /// tracker key, `{text}` the lane's words, the rest literal.
    pub title: Option<String>,
    pub types: Vec<String>,
    /// The body's first lines, `{key}` filled.
    pub head: Vec<String>,
    /// Set when the PR is first recorded.
    pub assignee: Option<String>,
    /// Requested at the lane's ready handoff.
    pub reviewers: Vec<String>,
}

impl PrRules {
    /// Whether `title` fills the template with `key`, one of `types` and a
    /// `{text}` that is not empty and has no space at either end. With no
    /// template, any title fits.
    pub fn fits(&self, title: &str, key: &str) -> bool {
        let Some(template) = &self.title else {
            return true;
        };
        let Some((pre, post)) = template.split_once("{text}") else {
            return false;
        };
        let none = [String::new()];
        let types = match template.contains("{type}") {
            true => &self.types[..],
            false => &none[..],
        };
        let fill = |s: &str, t: &str| s.replace("{type}", t).replace("{key}", key);
        types.iter().any(|t| {
            let text = title.strip_prefix(&fill(pre, t));
            let text = text.and_then(|r| r.strip_suffix(&fill(post, t)));
            text.is_some_and(|text| !text.is_empty() && text.trim() == text)
        })
    }

    /// The `head` lines with `{key}` filled, then a blank line; empty when
    /// there are none.
    pub fn head(&self, key: &str) -> String {
        let head: String = self
            .head
            .iter()
            .map(|l| l.replace("{key}", key) + "\n")
            .collect();
        if head.is_empty() { head } else { head + "\n" }
    }
}

/// The [`PrRules`] of the base branch's [`REPO_PROFILE`] for the repo `dir`
/// is in, never the worktree's copy; none when there is no profile or no
/// `[pr]`. A profile that is not TOML, a `[pr]` that does not load, or a
/// `title` without exactly one `{text}`, with a second `{type}` or `{key}`,
/// another brace, or `{type}` and no `types` refuses.
pub fn pr_rules(bin: &Path, dir: &Path, deadline: Instant) -> Result<PrRules, Error> {
    let Some(found) = base_profile(bin, dir, deadline)? else {
        return Ok(PrRules::default());
    };
    let bad = |why: &str| Error::Invalid(format!("profile {}: {why}", found.at()));
    let mut profile: toml::Table = toml::from_str(&found.text).map_err(|e| bad(e.message()))?;
    let Some(pr) = profile.remove("pr") else {
        return Ok(PrRules::default());
    };
    let mut pr: toml::Table = pr.try_into().map_err(|e| bad(e.message()))?;
    for key in ["flow", "create", "address", "merge", "stack"] {
        pr.remove(key);
    }
    let rules: PrRules = toml::Value::Table(pr)
        .try_into()
        .map_err(|e| bad(e.message()))?;
    if let Some(t) = &rules.title {
        let rest = t.replacen("{text}", "", 1).replacen("{type}", "", 1);
        if t.matches("{text}").count() != 1
            || rest.replacen("{key}", "", 1).contains(['{', '}'])
            || (t.contains("{type}") && rules.types.is_empty())
        {
            return Err(bad(&format!(
                "[pr] title {t:?} needs one {{text}}, at most one {{type}} and one {{key}}, no \
                 other brace, and `types` when it names {{type}}"
            )));
        }
    }
    Ok(rules)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    code: Vec<String>,
}

/// `[size]`, its own table so a binary that predates it still loads the
/// profile: the globs of test files, whose lines the size cap leaves out.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Size {
    tests: Vec<String>,
}

/// `[assets]`, a table of its own as `[size]` is: the globs of asset files,
/// those with no line counts that a size gate does not refuse.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assets {
    globs: Vec<String>,
}

/// A `[[companions]]` rule (its own table, so a binary that predates it
/// still loads `[diff]`): a diff that changes a file `when` names
/// must change one `needs` names too.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    pub name: String,
    pub when: Vec<String>,
    pub needs: Vec<String>,
}

/// A `[[review.required]]` lens: a review of a change to a file `when`
/// names is not complete until a lens has asked `prompt` of it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Required {
    pub name: String,
    pub when: Vec<String>,
    pub prompt: String,
}

/// The `[diff] code` globs of a base branch's profile (a file matching one
/// is code even under docs or in Markdown) and its companion rules, with
/// the profile's hash, base and commit as [`BaseTests`] has them.
#[derive(Debug)]
pub struct BaseCode {
    pub code: Vec<String>,
    /// `[size] tests`: code files counted as test lines.
    pub tests: Vec<String>,
    /// `[assets] globs`: binary files the size gate lets through.
    pub assets: Vec<String>,
    pub companions: Vec<Companion>,
    /// `[[review.required]]`: the lenses a review of the files they name needs.
    pub required: Vec<Required>,
    pub hash: String,
    pub base: &'static str,
    pub commit: String,
}

/// The [`BaseCode`] of the base branch's [`REPO_PROFILE`] for the repo
/// `dir` is in; None when there is none. No `[diff]` names no globs. A
/// profile that is a link or no file, is not TOML, or whose `[diff]`, `[size]`,
/// `[assets]`, `[[companions]]` or `[[review.required]]` does not load refuses, as does a rule
/// with an empty list or a name that is empty or holds a space or `:` (a trailer
/// names a companion rule, a lens row a required lens), or a blank prompt.
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
    let size = profile.remove("size").map(toml::Value::try_into::<Size>);
    let tests = size
        .transpose()
        .map_err(bad)?
        .map_or_else(Vec::new, |s| s.tests);
    let assets = profile
        .remove("assets")
        .map(toml::Value::try_into::<Assets>);
    let assets = assets
        .transpose()
        .map_err(bad)?
        .map_or_else(Vec::new, |a| a.globs);
    let rules = profile
        .remove("companions")
        .map(toml::Value::try_into::<Vec<Companion>>);
    let companions = rules.transpose().map_err(bad)?.unwrap_or_default();
    let named = |n: &str| !n.is_empty() && !n.contains(|c: char| c.is_whitespace() || c == ':');
    if let Some(c) = companions
        .iter()
        .find(|c| !named(&c.name) || c.when.is_empty() || c.needs.is_empty())
    {
        return Err(Error::Invalid(format!(
            "profile {}: [[companions]] {:?} needs a name without spaces or `:`, and \
             non-empty `when` and `needs`",
            found.at(),
            c.name
        )));
    }
    let required = profile
        .get("review")
        .and_then(|r| r.get("required"))
        .cloned();
    let required = required.map(toml::Value::try_into::<Vec<Required>>);
    let required = required.transpose().map_err(bad)?.unwrap_or_default();
    if let Some(r) = required
        .iter()
        .find(|r| !named(&r.name) || r.when.is_empty() || r.prompt.trim().is_empty())
    {
        return Err(Error::Invalid(format!(
            "profile {}: [[review.required]] {:?} needs a name without spaces or `:`, a \
             non-empty `when` and a prompt",
            found.at(),
            r.name
        )));
    }
    Ok(Some(BaseCode {
        code,
        tests,
        assets,
        companions,
        required,
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

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Duration;

    use super::*;

    const PROFILE: &str = "[tests]\ncommands = [[\"command\"]]\n";

    /// The repo's profile is its base branch's: an edit in the worktree or
    /// a later commit does not change it, and a broken one or a link
    /// refuses by name.
    #[test]
    fn profile_comes_from_the_base_branch() {
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
            let deadline = Instant::now() + Duration::from_secs(5);
            commands_in(Path::new("git"), wt, "tests", deadline)
        };
        let kinds = |r: BaseTests| r.tests.commands[0].clone();
        run(&["init", "-q"]);
        run(&["commit", "-q", "--allow-empty", "-m", "one"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // The lane adds a profile, and a local branch takes the base's
        // name: the base has none.
        commit(PROFILE);
        run(&["branch", "origin/main"]);
        let Err(Error::Invalid(why)) = rules() else {
            panic!("a profile the base does not have was read");
        };
        assert!(why.starts_with("no .chaperone/profile.toml"), "{why}");

        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // The lane rewrites its rules, committed and not: the base's hold.
        commit(&PROFILE.replace("command", "weakened"));
        std::fs::write(wt.join(REPO_PROFILE), "not toml").unwrap();
        assert_eq!(kinds(rules().unwrap()), ["command"]);

        commit("[areas]\n");
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let Err(Error::Invalid(why)) = rules() else {
            panic!("a profile with no [tests] answered");
        };
        let named = "profile origin/main:.chaperone/profile.toml: no [tests] commands (each a non-empty argv)";
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
