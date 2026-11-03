//! `EventKind` is the M1 list in `docs/events.md`, and no reserved kind parses.

use std::collections::BTreeSet;

use seamlane_core::ledger::EventKind;

/// Backticked kinds in the first column of the `Kind` tables under a `## `
/// heading, up to the next one.
fn kinds(doc: &str, heading: &str) -> BTreeSet<String> {
    let section = doc
        .split("\n## ")
        .find(|s| s.starts_with(heading))
        .unwrap_or_else(|| panic!("events.md has no ## {heading}"));
    let mut found = BTreeSet::new();
    let mut in_kind_table = false;
    for line in section.lines() {
        let Some(first) = line.strip_prefix('|').and_then(|l| l.split('|').next()) else {
            in_kind_table = false;
            continue;
        };
        if first.trim() == "Kind" {
            in_kind_table = true;
        } else if in_kind_table {
            found.extend(first.split('`').skip(1).step_by(2).map(String::from));
        }
    }
    found
}

#[test]
fn event_kinds_match_events_md() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/events.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let ours: BTreeSet<String> = EventKind::ALL.iter().map(|k| k.as_str().into()).collect();
    assert_eq!(ours, kinds(&doc, "M1 kinds"));
    for kind in EventKind::ALL {
        assert_eq!(kind.as_str().parse::<EventKind>(), Ok(*kind));
    }
    let reserved = kinds(&doc, "Reserved");
    assert!(reserved.len() > 20, "{reserved:?}");
    for kind in reserved {
        assert!(kind.parse::<EventKind>().is_err(), "reserved {kind} parses");
    }
}
