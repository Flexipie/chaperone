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

impl SpecSection {
    /// The heading names `key`, ignoring case.
    pub(crate) fn has_key(&self, key: &str) -> bool {
        self.key
            .as_deref()
            .is_some_and(|k| k.trim().eq_ignore_ascii_case(key.trim()))
    }
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
/// uppercase `CORE-1234`/`PER-12` or a GitHub `owner/repo#40`. The fold
/// stores its result in `spec_keys`: a change here needs a migration that
/// refills it, as 0003's step does.
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

/// Lowercase ASCII words, everything else gone (`[a-z0-9]+`).
fn words(s: &str) -> String {
    let s = s.to_lowercase();
    let w: Vec<&str> = s
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    w.join(" ")
}

/// Characters `a` and `b` share in matching blocks (Ratcliff/Obershelp,
/// what Python's `SequenceMatcher` counts, less its junk rule for 200+
/// characters).
fn matched(a: &[char], b: &[char]) -> usize {
    let (mut at, mut len) = ((0, 0), 0);
    let mut prev = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        let mut row = vec![0; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            if x == y {
                row[j + 1] = prev[j] + 1;
                if row[j + 1] > len {
                    len = row[j + 1];
                    at = (i + 1 - len, j + 1 - len);
                }
            }
        }
        prev = row;
    }
    if len == 0 {
        return 0;
    }
    let (i, j) = at;
    len + matched(&a[..i], &b[..j]) + matched(&a[i + len..], &b[j + len..])
}

/// Two decision questions are one: equal once case and punctuation go,
/// or a light rewording (ratio 0.85 or more), as `ship-handoff` reads them.
fn same_question(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let total = a.len() + b.len();
    a == b || total > 0 && 2 * matched(&a, &b) * 100 >= 85 * total
}

/// A question's note asks about decision question `q`: the same question,
/// or a note that names it ("confirm: <q>").
pub fn asks_about(note: &str, q: &str) -> bool {
    same_question(note, q) || words(note).contains(&words(q))
}

/// The decisions whose newest line for their question is still agreed
/// `provisional ...`: what the owner has yet to confirm, as `ship-handoff`
/// reads them.
pub fn standing_provisional(decisions: Vec<Decision>) -> Vec<Decision> {
    let q = |d: &Decision| d.question.clone().unwrap_or_else(|| d.text.clone());
    let mut newest: Vec<Decision> = Vec::new();
    for d in decisions {
        let k = q(&d);
        newest.retain(|n| !same_question(&q(n), &k));
        newest.push(d);
    }
    newest.retain(|d| {
        d.answer.is_some()
            && d.agreed
                .as_deref()
                .is_some_and(|a| a.starts_with("provisional"))
    });
    newest
}
