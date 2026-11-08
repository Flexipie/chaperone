//! Chaperone's code graph: Rust module nodes and import edges, and the
//! files that depend on a change. [`impact`] runs no git and reads no
//! ledger: the caller lists the tree, and the facts live in their own
//! cache (ADR 0043). [`impact_in`] lists it from git: the whole run.

mod cache;
mod entry;
mod parse;
mod resolve;
mod worktree;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub use entry::{Rules, Unmatched};
pub use worktree::{Error, impact_in};

/// Files past this size are not parsed (generated code): their references
/// are unknown, so the answer is a lower bound. A parse holds about 45
/// bytes of tree per byte of source, so this bounds a call's peak RSS.
const MAX_BYTES: usize = 256 * 1024;
/// `unsure` entries listed; the rest are counted.
const UNSURE_ROWS: usize = 20;
/// Files listed per undeclared area, and unmapped files; the rest counted.
const AREA_FILES: usize = 50;

/// One file of the tree: its path from the repo root, and the key of its
/// content when the caller knows it (git's blob id for a clean file).
/// Without one, the file is read and hashed.
pub struct File {
    pub path: String,
    pub key: Option<String>,
}

/// What `impact` needs to list the entry points reached.
pub struct Entries<'a> {
    /// Checked against the tree and the targets by the caller.
    pub rules: &'a Rules,
    /// `Rules::check`'s answer.
    pub unmatched: Vec<Unmatched>,
    /// The commit the lines are read at.
    pub head: String,
    /// Paths whose index differs from `head`: their lines are not `head`'s.
    pub staged: HashSet<String>,
    /// The areas the caller says the targets change, to compare.
    pub declared: Option<Vec<String>>,
}

// Fields in sorted order: the bytes `render_ok` gives.
#[derive(Serialize)]
pub struct Impact {
    /// With `Entries::declared`: those areas against the targets' and the
    /// entry points'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub areas: Option<Areas>,
    /// `exact`, or `lower_bound` when some file's references are unknown
    /// (see `unsure`): every depth, and an empty answer, may miss files.
    pub bound: &'static str,
    pub cache: Cache,
    pub depths: Vec<Depth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_points: Option<EntryPoints>,
    pub files_omitted: usize,
    pub stale: bool,
    pub targets: Vec<Target>,
    pub unsure: Vec<Unsure>,
    pub unsure_omitted: usize,
}

