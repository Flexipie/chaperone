//! Module nodes and import edges for the Rust crates of one tree, built in
//! memory from each file's facts and the Cargo manifests.

use std::collections::{HashMap, HashSet};

use crate::parse::{Facts, Ref};

/// One crate target: a lib, a bin, an integration test, an example, a bench
/// or a build script.
struct Krate {
    /// The crate's name with `-` as `_`: `[lib] name` or the package's for
    /// its lib, the package's for `src/main.rs`, else the target's.
    name: String,
    root: usize,
    lib: bool,
    package: usize,
    test: bool,
}

pub struct Graph {
    /// `deps[f]`: the files `f` depends on.
    pub deps: Vec<HashSet<usize>>,
    /// Each file's module path (`chaperone_core::ledger::write`), when a
    /// crate root reaches it.
    pub module: Vec<Option<String>>,
    /// Files whose references are not all known: (file, why).
    pub unsure: Vec<(usize, String)>,
}

/// `paths[i]` with `facts[i]` (`None`: not read); `manifests`: each
/// package's directory and its `Cargo.toml` text.
pub fn build(paths: &[String], facts: &[Option<Facts>], manifests: &[(String, String)]) -> Graph {
    let index: HashMap<&str, usize> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    let krates = krates(&index, manifests);
    let n = paths.len();
    let mut g = Graph {
        deps: vec![HashSet::new(); n],
        module: vec![None; n],
        unsure: Vec::new(),
    };

    // Module map: from each root, follow `mod name;` to its file.
    let mut mods: HashMap<(usize, Vec<String>), usize> = HashMap::new();
    let mut owners: Vec<Vec<(usize, Vec<String>)>> = vec![Vec::new(); n];
    for (k, krate) in krates.iter().enumerate() {
        let mut stack = vec![(krate.root, Vec::new())];
        while let Some((file, path)) = stack.pop() {
            if mods.contains_key(&(k, path.clone())) {
                continue;
            }
            mods.insert((k, path.clone()), file);
            if owners[file].is_empty() {
                let mut name = vec![krate.name.clone()];
                name.extend(path.iter().cloned());
                g.module[file] = Some(name.join("::"));
            }
            owners[file].push((k, path.clone()));
            let Some(f) = &facts[file] else { continue };
            let dir = child_dir(&paths[file], file == krate.root);
            for (scope, name) in &f.mods {
                let base = [dir.as_str()]
                    .into_iter()
                    .chain(scope.iter().map(String::as_str))
                    .chain([name.as_str()])
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("/");
                let found = [format!("{base}.rs"), format!("{base}/mod.rs")]
                    .into_iter()
                    .find_map(|c| index.get(c.as_str()).copied());
                if let Some(child) = found {
                    let mut p = path.clone();
                    p.extend(scope.iter().cloned());
                    p.push(name.clone());
                    stack.push((child, p));
                }
            }
        }
    }
    // A package's integration tests run its binaries.
    for t in krates.iter().filter(|k| k.test) {
        for b in krates
            .iter()
            .filter(|b| b.package == t.package && !b.lib && !b.test)
        {
            g.deps[t.root].insert(b.root);
        }
    }
    let libs: HashMap<&str, usize> = krates
        .iter()
        .enumerate()
        .filter(|(_, k)| k.lib)
        .map(|(i, k)| (k.name.as_str(), i))
        .collect();
    // `macro_rules!` is in scope by name across its crate; `#[macro_export]`
    // puts it at the crate root, where `crate::name!` and `other::name!` land.
    let mut defs: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
    for file in 0..n {
        let Some(f) = &facts[file] else { continue };
        for (k, _) in &owners[file] {
            for (name, export) in &f.macros {
                defs.entry((*k, name)).or_default().push(file);
                if *export {
                    g.deps[krates[*k].root].insert(file);
                }
            }
        }
    }

    for file in 0..n {
        let Some(f) = &facts[file] else {
            g.unsure.push((file, "unparsed".into()));
            continue;
        };
        if let Some(why) = &f.inexact {
            g.unsure.push((file, why.clone()));
        }
        if owners[file].is_empty() {
            g.unsure.push((file, "orphan".into()));
            continue;
        }
        for (k, path) in &owners[file] {
            let cx = Cx {
                mods: &mods,
                libs: &libs,
                k: *k,
                path,
                facts: f,
            };
            for name in &f.bangs {
                if let Some(d) = defs.get(&(*k, name.as_str())) {
                    g.deps[file].extend(d);
                }
            }
            let mut past_root = false;
            for r in &f.refs {
                match cx.resolve(r, 0) {
                    Res::Module(m) => {
                        g.deps[file].insert(mods[&m]);
                    }
                    Res::PastRoot => past_root = true,
                    Res::External => {}
                }
            }
            // `impl T` here adds to T: T's module depends on this file.
            for r in &f.impls {
                let mut targets = Vec::new();
                match (r.path.len(), cx.alias(&r.path[0])) {
                    (1, Some(_)) | (2.., _) => {
                        if let Res::Module(m) = cx.resolve(r, 0) {
                            targets.push(m);
                        }
                    }
                    // Defined here, or brought in by a glob.
                    _ => targets.extend(cx.globs()),
                }
                for m in targets {
                    let t = mods[&m];
                    g.deps[t].insert(file);
                    // `pub use foo::T;` there: callers that name `foo::T`
                    // need this file too.
                    let Some(tf) = &facts[t] else { continue };
                    let there = Cx {
                        k: m.0,
                        path: &m.1,
                        facts: tf,
                        ..cx
                    };
                    let name = &r.path[r.path.len() - 1];
                    if let Some(Res::Module(d)) = there.alias(name).map(|a| there.resolve(a, 0)) {
                        g.deps[mods[&d]].insert(file);
                    }
                }
            }
            if past_root {
                g.unsure.push((file, "super_past_root".into()));
            }
        }
        g.deps[file].remove(&file);
    }
    g
}

