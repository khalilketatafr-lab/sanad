//! `docs/qa/typographic-qa.md` and `crates/atelier/src/qa.rs` describe the
//! same automated checks: same ids, same severities, nothing missing on
//! either side.

use sanad_atelier::qa::Check;

const CHECKLIST: &str = include_str!("../../../docs/qa/typographic-qa.md");

/// `(id, severity)` of every automated row: `| TQ-xx | check | scope | severity | … |`.
fn automated_rows() -> Vec<(&'static str, &'static str)> {
    CHECKLIST
        .lines()
        .filter(|l| l.starts_with("| TQ-") && !l.starts_with("| TQ-M"))
        .map(|l| {
            let cols: Vec<&str> = l.split('|').map(str::trim).collect();
            (cols[1], cols[4])
        })
        .collect()
}

#[test]
fn every_check_is_documented_with_its_severity() {
    let rows = automated_rows();
    for check in Check::ALL {
        let row = rows.iter().find(|(id, _)| *id == check.id());
        let expected = format!("{:?}", check.severity()).to_lowercase();
        assert_eq!(
            row.map(|(_, sev)| *sev),
            Some(expected.as_str()),
            "{} in docs/qa/typographic-qa.md",
            check.id()
        );
    }
}

#[test]
fn no_documented_check_is_missing_from_the_code() {
    let rows = automated_rows();
    // Rows appear once in the checklist tables (the results table repeats
    // ids with numbers in column 4, which never parse as a severity).
    let documented: Vec<&str> = rows
        .iter()
        .filter(|(_, sev)| matches!(*sev, "blocker" | "major" | "minor"))
        .map(|(id, _)| *id)
        .collect();
    for id in &documented {
        assert!(
            Check::ALL.iter().any(|c| c.id() == *id),
            "{id} is documented but not implemented"
        );
    }
    assert_eq!(documented.len(), Check::ALL.len());
}
