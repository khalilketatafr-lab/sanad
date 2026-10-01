//! The launch manifest (`catalog/launch/titles.toml`) obeys its rules and
//! the composition roadmap §10.7 asks for.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use sanad_atelier::catalog::{Feature, Manifest, Source, Status};
use sanad_atelier::qa::Lang;

const MANIFEST: &str = include_str!("../../../catalog/launch/titles.toml");
const README: &str = include_str!("../../../catalog/launch/README.md");

fn manifest() -> Manifest {
    Manifest::parse(MANIFEST).unwrap()
}

#[test]
fn every_title_is_publishable_as_listed() {
    let issues = manifest().validate();
    assert!(
        issues.is_empty(),
        "{}",
        issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn fifty_titles_in_three_languages() {
    let m = manifest();
    assert_eq!(m.titles.len(), 50);
    let mut by_lang: BTreeMap<Lang, usize> = BTreeMap::new();
    for t in &m.titles {
        *by_lang.entry(t.lang).or_default() += 1;
    }
    assert_eq!(by_lang[&Lang::En], 20);
    assert_eq!(by_lang[&Lang::Fr], 12);
    assert_eq!(by_lang[&Lang::Ar], 18);
}

#[test]
fn english_comes_from_standard_ebooks() {
    for t in manifest().titles.iter().filter(|t| t.lang == Lang::En) {
        assert!(
            matches!(t.source, Source::StandardEbooks { .. }),
            "{}: roadmap §3 names Standard Ebooks' CC0 catalog",
            t.id
        );
    }
}

#[test]
fn nothing_is_marked_further_along_than_its_source_allows() {
    // No title is signed off yet; QA starts once ingest exists.
    for t in &manifest().titles {
        assert!(t.status <= Status::Typesetting, "{}: {:?}", t.id, t.status);
    }
}

#[test]
fn every_feature_is_documented() {
    for f in Feature::ALL {
        assert!(
            README.contains(&format!("`{}`", f.name())),
            "catalog/launch/README.md does not describe `{}`",
            f.name()
        );
    }
}

#[test]
fn features_table_matches_the_manifest() {
    let m = manifest();
    let mut documented = 0;
    for row in README.lines().filter(|l| l.starts_with("| `")) {
        let cols: Vec<&str> = row.split('|').map(str::trim).collect();
        let name = cols[1].trim_matches('`');
        let Some(feature) = Feature::ALL.into_iter().find(|f| f.name() == name) else {
            continue; // another table (statuses)
        };
        // ["", feature, needs, status, titles, ""]
        assert_eq!(cols.len(), 6, "malformed row: {row}");
        documented += 1;
        let listed: Vec<&str> = cols[4]
            .split(", ")
            .map(|s| s.trim_matches('`'))
            .filter(|s| !s.is_empty())
            .collect();
        let actual: Vec<&str> = m
            .titles
            .iter()
            .filter(|t| t.features.contains(&feature))
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(listed, actual, "README row for `{name}`");
    }
    assert_eq!(documented, Feature::ALL.len());
}
