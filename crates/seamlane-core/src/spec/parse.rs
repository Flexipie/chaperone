//! The dispatch spec's markdown: `## slice N: <title>  (<KEY>)` sections and
//! the `## decisions` list, parsed as daisy parses them (`daisy-core`
//! `spec.rs`), so both read a spec the same way.

use serde::Serialize;

use crate::work::pr_ref_ok;

/// One `## slice N: <title>  (<KEY>)` section of a dispatch spec.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpecSection {
    pub n: Option<u32>,
    pub key: Option<String>,
    /// The heading's title, without the `slice N:` prefix and the key.
    pub title: String,
    pub done_when: String,
    pub not_in_slice: String,
    /// The section's text under its heading.
    pub body: String,
}

/// One line of the `## decisions` section: `- [slice N] Q: ... A: ...
/// (agreed: where, date)`. A line without that shape still counts, as
/// `text` alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Decision {
    /// The slice it is about; None = the whole spec.
    pub slice: Option<u32>,
    pub question: Option<String>,
    pub answer: Option<String>,
    /// Where and when it was agreed, as written.
    pub agreed: Option<String>,
    /// The whole line, without the list marker.
    pub text: String,
}

/// The value of a `field: value` line, joined with its continuation lines
/// (anything up to the next `word:` field, heading or blank line).
pub fn field(text: &str, name: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut on = false;
    for line in text.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if let Some(rest) = lower.strip_prefix(&format!("{name}:")) {
            out.push(t[t.len() - rest.len()..].trim());
            on = true;
            continue;
        }
        if on {
            let is_field = t.split_once(':').is_some_and(|(k, _)| {
                !k.is_empty()
                    && k.len() <= 30
                    && k.chars().all(|c| c.is_ascii_lowercase() || c == ' ')
            });
            if t.is_empty() || t.starts_with('#') || is_field {
                break;
            }
            out.push(t);
        }
    }
    out.join(" ")
}

/// The issue key in parentheses at the end of a heading, if any: an
/// uppercase `CORE-1234`/`PER-12` or a GitHub `owner/repo#40`.
fn heading_key(heading: &str) -> Option<String> {
    let open = heading.rfind('(')?;
    let close = heading[open..].find(')')? + open;
    let inner = heading[open + 1..close].trim();
    let issue = inner.split_once('-').is_some_and(|(p, n)| {
        !p.is_empty()
            && p.chars().all(|c| c.is_ascii_uppercase())
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
    });
    (pr_ref_ok(inner) || issue).then(|| inner.to_string())
}

/// `slice 2: support can work the inbox  (CORE-3899)` -> the title.
fn heading_title(heading: &str) -> String {
    let t = heading.trim_start_matches('#').trim();
    let t = match t.split_once(':') {
        Some((head, rest)) if head.to_lowercase().starts_with("slice") => rest,
        _ => t,
    };
    let t = match (heading_key(t), t.rfind('(')) {
        (Some(_), Some(open)) => &t[..open],
        _ => t,
    };
    t.trim().to_string()
}

pub(crate) fn is_heading(line: &str) -> bool {
    line.starts_with("## ") || line.starts_with("### ")
}

/// A `##`/`###` heading's text, lowercased.
pub(crate) fn heading_text(line: &str) -> String {
    line.trim_start_matches('#').trim().to_lowercase()
}

/// The `## slice N: ...` sections of a dispatch spec.
pub fn parse_spec_sections(md: &str) -> Vec<SpecSection> {
    let mut out = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    let flush = |cur: Option<(&str, Vec<&str>)>, out: &mut Vec<SpecSection>| {
        if let Some((heading, body)) = cur {
            let body = body.join("\n");
            // Case-sensitive, as daisy reads it: `## Slice 1:` is a slice
            // with no number.
            let n = heading
                .trim_start_matches('#')
                .trim()
                .strip_prefix("slice ")
                .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|d| d.parse().ok());
            out.push(SpecSection {
                n,
                key: heading_key(heading),
                title: heading_title(heading),
                done_when: field(&body, "done when"),
                not_in_slice: field(&body, "not in this slice"),
                body: body.trim().to_string(),
            });
        }
    };
    for line in md.lines() {
        if is_heading(line) {
            flush(current.take(), &mut out);
            if heading_text(line).starts_with("slice ") {
                current = Some((line, Vec::new()));
            }
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    flush(current, &mut out);
    out
}

/// The text after a list marker (`- `, `* `, `1. `), if the line is an item.
fn item_text(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some(rest.trim());
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    (digits > 0)
        .then(|| t[digits..].strip_prefix(". "))
        .flatten()
        .map(str::trim)
}

fn parse_decision(text: &str) -> Decision {
    let mut rest = text.trim();
    let mut slice = None;
    if let Some((tag, after)) = rest.strip_prefix('[').and_then(|r| r.split_once(']'))
        && let Some(n) = tag
            .trim()
            .to_lowercase()
            .strip_prefix("slice")
            .and_then(|n| n.trim().parse().ok())
    {
        slice = Some(n);
        rest = after.trim();
    }
    let (body, agreed) = match rest.rfind("(agreed:") {
        Some(at) if rest.ends_with(')') => (
            rest[..at].trim(),
            Some(
                rest[at + "(agreed:".len()..rest.len() - 1]
                    .trim()
                    .to_string(),
            ),
        ),
        _ => (rest, None),
    };
    let (question, answer) = match body.strip_prefix("Q:").and_then(|q| q.split_once(" A:")) {
        Some((q, a)) => (Some(q.trim().to_string()), Some(a.trim().to_string())),
        None => (None, None),
    };
    Decision {
        slice,
        question,
        answer,
        agreed,
        text: text.trim().to_string(),
    }
}

/// The list items of the `## decisions` section, indented continuation
/// lines folded into their item.
pub fn parse_decisions(md: &str) -> Vec<Decision> {
    let mut items: Vec<String> = Vec::new();
    let mut on = false;
    for line in md.lines() {
        if line.starts_with("# ") || is_heading(line) {
            on = is_heading(line) && heading_text(line).starts_with("decisions");
            continue;
        }
        if !on {
            continue;
        }
        if let Some(text) = item_text(line) {
            items.push(text.to_string());
        } else if !line.trim().is_empty()
            && line.starts_with([' ', '\t'])
            && let Some(last) = items.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    items.iter().map(|t| parse_decision(t)).collect()
}
