//! What one Rust file says about other modules, read with tree-sitter.
//! Depends on the file's bytes only, so it is cached by content.

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

/// Bump when what `facts` extracts changes, or the grammar does: cached
/// facts of another version are never read (`cache.rs` names its table
/// with it).
pub const FACTS_VERSION: u32 = 3;
/// Tree depth read; every walk below recurses along the tree, so a deeper
/// file could overflow the stack and is not read.
const MAX_DEPTH: u32 = 256;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Facts {
    /// `mod name;` declarations: (inline modules around it, name).
    pub mods: Vec<(Vec<String>, String)>,
    pub refs: Vec<Ref>,
    /// The self type of each `impl`, as written.
    pub impls: Vec<Ref>,
    /// `macro_rules!` names defined here, with whether `#[macro_export]`.
    pub macros: Vec<(String, bool)>,
    /// Bare macro names invoked here (`name!`).
    pub bangs: Vec<String>,
    /// Why some of this file's references could not be read: a parse
    /// error, `#[path]`, `include!`, or nesting past `MAX_DEPTH`.
    pub inexact: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Ref {
    /// The inline modules the path is written in.
    pub scope: Vec<String>,
    pub path: Vec<String>,
    /// For a `use`: the name it brings in, `*` for a glob.
    pub alias: Option<String>,
}

/// A Rust parser, made once per call and reused for every file.
pub fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("the Rust grammar matches the tree-sitter it was built with");
    parser
}

pub fn facts(parser: &mut Parser, src: &[u8]) -> Facts {
    let mut f = Facts::default();
    let Some(tree) = parser.parse(src, None) else {
        f.inexact = Some("parse_error".into());
        return f;
    };
    let root = tree.root_node();
    if root.has_error() {
        f.inexact = Some("parse_error".into());
    }
    if too_deep(root) {
        f.inexact.get_or_insert_with(|| "too_deep".into());
        return f;
    }
    walk(root, src, &mut Vec::new(), &mut f);
    f
}

fn too_deep(root: Node) -> bool {
    let mut c = root.walk();
    loop {
        if c.depth() > MAX_DEPTH {
            return true;
        }
        if c.goto_first_child() {
            continue;
        }
        while !c.goto_next_sibling() {
            if !c.goto_parent() {
                return false;
            }
        }
    }
}

fn walk(node: Node, src: &[u8], scope: &mut Vec<String>, f: &mut Facts) {
    match node.kind() {
        "mod_item" => {
            let Some(name) = node.child_by_field_name("name").map(|n| text(n, src)) else {
                return;
            };
            if has_attr(node, src, "#[path") {
                f.inexact.get_or_insert_with(|| "path_attr".into());
                return;
            }
            match node.child_by_field_name("body") {
                Some(body) => {
                    scope.push(name);
                    walk(body, src, scope, f);
                    scope.pop();
                }
                None => f.mods.push((scope.clone(), name)),
            }
            return;
        }
        "use_declaration" => {
            if let Some(arg) = node.child_by_field_name("argument") {
                use_tree(arg, src, &[], scope, f);
            }
            return;
        }
        "impl_item" => {
            if let Some(path) = node.child_by_field_name("type").and_then(|t| segs(t, src)) {
                f.impls.push(Ref {
                    scope: scope.clone(),
                    path,
                    alias: None,
                });
            }
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(path) = segs(node, src) {
                f.refs.push(Ref {
                    scope: scope.clone(),
                    path,
                    alias: None,
                });
                // `Vec::<crate::a::Bar>::new`: type arguments inside the path.
                let mut at = node.child_by_field_name("path");
                while let Some(p) = at {
                    if let Some(args) = p.child_by_field_name("type_arguments") {
                        walk(args, src, scope, f);
                    }
                    at = p
                        .child_by_field_name("path")
                        .or_else(|| p.child_by_field_name("type"));
                }
                return;
            }
        }
        "macro_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                let export = has_attr(node, src, "#[macro_export");
                f.macros.push((text(name, src), export));
            }
        }
        "macro_invocation" => {
            if let Some(m) = node
                .child_by_field_name("macro")
                .filter(|m| m.kind() == "identifier")
            {
                let name = text(m, src);
                if name == "include" {
                    f.inexact.get_or_insert_with(|| "include".into());
                }
                bang(f, name);
            }
        }
        // Macro arguments are tokens, not expressions: `a::b` in them is
        // read from the token run.
        "token_tree" => {
            tokens(node, src, scope, f);
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, src, scope, f);
    }
}

