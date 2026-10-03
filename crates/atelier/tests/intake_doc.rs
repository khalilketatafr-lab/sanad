//! `docs/atelier/intake.md` lists every intake rule with the code's
//! severity and EPUBCheck id.

use sanad_atelier::epub::diag::ALL;

const DOC: &str = include_str!("../../../docs/atelier/intake.md");

#[test]
fn every_rule_is_documented_exactly() {
    let rows: Vec<Vec<&str>> = DOC
        .lines()
        .filter(|l| l.starts_with("| ") && l.as_bytes().get(5) == Some(&b'-'))
        .map(|l| l.split('|').map(str::trim).collect())
        .collect();
    assert_eq!(rows.len(), ALL.len(), "one row per rule");
    for code in ALL {
        let row = rows
            .iter()
            .find(|r| r[1] == code.id)
            .unwrap_or_else(|| panic!("{} is not documented", code.id));
        assert_eq!(
            row[2],
            format!("{:?}", code.severity).to_lowercase(),
            "{}",
            code.id
        );
        assert_eq!(row[3], code.epubcheck.unwrap_or("—"), "{}", code.id);
        assert_eq!(row[4], code.summary, "{}", code.id);
    }
}
