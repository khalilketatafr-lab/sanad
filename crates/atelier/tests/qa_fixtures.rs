//! The typesetting fixtures pass the automated checklist: no blocker or major
//! finding (minor findings are layout statistics, reviewed in
//! `docs/qa/typographic-qa.md`).

#![allow(clippy::unwrap_used)]

use std::str::FromStr;

use rustybuzz::{Face, Language};
use sanad_atelier::qa::{Check, Lang, QaOptions, Severity, check_book};
use sanad_atelier::shape::{FontFace, Typesetter};

const LATIN_FONT: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC_FONT: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

fn typesetter() -> Typesetter<'static> {
    Typesetter::new(
        FontFace {
            face: Face::from_slice(LATIN_FONT, 0).unwrap(),
            size_px: 36.0,
            script: rustybuzz::script::LATIN,
            language: Language::from_str("en").unwrap(),
        },
        FontFace {
            face: Face::from_slice(ARABIC_FONT, 0).unwrap(),
            size_px: 42.0,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar").unwrap(),
        },
    )
    .unwrap()
}

#[test]
fn fixtures_have_no_blocker_or_major_finding() {
    let ts = typesetter();
    for (name, lang, text) in [
        (
            "latin",
            Lang::En,
            include_str!("../../../fixtures/typeset/latin.txt"),
        ),
        (
            "arabic",
            Lang::Ar,
            include_str!("../../../fixtures/typeset/arabic.txt"),
        ),
        (
            "mixed",
            Lang::Ar,
            include_str!("../../../fixtures/typeset/mixed.txt"),
        ),
    ] {
        let report = check_book(&ts, lang, text, QaOptions::default()).unwrap();
        let serious: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.check.severity() != Severity::Minor)
            .collect();
        assert!(report.passes(), "{name}: {serious:#?}");
        assert!(report.paragraphs > 0);
    }
}

#[test]
fn every_fixture_character_has_a_glyph() {
    let ts = typesetter();
    for text in [
        include_str!("../../../fixtures/typeset/latin.txt"),
        include_str!("../../../fixtures/typeset/arabic.txt"),
        include_str!("../../../fixtures/typeset/mixed.txt"),
    ] {
        let report = check_book(&ts, Lang::Ar, text, QaOptions { layout: false }).unwrap();
        assert!(!report.counts.contains_key(Check::Coverage.id()));
    }
}
