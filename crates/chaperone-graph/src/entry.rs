//! The path globs of a profile's tables.

/// `pattern` names `path`: `*` is any run within a segment, a `**`
/// segment any number of segments.
pub fn glob(pattern: &str, path: &str) -> bool {
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