fn bang(f: &mut Facts, name: String) {
    if !f.bangs.contains(&name) {
        f.bangs.push(name);
    }
}

fn has_attr(node: Node, src: &[u8], prefix: &str) -> bool {
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev.filter(|p| p.kind() == "attribute_item") {
        if text(p, src).replace(' ', "").starts_with(prefix) {
            return true;
        }
        prev = p.prev_named_sibling();
    }
    false
}

/// Each path a `use` tree names, with the name it brings in.
fn use_tree(node: Node, src: &[u8], prefix: &[String], scope: &[String], f: &mut Facts) {
    let mut push = |path: Vec<String>, alias: String| {
        f.refs.push(Ref {
            scope: scope.to_vec(),
            path,
            alias: Some(alias),
        })
    };
    let join = |n: Node| segs(n, src).map(|s| [prefix, &s].concat());
    match node.kind() {
        "use_as_clause" => {
            let path = node.child_by_field_name("path").and_then(join);
            let alias = node.child_by_field_name("alias").map(|a| text(a, src));
            if let (Some(path), Some(alias)) = (path, alias) {
                push(path, alias);
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let inner = node.named_children(&mut cursor).next();
            if let Some(path) = inner.and_then(join) {
                push(path, "*".into());
            }
        }
        "scoped_use_list" => {
            let base = match node.child_by_field_name("path") {
                Some(p) => join(p),
                None => Some(prefix.to_vec()),
            };
            if let (Some(base), Some(list)) = (base, node.child_by_field_name("list")) {
                use_tree(list, src, &base, scope, f);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                use_tree(child, src, prefix, scope, f);
            }
        }
        // `{self}` in a list names the prefix itself.
        "self" if !prefix.is_empty() => push(prefix.to_vec(), prefix[prefix.len() - 1].clone()),
        _ => {
            if let Some(path) = join(node) {
                let alias = path.last().cloned().unwrap_or_default();
                push(path, alias);
            }
        }
    }
}

/// A path's segments; `None` for a shape that is not a plain path.
fn segs(node: Node, src: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            Some(vec![text(node, src)])
        }
        "generic_type" => segs(node.child_by_field_name("type")?, src),
        "scoped_identifier" | "scoped_type_identifier" => {
            let mut out = match node.child_by_field_name("path") {
                Some(p) => segs(p, src)?,
                None => Vec::new(),
            };
            out.push(text(node.child_by_field_name("name")?, src));
            Some(out)
        }
        _ => None,
    }
}

/// `a::b::c` runs among a macro's tokens, nested groups and `$(...)*`
/// included, `$crate` as `crate`, strings that are such a path
/// (`#[serde(default = "crate::a::f")]`), and `name!`.
fn tokens(node: Node, src: &[u8], scope: &[String], f: &mut Facts) {
    let push = |f: &mut Facts, path| {
        f.refs.push(Ref {
            scope: scope.to_vec(),
            path,
            alias: None,
        })
    };
    let mut run: Vec<String> = Vec::new();
    let mut joined = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = match child.kind() {
            "metavariable" if text(child, src) == "$crate" => "crate",
            k => k,
        };
        match kind {
            "identifier" | "crate" | "self" | "super" => {
                if !joined && run.len() > 1 {
                    push(f, std::mem::take(&mut run));
                }
                if !joined {
                    run.clear();
                }
                run.push(match kind {
                    "crate" => kind.into(),
                    _ => text(child, src),
                });
                joined = false;
            }
            "::" => joined = true,
            kind => {
                if kind == "!" && run.len() == 1 {
                    bang(f, run[0].clone());
                }
                if run.len() > 1 {
                    push(f, std::mem::take(&mut run));
                }
                run.clear();
                joined = false;
                if kind == "token_tree" || kind == "token_repetition" {
                    tokens(child, src, scope, f);
                } else if let Some(path) = (kind == "string_literal")
                    .then(|| str_path(&text(child, src)))
                    .flatten()
                {
                    push(f, path);
                }
            }
        }
    }
    if run.len() > 1 {
        push(f, run);
    }
}

/// `"a::b"` as a path: two or more identifiers and nothing else.
fn str_path(lit: &str) -> Option<Vec<String>> {
    let inner = lit.strip_prefix('"')?.strip_suffix('"')?;
    let path: Vec<String> = inner.split("::").map(String::from).collect();
    let ident = |s: &String| {
        s.starts_with(|c: char| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    (path.len() > 1 && path.iter().all(ident)).then_some(path)
}

fn text(node: Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or_default().to_string()
}