fn krates(index: &HashMap<&str, usize>, manifests: &[(String, String)]) -> Vec<Krate> {
    let mut out = Vec::new();
    for (package, (dir, text)) in manifests.iter().enumerate() {
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let Some(name) = table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        else {
            continue;
        };
        let at = |p: &str| match dir.as_str() {
            "" => p.to_string(),
            d => format!("{d}/{p}"),
        };
        let path_of = |t: &toml::Value| t.get("path").and_then(|p| p.as_str()).map(at);
        let lib_target = table.get("lib");
        let lib = lib_target
            .and_then(path_of)
            .unwrap_or_else(|| at("src/lib.rs"));
        let lib_name = lib_target
            .and_then(|l| l.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(name);
        let stem = |p: &str| {
            p.rsplit('/')
                .next()
                .unwrap_or(p)
                .trim_end_matches(".rs")
                .to_string()
        };
        let mut bins: Vec<(String, String)> = table
            .get("bin")
            .and_then(|b| b.as_array())
            .map(|b| {
                b.iter()
                    .filter_map(|t| Some((t.get("name")?.as_str()?.to_string(), path_of(t)?)))
                    .collect()
            })
            .unwrap_or_default();
        bins.push((name.to_string(), at("src/main.rs")));
        bins.push(("build_script".into(), at("build.rs")));
        let mut add = |name: &str, path: &str, lib: bool, test: bool| {
            if let Some(&root) = index.get(path)
                && !out.iter().any(|k: &Krate| k.root == root)
            {
                out.push(Krate {
                    name: name.replace('-', "_"),
                    root,
                    lib,
                    package,
                    test,
                });
            }
        };
        add(lib_name, &lib, true, false);
        for (name, path) in &bins {
            add(name, path, false, false);
        }
        let prefixes = [
            at("src/bin/"),
            at("tests/"),
            at("examples/"),
            at("benches/"),
        ];
        for (path, _) in index.iter() {
            for (i, prefix) in prefixes.iter().enumerate() {
                let Some(rest) = path.strip_prefix(prefix.as_str()) else {
                    continue;
                };
                let rest = rest.strip_suffix("/main.rs").unwrap_or(rest);
                if !rest.contains('/') && (rest.ends_with(".rs") || path.ends_with("/main.rs")) {
                    add(&stem(rest), path, false, i == 1);
                }
            }
        }
    }
    out
}

/// Where `mod name;` in `path` looks: a crate root or `mod.rs` in its own
/// directory, any other file in the directory named after it.
fn child_dir(path: &str, root: bool) -> String {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    if root || file == "mod.rs" {
        dir.to_string()
    } else {
        let stem = file.strip_suffix(".rs").unwrap_or(file);
        [dir, stem]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/")
    }
}

enum Res {
    /// The longest module prefix: (crate, module path).
    Module((usize, Vec<String>)),
    /// Not this workspace's code.
    External,
    /// `super` above the crate root.
    PastRoot,
}

struct Cx<'a> {
    mods: &'a HashMap<(usize, Vec<String>), usize>,
    libs: &'a HashMap<&'a str, usize>,
    k: usize,
    /// The file's module path in crate `k`.
    path: &'a [String],
    facts: &'a Facts,
}

