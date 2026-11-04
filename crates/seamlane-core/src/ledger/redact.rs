//! The redactor (R13 "Writes"). Prose fields are redacted in place, so a
//! secret pasted into a finding or a handoff never reaches the log; any
//! other string that matches is an identifier of the wrong type, and the
//! write is refused. High-confidence patterns only: a novel token format
//! is an accepted miss.

use serde_json::Value;

use super::event::{Payload, Refusal};
use super::write::Checked;

/// Keys whose strings are prose. A string's key is its nearest object key;
/// an array's strings take the array's key, and `changed.<column>.to` takes
/// the column's.
const PROSE: &[&str] = &[
    "answer",
    "blocker",
    "blockers",
    "body",
    "check_by_hand",
    "command",
    "decisions",
    "error",
    "evidence",
    "goal",
    "merge_case",
    "message",
    "next_action",
    "not_proven",
    "note",
    "open_questions",
    "proven",
    "question",
    "reason",
    "shape",
    "showed",
    "shows",
    "summary",
    "title",
    "verdict",
    "what",
    "where",
    "why",
];

/// Prefix, minimum length after it, the characters it takes, kind.
type Token = (&'static str, usize, fn(u8) -> bool, &'static str);

const TOKENS: &[Token] = &[
    ("ghp_", 20, word, "github_token"),
    ("gho_", 20, word, "github_token"),
    ("ghu_", 20, word, "github_token"),
    ("ghs_", 20, word, "github_token"),
    ("ghr_", 20, word, "github_token"),
    ("github_pat_", 20, word, "github_token"),
    ("AKIA", 16, upper, "aws_key"),
    ("xoxb-", 10, dashed, "slack_token"),
    ("xoxa-", 10, dashed, "slack_token"),
    ("xoxp-", 10, dashed, "slack_token"),
    ("xoxr-", 10, dashed, "slack_token"),
    ("xoxs-", 10, dashed, "slack_token"),
    ("sk-ant-", 20, dashed, "api_key"),
    ("sk-proj-", 20, dashed, "api_key"),
];

const SECRET_NAMES: &[&str] = &["TOKEN", "SECRET", "PASSWORD", "API_KEY", "APIKEY"];

const REDACTED: &str = "[redacted:";

/// Redacts the prose strings of `p` in place and returns how many matches
/// it replaced. A match outside prose refuses, naming the key, not the value.
pub(super) fn payload(p: &mut Payload) -> Checked<u64> {
    let mut v = serde_json::to_value(&*p).map_err(std::io::Error::other)?;
    let mut count = 0;
    if let Err(r) = root(&mut v, &mut count) {
        return Ok(Err(r));
    }
    if count > 0 {
        *p = serde_json::from_value(v).map_err(std::io::Error::other)?;
    }
    Ok(Ok(count))
}

/// The payload object. `changed.<column>.from` is a compare value, checked
/// against the row, so it is left as sent; `.to` is new text for its column.
fn root(v: &mut Value, count: &mut u64) -> Result<(), Refusal> {
    for (k, x) in v.as_object_mut().into_iter().flatten() {
        key(k)?;
        if k == "changed" {
            for (column, change) in x.as_object_mut().into_iter().flatten() {
                key(column)?;
                walk(&mut change["to"], column, count)?;
            }
        } else {
            walk(x, k, count)?;
        }
    }
    Ok(())
}