#[derive(Serialize)]
pub struct Cache {
    pub parsed: usize,
    pub reused: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

#[derive(Serialize)]
pub struct Depth {
    /// `exact`, or `lower_bound` when some file's references are unknown
    /// (see `unsure`): more files may belong here.
    pub bound: &'static str,
    pub depth: usize,
    pub files: Vec<Node>,
    pub files_omitted: usize,
}

#[derive(Serialize)]
pub struct Node {
    pub module: Option<String>,
    pub path: String,
}

#[derive(Serialize)]
pub struct EntryPoints {
    /// As `Impact::bound`, and `lower_bound` when a rule is `unmatched`:
    /// more may be reached.
    pub bound: &'static str,
    /// The commit the answer's lines are read at, less `dirty` hops.
    pub head: String,
    pub omitted: usize,
    /// Nearest first.
    pub reached: Vec<Entry>,
    pub unmatched: Vec<Unmatched>,
}

/// Declared areas against the targets'. Every list is sorted.
#[derive(Serialize)]
pub struct Areas {
    /// As `EntryPoints::bound`: when `lower_bound`, `reached` may miss
    /// areas, so `reached_undeclared` may too and `unused` may list more.
    pub bound: &'static str,
    /// Areas a target is in.
    pub changed: Vec<String>,
    /// Areas of the entry points reached.
    pub reached: Vec<String>,
    /// Reached, not declared and not changed: what a shared file reaches.
    pub reached_undeclared: Vec<String>,
    /// Changed and not declared, with the targets in each.
    pub undeclared: Vec<AreaFiles>,
    /// Declared names the profile does not have.
    pub unknown: Vec<String>,
    /// Targets in no area: an undeclared change no area names.
    pub unmapped: Vec<String>,
    pub unmapped_omitted: usize,
    /// Declared, and neither changed nor reached.
    pub unused: Vec<String>,
}

#[derive(Serialize)]
pub struct AreaFiles {
    pub area: String,
    pub files: Vec<String>,
    pub files_omitted: usize,
}

#[derive(Serialize)]
pub struct Entry {
    pub area: String,
    pub depth: usize,
    /// From the entry point's file to a target: each hop's file, and
    /// where it names the next one.
    pub evidence: Vec<Hop>,
    pub kind: String,
    pub name: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct Hop {
    /// `file:line` of the reference to the next hop; none for the target,
    /// and for an edge Cargo makes (integration tests run the binaries).
    pub at: Option<String>,
    /// `at` is read from the worktree or the index, not `head`.
    pub dirty: bool,
    pub path: String,
}

#[derive(Serialize)]
pub struct Target {
    /// In the tree.
    pub known: bool,
    /// `rust`, or none: no edges are read for other files yet.
    pub lang: Option<&'static str>,
    pub module: Option<String>,
    pub path: String,
}

#[derive(Serialize)]
pub struct Unsure {
    pub path: String,
    pub why: String,
}

/// The cache file for one origin in `dir`: a hash of its URL, so every
/// worktree and clone of it shares one.
pub fn cache_file(dir: &Path, origin: &str) -> PathBuf {
    let hash = format!("{:x}", Sha256::digest(origin.as_bytes()));
    dir.join(format!("{}.db", &hash[..16]))
}

/// The files that depend on `targets`, nearest first, listing at most
/// `limit` of them. `read` gives a file's bytes by path; `ids` the key of
/// those bytes as `File::key` names it (`None`: unknown); `cache` is the
/// facts file, `None` to parse everything, and `cache_given` whether it
/// was passed by hand (an empty one is then refused, not used);
/// `entries` to list the entry points reached.
#[allow(clippy::too_many_arguments)]
pub fn impact(
    files: &[File],
    read: impl Fn(&str) -> io::Result<Vec<u8>>,
    ids: impl Fn(&[&str]) -> Vec<Option<String>>,
    cache: Option<&Path>,
    cache_given: bool,
    targets: &[String],
    limit: usize,
    entries: Option<Entries>,
) -> Impact {
    let rust: Vec<&File> = files.iter().filter(|f| f.path.ends_with(".rs")).collect();
    let mut store = cache::Cache::open(cache, cache_given);

    // Keys: given, or the hash of the bytes read now (kept to parse).
    let mut bytes: Vec<Option<Vec<u8>>> = Vec::with_capacity(rust.len());
    let mut keys: Vec<Option<String>> = Vec::with_capacity(rust.len());
    for f in &rust {
        match &f.key {
            Some(k) => {
                keys.push(Some(k.clone()));
                bytes.push(None);
            }
            None => match read(&f.path) {
                Ok(b) => {
                    keys.push(Some(format!("sha256:{:x}", Sha256::digest(&b))));
                    bytes.push(Some(b));
                }
                Err(_) => {
                    keys.push(None);
                    bytes.push(None);
                }
            },
        }
    }
    let live: Vec<&str> = keys.iter().flatten().map(String::as_str).collect();
    let found = store.get(&live);
    // A given key on a miss: the bytes read may not be the ones it names
    // (an edit since, `skip-worktree`), so they are keyed by their own hash
    // unless `ids` says they match. Never wrong facts under a shared key.
    let mut check = Vec::new();
    for (i, f) in rust.iter().enumerate() {
        if f.key.is_some() && keys[i].as_ref().is_some_and(|k| !found.contains_key(k)) {
            match read(&f.path) {
                // Never parsed, so its key does not matter.
                Ok(b) if b.len() > MAX_BYTES => bytes[i] = Some(b),
                Ok(b) => {
                    bytes[i] = Some(b);
                    check.push(i);
                }
                Err(_) => keys[i] = None,
            }
        }
    }
    let paths: Vec<&str> = check.iter().map(|&i| rust[i].path.as_str()).collect();
    let got = ids(&paths);
    for (j, &i) in check.iter().enumerate() {
        if got.get(j).and_then(Option::as_ref) != keys[i].as_ref() {
            let b = bytes[i].as_deref().unwrap_or_default();
            keys[i] = Some(format!("sha256:{:x}", Sha256::digest(b)));
        }
    }
    let (mut parsed, mut reused) = (0, 0);
    let mut new = Vec::new();
    let mut parser = parse::parser();
    let mut facts = Vec::with_capacity(rust.len());
    for (i, f) in rust.iter().enumerate() {
        let Some(key) = &keys[i] else {
            facts.push(None);
            continue;
        };
        if let Some(cached) = found.get(key).cloned() {
            reused += 1;
            facts.push(Some(cached));
            continue;
        }
        let b = match bytes[i].take() {
            Some(b) => Ok(b),
            None => read(&f.path),
        };
        match b {
            Ok(b) if b.len() <= MAX_BYTES => {
                let fresh = parse::facts(&mut parser, &b);
                parsed += 1;
                if let Ok(json) = serde_json::to_vec(&fresh) {
                    new.push((key.clone(), json));
                }
                facts.push(Some(fresh));
            }
            _ => facts.push(None),
        }
    }
    let live: Vec<&str> = keys.iter().flatten().map(String::as_str).collect();
    store.put(&new, &live);

    let manifests: Vec<(String, String)> = files
        .iter()
        .filter_map(|f| {
            let dir = match f.path.as_str() {
                "Cargo.toml" => "",
                p => p.strip_suffix("/Cargo.toml")?,
            };
            let text = String::from_utf8(read(&f.path).ok()?).ok()?;
            Some((dir.to_string(), text))
        })
        .collect();
    let paths: Vec<String> = rust.iter().map(|f| f.path.clone()).collect();
    let graph = resolve::build(&paths, &facts, &manifests);

    let mut users: Vec<Vec<usize>> = vec![Vec::new(); paths.len()];
    for (file, deps) in graph.deps.iter().enumerate() {
        for &d in deps {
            users[d].push(file);
        }
    }
    // Each file reached: its depth, and the file it was reached through.
    let mut from: HashMap<usize, (usize, Option<usize>)> = HashMap::new();
    let mut frontier = Vec::new();
    let targets: Vec<Target> = targets
        .iter()
        .map(|t| {
            let i = paths.iter().position(|p| p == t);
            if let Some(i) = i
                && from.insert(i, (0, None)).is_none()
            {
                frontier.push(i);
            }
            Target {
                known: i.is_some() || files.iter().any(|f| &f.path == t),
                lang: i.map(|_| "rust"),
                module: i.and_then(|i| graph.module[i].clone()),
                path: t.clone(),
            }
        })
        .collect();
    // A target whose edges are not read (not a Rust file in the tree) has
    // dependents this answer cannot see.
    let bound = if graph.unsure.is_empty() && targets.iter().all(|t| t.lang.is_some()) {
        "exact"
    } else {
        "lower_bound"
    };
    let mut depths = Vec::new();
    let mut listed = 0;
    let mut files_omitted = 0;
    while !frontier.is_empty() {
        let depth = depths.len() + 1;
        let mut next: Vec<usize> = Vec::new();
        for &f in &frontier {
            for &u in &users[f] {
                if let std::collections::hash_map::Entry::Vacant(e) = from.entry(u) {
                    e.insert((depth, Some(f)));
                    next.push(u);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort_by(|a, b| paths[*a].cmp(&paths[*b]));
        let keep = next.len().min(limit - listed.min(limit));
        listed += keep;
        files_omitted += next.len() - keep;
        depths.push(Depth {
            bound,
            depth: depths.len() + 1,
            files: next[..keep]
                .iter()
                .map(|&i| Node {
                    module: graph.module[i].clone(),
                    path: paths[i].clone(),
                })
                .collect(),
            files_omitted: next.len() - keep,
        });
        frontier = next;
    }

    let mut areas = None;
    let entry_points = entries.map(|e| {
        let rules = e.rules;
        // Read from the worktree (a changed file, or one not matching its
        // blob), or from an index that differs from `head`.
        let dirty = |i: usize| {
            keys[i].as_deref().is_none_or(|k| k.starts_with("sha256:"))
                || e.staged.contains(&paths[i])
        };
        let mut reached = Vec::new();
        let mut add = |path: &str, depth: usize, evidence: &dyn Fn() -> Vec<Hop>| {
            for (kind, name) in rules.at(path) {
                reached.push(Entry {
                    area: rules.area(path).to_string(),
                    depth,
                    evidence: evidence(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    path: path.to_string(),
                });
            }
        };
        for (&i, &(depth, _)) in &from {
            add(&paths[i], depth, &|| {
                let mut hops = Vec::new();
                let mut at = i;
                while let Some(&(_, Some(next))) = from.get(&at) {
                    let w = graph.at.get(&(at, next));
                    hops.push(Hop {
                        at: w.map(|(f, line)| format!("{}:{line}", paths[*f])),
                        dirty: w.is_some_and(|(f, _)| dirty(*f)),
                        path: paths[at].clone(),
                    });
                    at = next;
                }
                hops.push(Hop {
                    at: None,
                    dirty: false,
                    path: paths[at].clone(),
                });
                hops
            });
        }
        // Not a Rust file: only the target itself.
        let mut once = HashSet::new();
        for t in targets
            .iter()
            .filter(|t| t.lang.is_none() && once.insert(&t.path))
        {
            add(&t.path, 0, &|| {
                vec![Hop {
                    at: None,
                    dirty: false,
                    path: t.path.clone(),
                }]
            });
        }
        reached.sort_by(|a, b| {
            (a.depth, &a.path, &a.kind, &a.name).cmp(&(b.depth, &b.path, &b.kind, &b.name))
        });
        let bound = match e.unmatched.is_empty() {
            true => bound,
            false => "lower_bound",
        };
        // Before the trim: every entry point reached counts.
        if let Some(declared) = &e.declared {
            areas = Some(compare(rules, declared, &targets, &reached, bound));
        }
        let omitted = reached.len().saturating_sub(limit);
        reached.truncate(limit);
        EntryPoints {
            bound,
            head: e.head,
            omitted,
            reached,
            unmatched: e.unmatched,
        }
    });

    let stale = facts.iter().any(Option::is_none);
    let unsure_omitted = graph.unsure.len().saturating_sub(UNSURE_ROWS);
    Impact {
        areas,
        bound,
        cache: Cache {
            parsed,
            reused,
            skipped: store.skipped,
        },
        depths,
        entry_points,
        files_omitted,
        stale,
        targets,
        unsure: graph
            .unsure
            .into_iter()
            .take(UNSURE_ROWS)
            .map(|(i, why)| Unsure {
                path: paths[i].clone(),
                why,
            })
            .collect(),
        unsure_omitted,
    }
}

/// `declared` against the areas the targets are in and the entry points
/// reached are in.
fn compare(
    rules: &Rules,
    declared: &[String],
    targets: &[Target],
    reached: &[Entry],
    bound: &'static str,
) -> Areas {
    let declared: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    let mut changed: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut unmapped = BTreeSet::new();
    for t in targets {
        let mut any = false;
        for a in rules.areas(&t.path) {
            changed.entry(a).or_default().push(&t.path);
            any = true;
        }
        if !any {
            unmapped.insert(t.path.as_str());
        }
    }
    let reached: BTreeSet<&str> = reached.iter().map(|e| e.area.as_str()).collect();
    let list = |s: &mut dyn Iterator<Item = &str>| s.map(String::from).collect();
    Areas {
        bound,
        changed: list(&mut changed.keys().copied()),
        reached: list(&mut reached.iter().copied()),
        reached_undeclared: list(
            &mut reached
                .iter()
                .copied()
                .filter(|a| !declared.contains(a) && !changed.contains_key(a)),
        ),
        undeclared: changed
            .iter()
            .filter(|(a, _)| !declared.contains(*a))
            .map(|(a, files)| {
                let mut files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
                files.sort();
                files.dedup();
                let files_omitted = files.len().saturating_sub(AREA_FILES);
                files.truncate(AREA_FILES);
                AreaFiles {
                    area: a.to_string(),
                    files,
                    files_omitted,
                }
            })
            .collect(),
        unknown: list(&mut declared.iter().copied().filter(|a| !rules.has_area(a))),
        unmapped_omitted: unmapped.len().saturating_sub(AREA_FILES),
        unmapped: list(&mut unmapped.iter().copied().take(AREA_FILES)),
        unused: list(
            &mut declared
                .iter()
                .copied()
                .filter(|a| rules.has_area(a) && !changed.contains_key(a) && !reached.contains(a)),
        ),
    }
}