impl Cx<'_> {
    fn is_mod(&self, k: usize, path: &[String]) -> bool {
        self.mods.contains_key(&(k, path.to_vec()))
    }

    fn alias(&self, name: &str) -> Option<&Ref> {
        self.facts.refs.iter().find(|r| {
            r.alias.as_deref() == Some(name) && r.path.first().map(String::as_str) != Some(name)
        })
    }

    /// The modules this file's glob imports name.
    fn globs(&self) -> Vec<(usize, Vec<String>)> {
        self.facts
            .refs
            .iter()
            .filter(|r| r.alias.as_deref() == Some("*"))
            .filter_map(|r| match self.resolve(r, 0) {
                Res::Module(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    fn resolve(&self, r: &Ref, depth: u8) -> Res {
        let mut cur: Vec<String> = self.path.to_vec();
        cur.extend(r.scope.iter().cloned());
        let (k, base, rest): (usize, Vec<String>, &[String]) = match r.path[0].as_str() {
            "crate" => (self.k, Vec::new(), &r.path[1..]),
            "self" => (self.k, cur, &r.path[1..]),
            "super" => {
                let ups = r.path.iter().take_while(|s| *s == "super").count();
                if ups > cur.len() {
                    return Res::PastRoot;
                }
                cur.truncate(cur.len() - ups);
                (self.k, cur, &r.path[ups..])
            }
            first => {
                let mut child = cur.clone();
                child.push(first.to_string());
                if self.is_mod(self.k, &child) {
                    (self.k, cur, &r.path[..])
                } else if let (Some(a), true) = (self.alias(first), depth < 2) {
                    let mut path = a.path.clone();
                    path.extend(r.path[1..].iter().cloned());
                    let via = Ref {
                        scope: a.scope.clone(),
                        path,
                        alias: None,
                    };
                    return self.resolve(&via, depth + 1);
                } else if let Some((gk, g)) = (depth < 2).then(|| self.globs_with(first)).flatten()
                {
                    (gk, g, &r.path[..])
                } else if let Some(&lk) = self.libs.get(first) {
                    (lk, Vec::new(), &r.path[1..])
                } else {
                    return Res::External;
                }
            }
        };
        let mut full = base;
        full.extend(rest.iter().cloned());
        while !self.is_mod(k, &full) {
            if full.pop().is_none() {
                return Res::External;
            }
        }
        Res::Module((k, full))
    }

    /// A glob-imported module with a child module named `name`.
    fn globs_with(&self, name: &str) -> Option<(usize, Vec<String>)> {
        self.facts
            .refs
            .iter()
            .filter(|r| {
                r.alias.as_deref() == Some("*") && r.path.first().map(String::as_str) != Some(name)
            })
            .filter_map(|r| match self.resolve(r, 2) {
                Res::Module(m) => Some(m),
                _ => None,
            })
            .find(|(k, g)| {
                let mut child = g.clone();
                child.push(name.to_string());
                self.is_mod(*k, &child)
            })
    }
}
