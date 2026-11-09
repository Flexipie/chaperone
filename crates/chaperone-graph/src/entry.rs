//! Entry points: the commands, routes and screens a repo names in its
//! pipeline profile, each in exactly one area. `impact` finds the ones a
//! change reaches.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Errors listed before the rest are counted.
const ERRORS: usize = 10;

/// The profile's `[[entry_points]]` and `[areas]`; its other keys are the
/// pipeline's and not read here.
#[derive(Deserialize)]
pub struct Rules {
    pub slug: Option<String>,
    #[serde(default)]
    entry_points: Vec<Rule>,
    #[serde(default)]
    areas: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    /// `command`, `route`, `screen`: the repo's word.
    kind: String,
    /// A glob from the repo root: `*` within a segment, `**` any segments.
    path: String,
    /// Default: the file's path.
    name: Option<String>,
}

/// A rule that names no file: a typo, or a file renamed or deleted since
/// the profile was written. Listed, not refused, so one lane's rename does
/// not break the call for every other lane.
#[derive(Serialize)]
pub struct Unmatched {
    pub kind: String,
    pub name: Option<String>,
    pub path: String,
}

impl Rules {
    /// The rules in a profile's `+++` TOML front matter. One without
    /// `[[entry_points]]` is refused: an empty answer would look complete.
    pub fn from_profile(text: &str) -> Result<Rules, String> {
        let toml = text
            .strip_prefix("+++\n")
            .and_then(|t| t.split_once("\n+++"))
            .map(|(t, _)| t)
            .ok_or("no +++ TOML front matter")?;
        let rules: Rules = toml::from_str(toml).map_err(|e| e.message().to_string())?;
        if rules.entry_points.is_empty() {
            return Err("no [[entry_points]] in the front matter".into());
        }
        Ok(rules)
    }

    /// The entry points at `path`: (kind, name).
    pub fn at<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.entry_points
            .iter()
            .filter(move |r| glob(&r.path, path))
            .map(move |r| (r.kind.as_str(), r.name.as_deref().unwrap_or(path)))
    }

    /// The one area `path` is in; `check` has refused a tree or target
    /// where an entry point is in none or several.
    pub fn area<'a>(&'a self, path: &'a str) -> &'a str {
        self.areas(path).next().unwrap_or_default()
    }

    /// Every area `path` is in; only an entry point's file must be in one.
    pub fn areas<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a str> {
        self.areas
            .iter()
            .filter(move |(_, globs)| globs.iter().any(|g| glob(g, path)))
            .map(|(name, _)| name.as_str())
    }

    pub(crate) fn has_area(&self, name: &str) -> bool {
        self.areas.contains_key(name)
    }

    /// Against every file of the tree and every target: each entry point is
    /// in exactly one area, never "other". The rules that name none of them
    /// are returned (a typo would otherwise reach nothing, quietly), and the
    /// entry points the tree has: one per file a rule names.
    pub fn check<'a>(
        &self,
        tree: impl Iterator<Item = &'a str>,
    ) -> Result<(Vec<Unmatched>, usize), String> {
        let mut errors = Vec::new();
        let mut used = vec![false; self.entry_points.len()];
        let mut total = 0;
        for path in tree {
            let mut hit = false;
            for (i, r) in self.entry_points.iter().enumerate() {
                if glob(&r.path, path) {
                    used[i] = true;
                    hit = true;
                    total += 1;
                }
            }
            if !hit {
                continue;
            }
            let areas: Vec<&str> = self.areas(path).collect();
            match areas.len() {
                0 => errors.push(format!("{path}: an entry point in no area")),
                1 => {}
                _ => errors.push(format!("{path}: in areas {}", areas.join(", "))),
            }
        }
        if errors.is_empty() {
            let unmatched = self
                .entry_points
                .iter()
                .zip(&used)
                .filter(|(_, u)| !**u)
                .map(|(r, _)| Unmatched {
                    kind: r.kind.clone(),
                    name: r.name.clone(),
                    path: r.path.clone(),
                })
                .collect();
            return Ok((unmatched, total));
        }
        // The tree comes in no order: the same refusal every run.
        errors.sort();
        let more = errors.len().saturating_sub(ERRORS);
        errors.truncate(ERRORS);
        if more > 0 {
            errors.push(format!("{more} more"));
        }
        Err(errors.join("; "))
    }
}

/// `pattern` names `path`: `*` is any run within a segment, a `**`
/// segment any number of segments.
fn glob(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segs(&p, &s)
}

fn segs(p: &[&str], s: &[&str]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((&"**", rest)) => (0..=s.len()).any(|i| segs(rest, &s[i..])),
        Some((first, rest)) => s
            .split_first()
            .is_some_and(|(head, tail)| seg(first, head) && segs(rest, tail)),
    }
}

fn seg(p: &str, s: &str) -> bool {
    match p.split_once('*') {
        None => p == s,
        Some((head, tail)) => s.strip_prefix(head).is_some_and(|rest| {
            (0..=rest.len())
                .filter(|&i| rest.is_char_boundary(i))
                .any(|i| seg(tail, &rest[i..]))
        }),
    }
}