fn walk(v: &mut Value, name: &str, count: &mut u64) -> Result<(), Refusal> {
    match v {
        Value::String(s) => {
            let (out, kinds) = redact(s);
            if let Some(kind) = kinds.first() {
                if !PROSE.contains(&name) {
                    return Err(Refusal::InvalidInput(format!(
                        "{name} holds what looks like a {kind}; an identifier is refused, not redacted"
                    )));
                }
                *s = out;
                *count += kinds.len() as u64;
            }
        }
        Value::Array(a) => {
            for x in a {
                walk(x, name, count)?;
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                key(k)?;
                walk(x, k, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A key is a name, never prose.
fn key(k: &str) -> Result<(), Refusal> {
    match redact(k).1.first() {
        Some(kind) => Err(Refusal::InvalidInput(format!(
            "a payload key looks like a {kind}; a key is refused, not redacted"
        ))),
        None => Ok(()),
    }
}

/// `s` with every match replaced by `[redacted:<kind>]`, and the kind of
/// each match. Redacting the result again changes nothing.
fn redact(s: &str) -> (String, Vec<&'static str>) {
    let mut out = String::with_capacity(s.len());
    let mut kinds = Vec::new();
    // Where each `-----END ` starts, found once so a key's END line is a
    // lookup and many BEGIN headers stay linear.
    let ends: Vec<usize> = s.match_indices("-----END ").map(|(at, _)| at).collect();
    let (mut i, mut copied) = (0, 0);
    while i < s.len() {
        match hit(s, &ends, i) {
            // Matches start and end on ASCII bytes, so the slices are on
            // char boundaries.
            Some((start, end, kind)) => {
                out.push_str(&s[copied..start]);
                out.push_str(REDACTED);
                out.push_str(kind);
                out.push(']');
                kinds.push(kind);
                (i, copied) = (end, end);
            }
            None => i += 1,
        }
    }
    out.push_str(&s[copied..]);
    (out, kinds)
}

/// The byte range to replace for a match found at `i`, and its kind.
fn hit(s: &str, ends: &[usize], i: usize) -> Option<(usize, usize, &'static str)> {
    // Every pattern starts on an ASCII byte.
    if !s.is_char_boundary(i) {
        return None;
    }
    let b = s.as_bytes();
    let rest = &s[i..];
    if let Some(label) = rest.strip_prefix("-----BEGIN ") {
        let label = &label[..label.find("-----")?];
        if !label.contains("PRIVATE KEY") || label.contains('\n') {
            return None;
        }
        // Through the matching END line, whatever the key is wrapped in
        // (indented YAML, `\n` escapes, one flattened line, PGP armor
        // headers). With no END line next, a key cut off is still a key,
        // but only its base64 lines go: prose that names the header keeps
        // the text after it.
        let header = i + "-----BEGIN -----".len() + label.len();
        let end_line = format!("-----END {label}-----");
        let next_end = ends.get(ends.partition_point(|&e| e < header));
        let end = match next_end {
            Some(&e) if s[e..].starts_with(&end_line) => e + end_line.len(),
            _ => {
                header
                    + run(b, header, |c| {
                        c.is_ascii_alphanumeric() || b"+/=\r\n".contains(&c)
                    })
            }
        };
        return Some((i, end, "private_key"));
    }
    for &(prefix, min, takes, kind) in TOKENS {
        if rest.starts_with(prefix) {
            let n = run(b, i + prefix.len(), takes);
            return (n >= min).then_some((i, i + prefix.len() + n, kind));
        }
    }
    if rest.starts_with("Bearer") {
        let start = i + 6 + run(b, i + 6, blank);
        if start == i + 6 {
            return None;
        }
        let n = run(b, start, bearer);
        return (n >= 20).then_some((start, start + n, "bearer"));
    }
    if matches!(b.get(i), Some(b'=' | b':')) {
        return assignment(s, ends, i);
    }
    None
}

/// `NAME_TOKEN=value`, `"API_KEY": "value"`, `API_KEY := value`,
/// `API_KEY: &str = "value"`: the value side, when the name ends in one of
/// [`SECRET_NAMES`] and `b[sep]` is `=` or `:`.
fn assignment(s: &str, ends: &[usize], sep: usize) -> Option<(usize, usize, &'static str)> {
    let b = s.as_bytes();
    let mut j = sep;
    while j > 0 && blank(b[j - 1]) {
        j -= 1;
    }
    // `env["API_KEY"] =`, `"API_KEY":`, and `\"API_KEY\":` inside a logged
    // JSON string.
    for close in [b']', b'"', b'\\'] {
        if j > 0 && (b[j - 1] == close || close == b'"' && b[j - 1] == b'\'') {
            j -= 1;
        }
    }
    let name_start = (0..j).rev().take_while(|&k| word(b[k])).last()?;
    let name = &s[name_start..j];
    if !SECRET_NAMES.iter().any(|n| name.ends_with(n)) {
        return None;
    }
    let mut at = sep + 1;
    if b[sep] == b':' {
        match b.get(at) {
            Some(b':') => return None,
            Some(b'=') => at += 1,
            _ => {
                // A type annotation: the value is after its `=`. Types are
                // short, so the look is capped and a long line stays linear.
                let t = at + run(b, at, blank);
                let ty = run(&b[..b.len().min(t + 64)], t, |c| {
                    !c.is_ascii_whitespace() && c != b'='
                });
                let gap = run(b, t + ty, blank);
                let eq = t + ty + gap;
                // `NAME: T = value`: a type, not a quoted value, then a
                // spaced `=`. `NAME: abc=` is a value with an `=` in it.
                if gap > 0
                    && !matches!(b.get(t), Some(b'"' | b'\''))
                    && b.get(eq) == Some(&b'=')
                    && b.get(eq + 1) != Some(&b'=')
                {
                    at = eq + 1;
                }
            }
        }
    }
    let mut start = at + run(b, at, blank);
    let quote = b.get(start).copied().filter(|&c| c == b'"' || c == b'\'');
    start += usize::from(quote.is_some());
    let rest = &s[start..];
    // Already redacted, a reference to a secret (`${{ secrets.X }}`), or
    // another pattern that starts here and redacts more than the first word
    // (`Bearer ` always, so a redacted bearer stays as it is). Checked before
    // the value is read, so a long line stays linear.
    if [REDACTED, "${", "Bearer "]
        .iter()
        .any(|p| rest.starts_with(p))
        || hit(s, ends, start).is_some()
    {
        return None;
    }
    let n = match quote {
        Some(q) => run(b, start, |c| c != q),
        None => run(b, start, |c| !c.is_ascii_whitespace()),
    };
    (n > 0).then_some((start, start + n, "assignment"))
}

fn run(b: &[u8], from: usize, takes: impl Fn(u8) -> bool) -> usize {
    b.get(from..)
        .map_or(0, |t| t.iter().take_while(|&&c| takes(c)).count())
}

fn blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn upper(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

fn dashed(c: u8) -> bool {
    word(c) || c == b'-'
}

fn bearer(c: u8) -> bool {
    dashed(c) || matches!(c, b'.' | b'~' | b'+' | b'/' | b'=')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ledger::{Change, Finding, Findings};

    /// One fixture per R13 pattern: the text, what it becomes.
    #[test]
    fn each_pattern_is_redacted() {
        let t20 = "a".repeat(20);
        let pem =
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blbg==\n-----END OPENSSH PRIVATE KEY-----";
        let cases = [
            (format!("key {pem} done"), "key [redacted:private_key] done"),
            (
                "-----BEGIN PRIVATE KEY-----\nMIIEvQ\nab+/=".into(),
                "[redacted:private_key]",
            ),
            (
                "see `-----BEGIN PRIVATE KEY-----` in parser.rs".into(),
                "see `[redacted:private_key]` in parser.rs",
            ),
            (
                "JWT_SECRET: Zm9vYmFyMA=".into(),
                "JWT_SECRET: [redacted:assignment]",
            ),
            (
                r#"{"API_KEY":"hunter2","url":"https://x?a=b"}"#.into(),
                r#"{"API_KEY":"[redacted:assignment]","url":"https://x?a=b"}"#,
            ),
            (
                r#"{"API_KEY": "a = b"}"#.into(),
                r#"{"API_KEY": "[redacted:assignment]"}"#,
            ),
            (
                "DB_PASSWORD:hunter2=x SESSION_SECRET: s3cr3t=value".into(),
                "DB_PASSWORD:[redacted:assignment] SESSION_SECRET: [redacted:assignment]",
            ),
            (
                "tls.key: |\n  -----BEGIN RSA PRIVATE KEY-----\n  MIIE\n  -----END RSA PRIVATE KEY-----\nnext: 1".into(),
                "tls.key: |\n  [redacted:private_key]\nnext: 1",
            ),
            (
                r#"{"private_key":"-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n","x":1}"#.into(),
                r#"{"private_key":"[redacted:private_key]\n","x":1}"#,
            ),
            (
                "-----BEGIN PRIVATE KEY----- MIIE ab -----END PRIVATE KEY----- after".into(),
                "[redacted:private_key] after",
            ),
            (
                "-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: 1\n\nlQ\n-----END PGP PRIVATE KEY BLOCK-----".into(),
                "[redacted:private_key]",
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----".into(),
                "[redacted:private_key]-----END RSA PRIVATE KEY-----",
            ),
            (
                r#"log {\"API_KEY\": \"hunter2\"}"#.into(),
                r#"log {\"API_KEY\": [redacted:assignment]"#,
            ),
            (
                r#"os.environ["API_TOKEN"] = "abc123""#.into(),
                r#"os.environ["API_TOKEN"] = "[redacted:assignment]""#,
            ),
            (
                "ENV['API_KEY'] = x".into(),
                "ENV['API_KEY'] = [redacted:assignment]",
            ),
            (
                "MY_TOKEN\t=\tabc KEY_SECRET:\tv".into(),
                "MY_TOKEN\t=\t[redacted:assignment] KEY_SECRET:\t[redacted:assignment]",
            ),
            (
                "PASSWORD=Bearerish123".into(),
                "PASSWORD=[redacted:assignment]",
            ),
            (format!("ghp_{t20}."), "[redacted:github_token]."),
            (format!("x gho_{t20}"), "x [redacted:github_token]"),
            (format!("ghu_{t20}"), "[redacted:github_token]"),
            (format!("ghs_{t20}"), "[redacted:github_token]"),
            (format!("ghr_{t20}"), "[redacted:github_token]"),
            (format!("github_pat_{t20}_B9"), "[redacted:github_token]"),
            ("id AKIAIOSFODNN7EXAMPLE,".into(), "id [redacted:aws_key],"),
            ("xoxb-12345-67890".into(), "[redacted:slack_token]"),
            ("xoxa-1234567890".into(), "[redacted:slack_token]"),
            ("xoxp-1234567890".into(), "[redacted:slack_token]"),
            ("xoxr-1234567890".into(), "[redacted:slack_token]"),
            ("xoxs-1234567890".into(), "[redacted:slack_token]"),
            (
                format!("Authorization: Bearer {t20}.x/y="),
                "Authorization: Bearer [redacted:bearer]",
            ),
            (format!("sk-ant-api03-{t20}"), "[redacted:api_key]"),
            (format!("sk-proj-{t20}"), "[redacted:api_key]"),
            (
                "GITHUB_TOKEN=abc123 cargo".into(),
                "GITHUB_TOKEN=[redacted:assignment] cargo",
            ),
            (
                "DB_PASSWORD = 'p w'".into(),
                "DB_PASSWORD = '[redacted:assignment]'",
            ),
            (
                r#"{"API_KEY": "v"}"#.into(),
                r#"{"API_KEY": "[redacted:assignment]"}"#,
            ),
            (
                "-----BEGIN PGP PRIVATE KEY BLOCK-----\nx\n-----END PGP PRIVATE KEY BLOCK-----"
                    .into(),
                "[redacted:private_key]",
            ),
            (
                "JWT_SECRET=-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----".into(),
                "JWT_SECRET=[redacted:private_key]",
            ),
            (
                format!("ACCESS_TOKEN: Bearer {t20}"),
                "ACCESS_TOKEN: Bearer [redacted:bearer]",
            ),
            ("API_KEY := v".into(), "API_KEY := [redacted:assignment]"),
            (
                "API_KEY: x == y".into(),
                "API_KEY: [redacted:assignment] == y",
            ),
            (
                r#"const API_KEY: &str = "v";"#.into(),
                r#"const API_KEY: &str = "[redacted:assignment]";"#,
            ),
            (
                "SECRET:v APIKEY=w".into(),
                "SECRET:[redacted:assignment] APIKEY=[redacted:assignment]",
            ),
        ];
        for (text, want) in cases {
            let (out, _) = redact(&text);
            assert_eq!(out, want, "{text}");
            assert_eq!(
                redact(&out),
                (out.clone(), vec![]),
                "redacting twice: {text}"
            );
        }
    }

    /// Text close to a pattern stays as it is.
    #[test]
    fn near_misses_are_kept() {
        for text in [
            "ghp_short",
            "AKIAlowercase0000000",
            "xoxb-123",
            "Bearer short",
            "Bearer",
            "Bearer-token-that-is-long-enough",
            "sk-ant-short",
            "-----BEGIN PUBLIC KEY-----",
            "-----BEGIN PRIVATE",
            "-----BEGIN A\nRSA PRIVATE KEY-----",
            "TOKEN_COUNT=3 is fine? no: NAME=v, github_token=lowercase",
            "API_KEY::new()",
            "API_KEY=",
            "= : leading separators",
            "é TOKEN=",
            "GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
            "TOKEN=",
        ] {
            assert_eq!(redact(text), (text.to_string(), vec![]), "{text}");
        }
        assert_eq!(redact("é TOKEN=ü").0, "é TOKEN=[redacted:assignment]");
    }

    /// Prose anywhere in the payload is redacted and counted; a match in
    /// any other field refuses and does not echo the value.
    #[test]
    fn prose_is_redacted_and_identifiers_refused() {
        let token = format!("ghp_{}", "a".repeat(20));
        let mut p = Payload {
            findings: Some(Findings::List(vec![Finding {
                finding_key: "f".into(),
                severity: "nit".into(),
                title: format!("t {token}"),
                body: format!("b {token} {token}"),
                path: None,
                rest: Default::default(),
            }])),
            changed: [(
                "blocker".to_string(),
                Change {
                    from: json!(format!("old {token}")),
                    to: json!(token),
                },
            )]
            .into(),
            ..Payload::default()
        };
        p.rest.insert(
            "handoff".into(),
            json!({"verification": [{"command": "c", "note": token}],
                   "smoke": [{"what": "w", "shows": token}]}),
        );
        assert_eq!(payload(&mut p).unwrap(), Ok(6));
        // `from` is the row's value to compare, not new text.
        let from = p.changed.remove("blocker").unwrap().from;
        assert_eq!(from, json!(format!("old {token}")));
        assert!(!serde_json::to_string(&p).unwrap().contains(&token));

        // Only the top-level `changed` leaves `from` alone.
        let mut nested = Payload::default();
        nested
            .rest
            .insert("x".into(), json!({"changed": {"a": {"from": token}}}));
        let mut keyed = Payload::default();
        keyed.rest.insert(token.clone(), json!(1));
        let column = Payload {
            changed: [(
                token.clone(),
                Change {
                    from: json!(null),
                    to: json!(null),
                },
            )]
            .into(),
            ..Payload::default()
        };
        let branch = Payload {
            changed: [(
                "branch".to_string(),
                Change {
                    from: json!(null),
                    to: json!(token),
                },
            )]
            .into(),
            ..Payload::default()
        };
        for (key, mut p) in [
            (
                "head_sha",
                Payload {
                    head_sha: Some(token.clone()),
                    ..Payload::default()
                },
            ),
            (
                "to",
                Payload {
                    to: Some(token.clone()),
                    ..Payload::default()
                },
            ),
            ("branch", branch),
            ("from", nested),
            ("a payload key", keyed),
            ("a payload key", column),
        ] {
            let refused = format!("{:?}", payload(&mut p).unwrap());
            assert!(
                refused.starts_with(&format!("Err(InvalidInput(\"{key} ")),
                "{refused}"
            );
            assert!(refused.contains("github_token"), "{refused}");
            assert!(!refused.contains(&token), "{refused}");
        }
    }
}
