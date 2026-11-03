//! `spec decide`: one decision appended to `## decisions` as a new version,
//! in daisy's line shape, `- [slice N] Q: ... A: ... (agreed: <where>, <date>)`.

use serde_json::json;

use super::parse::{self, heading_text, is_heading};
use super::{Spec, body_title, read};
use crate::ledger::{Actor, Checked, EventKind, Ledger, Mutation, Payload, Refusal};
use crate::work::resolve_spec;

/// What a decision says. Neither `slice` nor `key` makes it spec-wide.
#[derive(Debug, Clone, Default)]
pub struct DecideInput {
    pub slice: Option<u32>,
    /// The slice by its issue key instead of its number.
    pub key: Option<String>,
    pub question: String,
    pub answer: String,
    /// Where it was agreed: a PR thread, a Slack link, "in chat".
    pub agreed_where: Option<String>,
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Ledger {
    /// The decision as a new version of the newest body, read and written
    /// in one transaction, so two decides at once both land.
    pub fn spec_decide(
        &self,
        actor: &Actor,
        spec: &str,
        input: &DecideInput,
        at: &str,
        dry_run: bool,
    ) -> Checked<Spec> {
        let (question, answer) = (one_line(&input.question), one_line(&input.answer));
        if question.is_empty() || answer.is_empty() {
            return Ok(Err(Refusal::InvalidInput(
                "a decision needs a --question and an --answer".into(),
            )));
        }
        let agreed_where = input
            .agreed_where
            .as_deref()
            .map(one_line)
            .filter(|w| !w.is_empty());
        self.spec_write_planned(actor, at, dry_run, |conn, at| {
            let id = match resolve_spec(conn, spec)? {
                Ok(id) => id,
                Err(r) => return Ok(Err(r)),
            };
            let Some(head) = read(conn, id, None)? else {
                return Ok(Err(Refusal::NotFound { entity: "spec", id }));
            };
            let sections = parse::parse_spec_sections(&head.body);
            let section = match (input.slice, input.key.as_deref()) {
                (Some(n), _) => match sections.iter().find(|s| s.n == Some(n)) {
                    Some(s) => Some(s),
                    None => {
                        return Ok(Err(Refusal::InvalidInput(format!(
                            "spec {id} has no slice {n}"
                        ))));
                    }
                },
                (None, Some(key)) => {
                    match sections
                        .iter()
                        .find(|s| s.has_key(key))
                        .filter(|s| s.n.is_some())
                    {
                        Some(s) => Some(s),
                        None => {
                            return Ok(Err(Refusal::InvalidInput(format!(
                                "spec {id} has no slice for {key}"
                            ))));
                        }
                    }
                }
                (None, None) => None,
            };
            let slice = section.and_then(|s| s.n);
            let date = at.get(..10).unwrap_or(at);
            let agreed = match &agreed_where {
                Some(w) => format!("{w}, {date}"),
                None => date.to_string(),
            };
            let tag = slice.map(|n| format!("[slice {n}] ")).unwrap_or_default();
            let line = format!("- {tag}Q: {question} A: {answer} (agreed: {agreed})");
            let body = append_decision(&head.body, &line);
            let mut payload = Payload {
                title: body_title(&body),
                body: Some(body),
                ..Payload::default()
            };
            let reason = match slice {
                Some(n) => format!("decision on slice {n}"),
                None => "decision".into(),
            };
            payload.rest.insert("reason".into(), reason.into());
            payload.rest.insert(
                "decision".into(),
                json!({
                    "slice": slice,
                    "key": section.and_then(|s| s.key.clone()),
                    "question": question,
                    "answer": answer,
                    "where": agreed_where,
                    "date": date,
                }),
            );
            Ok(Ok(Mutation::Event {
                kind: EventKind::SpecRevised,
                lane_id: None,
                spec_id: Some(id),
                payload,
            }))
        })
    }
}

/// `body` with `line` appended to its `## decisions` section; the section
/// is created before the first slice when there is none, else at the end.
fn append_decision(body: &str, line: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<String> = Vec::new();
    if let Some(start) = lines
        .iter()
        .position(|l| is_heading(l) && heading_text(l).starts_with("decisions"))
    {
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.starts_with("# ") || is_heading(l))
            .map_or(lines.len(), |i| start + 1 + i);
        // After the section's last non-blank line, so the blank line before
        // the next heading stays.
        let insert_at = (start + 1..end)
            .rev()
            .find(|i| !lines[*i].trim().is_empty())
            .map_or(start + 1, |i| i + 1);
        out.extend(lines[..insert_at].iter().map(|l| l.to_string()));
        if insert_at == start + 1 {
            out.push(String::new());
        }
        out.push(line.to_string());
        out.extend(lines[insert_at..].iter().map(|l| l.to_string()));
    } else {
        let first_slice = lines
            .iter()
            .position(|l| is_heading(l) && heading_text(l).starts_with("slice "));
        match first_slice {
            Some(at) => {
                out.extend(lines[..at].iter().map(|l| l.to_string()));
                out.extend([
                    "## decisions".to_string(),
                    String::new(),
                    line.to_string(),
                    String::new(),
                ]);
                out.extend(lines[at..].iter().map(|l| l.to_string()));
            }
            None => {
                out.extend(lines.iter().map(|l| l.to_string()));
                while out.last().is_some_and(|l| l.trim().is_empty()) {
                    out.pop();
                }
                out.extend([
                    String::new(),
                    "## decisions".to_string(),
                    String::new(),
                    line.to_string(),
                ]);
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}
