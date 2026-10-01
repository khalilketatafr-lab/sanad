//! The launch catalog manifest (`catalog/launch/titles.toml`): which titles
//! Atelier ingests, from which source, and why each is free to publish.
//!
//! [`Manifest::validate`] enforces the rules in `catalog/launch/README.md`:
//!
//! - **EU and most of MENA (life + 70):** every creator (author, translator,
//!   illustrator, editor of the base edition) died at least 71 years before
//!   `rights_as_of`, so the term ran out on 1 January. French *mort pour la
//!   France* authors get 30 more years. Anonymous works: publication + 70.
//! - **US:** the text as distributed (a translation: the translation) was
//!   first published at least 96 years before `rights_as_of`.
//! - **No licence conditions:** sources are CC0, public-domain texts, or our
//!   own transcriptions. CC BY / BY-SA 4.0 texts are out: their "No
//!   downstream restrictions" clause (BY §2(a)(5)(B), BY-SA §2(a)(5)(C))
//!   forbids applying effective technological measures, which P1 delivery
//!   is.
//! - **Provenance before typesetting:** a title past `collation` from a
//!   transcription that is not already a collated edition (Gutenberg,
//!   Wikisource) names the public-domain print edition it was collated
//!   against.

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::qa::Lang;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    /// The year rights are evaluated for: the first year any title ships.
    pub rights_as_of: i32,
    #[serde(rename = "title")]
    pub titles: Vec<Title>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    /// `<lang>-<author>-<work>`, lowercase ASCII: the edition's catalog key.
    pub id: String,
    /// In the original script.
    pub title: String,
    /// English title, for non-English works.
    #[serde(default)]
    pub title_en: Option<String>,
    pub lang: Lang,
    #[serde(default)]
    pub creators: Vec<Creator>,
    /// No known author (rights run from publication).
    #[serde(default)]
    pub anonymous: bool,
    /// First publication of the text as distributed. Translations: the
    /// translation's. Works composed before print: the approximate year of
    /// composition (CE).
    pub published: i32,
    pub status: Status,
    #[serde(default)]
    pub features: Vec<Feature>,
    pub source: Source,
    #[serde(default)]
    pub base_edition: Option<BaseEdition>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Creator {
    /// As the edition prints it (Arabic script for Arabic titles).
    pub name: String,
    /// Latin transliteration, for names in another script.
    #[serde(default)]
    pub name_en: Option<String>,
    pub role: Role,
    /// Year of death (CE; approximate for pre-modern creators).
    pub died: i32,
    /// French *mort pour la France*: +30 years in France (CPI L123-10).
    #[serde(default)]
    pub mort_pour_la_france: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Author,
    Translator,
    Illustrator,
    Editor,
}

/// Where a title is in the pipeline, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// No complete clean text yet: transcription or proofreading needed.
    Sourcing,
    /// Complete text located; collate against a public-domain print edition.
    Collation,
    /// Text final; Atelier ingest and design.
    Typesetting,
    /// Automated checks pass; manual checklist in progress.
    Qa,
    /// Signed off.
    Ready,
}

/// Structures that need engine support beyond running prose. The capability
/// each needs, and whether it exists yet, is tabulated in
/// `catalog/launch/README.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    Verse,
    VerseHemistich,
    Drama,
    Footnotes,
    Illustrations,
    ShapedText,
    Epigraphs,
    Epistolary,
    Dialect,
    NestedQuotation,
    Long,
    LongParagraphs,
    Vocalized,
    SectionBreaks,
    NumberedSections,
}

impl Feature {
    pub const ALL: [Self; 15] = [
        Self::Verse,
        Self::VerseHemistich,
        Self::Drama,
        Self::Footnotes,
        Self::Illustrations,
        Self::ShapedText,
        Self::Epigraphs,
        Self::Epistolary,
        Self::Dialect,
        Self::NestedQuotation,
        Self::Long,
        Self::LongParagraphs,
        Self::Vocalized,
        Self::SectionBreaks,
        Self::NumberedSections,
    ];

    /// The manifest spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Verse => "verse",
            Self::VerseHemistich => "verse-hemistich",
            Self::Drama => "drama",
            Self::Footnotes => "footnotes",
            Self::Illustrations => "illustrations",
            Self::ShapedText => "shaped-text",
            Self::Epigraphs => "epigraphs",
            Self::Epistolary => "epistolary",
            Self::Dialect => "dialect",
            Self::NestedQuotation => "nested-quotation",
            Self::Long => "long",
            Self::LongParagraphs => "long-paragraphs",
            Self::Vocalized => "vocalized",
            Self::SectionBreaks => "section-breaks",
            Self::NumberedSections => "numbered-sections",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    /// A Standard Ebooks edition (CC0 dedication; proofread and collated by
    /// Standard Ebooks against page scans).
    StandardEbooks { url: String },
    /// A Project Gutenberg ebook (public domain in the US; the Gutenberg
    /// trademark is dropped on ingest).
    Gutenberg { ebook: u32 },
    /// A Wikisource page. Only the public-domain text is used, never
    /// wiki-original annotations or translations (CC BY-SA).
    Wikisource { wiki: Lang, page: String },
}

impl Source {
    #[must_use]
    pub fn url(&self) -> String {
        match self {
            Self::StandardEbooks { url } => url.clone(),
            Self::Gutenberg { ebook } => format!("https://www.gutenberg.org/ebooks/{ebook}"),
            Self::Wikisource { wiki, page } => {
                format!(
                    "https://{}.wikisource.org/wiki/{}",
                    wiki.tag(),
                    page.replace(' ', "_")
                )
            }
        }
    }

    /// Whether the source is itself collated against print, so no separate
    /// base edition is needed before typesetting.
    #[must_use]
    pub const fn is_collated(&self) -> bool {
        matches!(self, Self::StandardEbooks { .. })
    }
}

/// The public-domain print edition a transcription was collated against.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseEdition {
    /// Publisher, place and year as printed.
    pub citation: String,
    pub year: i32,
    /// A page-scan URL (Internet Archive, HathiTrust, Gallica…).
    #[serde(default)]
    pub scan: Option<String>,
}

/// One rule a title breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub rule: &'static str,
    pub detail: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.id, self.rule, self.detail)
    }
}

/// Last year of death whose life + 70 (+30 for *mort pour la France*) term
/// has expired by 1 January of `as_of`.
#[must_use]
pub const fn last_free_death_year(as_of: i32, mort_pour_la_france: bool) -> i32 {
    as_of - 71 - if mort_pour_la_france { 30 } else { 0 }
}

/// Last US publication year in the public domain on 1 January of `as_of`
/// (95-year term for works published 1927–1977).
#[must_use]
pub const fn last_free_us_year(as_of: i32) -> i32 {
    as_of - 96
}

fn valid_id(id: &str, lang: Lang) -> bool {
    id.strip_prefix(lang.tag())
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| {
            !rest.is_empty()
                && !rest.starts_with('-')
                && !rest.ends_with('-')
                && !rest.contains("--")
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

impl Manifest {
    pub fn parse(toml_text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_text)
    }

    /// Every rule every title breaks; empty means publishable as listed.
    #[must_use]
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        if self.schema != SCHEMA {
            issues.push(Issue {
                id: "<manifest>".into(),
                rule: "schema",
                detail: format!("schema {} (expected {SCHEMA})", self.schema),
            });
        }
        let mut seen = BTreeSet::new();
        for t in &self.titles {
            let mut issue = |rule: &'static str, detail: String| {
                issues.push(Issue {
                    id: t.id.clone(),
                    rule,
                    detail,
                });
            };
            if !seen.insert(t.id.as_str()) {
                issue("unique id", "duplicate".into());
            }
            if !valid_id(&t.id, t.lang) {
                issue(
                    "id format",
                    format!("expected {}-<author>-<work>", t.lang.tag()),
                );
            }
            if t.lang != Lang::En && t.title_en.is_none() {
                issue(
                    "title_en",
                    "non-English titles carry an English title".into(),
                );
            }
            self.check_rights(t, &mut issue);
            check_source(t, &mut issue);
            let unique: BTreeSet<_> = t.features.iter().collect();
            if unique.len() != t.features.len() {
                issue("features", "listed twice".into());
            }
        }
        issues
    }

    fn check_rights(&self, t: &Title, issue: &mut impl FnMut(&'static str, String)) {
        let as_of = self.rights_as_of;
        if t.published > as_of {
            issue(
                "published",
                format!("{} is after rights_as_of", t.published),
            );
        }
        match (t.anonymous, t.creators.is_empty()) {
            (true, false) => issue("anonymous", "anonymous titles list no creators".into()),
            (false, true) => issue("creators", "name the creators or mark anonymous".into()),
            (true, true) if t.published > as_of - 71 => issue(
                "life + 70",
                format!(
                    "anonymous, published {}: protected until {}",
                    t.published,
                    t.published + 70
                ),
            ),
            _ => {}
        }
        for c in &t.creators {
            if c.died > last_free_death_year(as_of, c.mort_pour_la_france) {
                issue(
                    "life + 70",
                    format!(
                        "{} ({:?}) died {}{}: protected in {as_of}",
                        c.name,
                        c.role,
                        c.died,
                        if c.mort_pour_la_france {
                            ", mort pour la France"
                        } else {
                            ""
                        }
                    ),
                );
            }
        }
        if t.published > last_free_us_year(as_of) {
            issue(
                "US 95 years",
                format!("published {}: protected in the US in {as_of}", t.published),
            );
        }
        if let Some(b) = &t.base_edition
            && b.year > last_free_us_year(as_of)
        {
            issue(
                "base edition",
                format!(
                    "{} ({}) is too recent to be public domain",
                    b.citation, b.year
                ),
            );
        }
    }
}

fn check_source(t: &Title, issue: &mut impl FnMut(&'static str, String)) {
    match &t.source {
        Source::StandardEbooks { url } => {
            if t.lang != Lang::En {
                issue("source", "Standard Ebooks publishes English only".into());
            }
            if !url.starts_with("https://standardebooks.org/ebooks/") {
                issue("source", format!("not a Standard Ebooks URL: {url}"));
            }
        }
        Source::Gutenberg { ebook } => {
            if *ebook == 0 {
                issue("source", "Gutenberg ebook number".into());
            }
        }
        Source::Wikisource { wiki, page } => {
            if *wiki != t.lang {
                issue(
                    "source",
                    format!("{} Wikisource for a {} title", wiki.tag(), t.lang.tag()),
                );
            }
            if page.trim().is_empty() {
                issue("source", "Wikisource page".into());
            }
        }
    }
    if t.status >= Status::Typesetting && !t.source.is_collated() && t.base_edition.is_none() {
        issue(
            "provenance",
            format!("{:?} without a base edition: collate first", t.status),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn manifest(as_of: i32, title: &str) -> Manifest {
        Manifest::parse(&format!(
            "schema = 1\nrights_as_of = {as_of}\n\n[[title]]\n{title}"
        ))
        .unwrap()
    }

    fn rules(m: &Manifest) -> Vec<&'static str> {
        m.validate().iter().map(|i| i.rule).collect()
    }

    const CHERI: &str = r#"
id = "fr-colette-cheri"
title = "Chéri"
title_en = "Chéri"
lang = "fr"
creators = [{ name = "Colette", role = "author", died = 1954 }]
published = 1920
status = "collation"
source = { kind = "gutenberg", ebook = 6484 }
"#;

    #[test]
    fn terms_run_to_the_end_of_the_calendar_year() {
        assert_eq!(last_free_death_year(2026, false), 1955);
        assert_eq!(last_free_death_year(2026, true), 1925);
        assert_eq!(last_free_us_year(2026), 1930);
        // Colette died in 1954: free in the EU from 1 January 2025.
        assert!(rules(&manifest(2025, CHERI)).is_empty());
        assert_eq!(rules(&manifest(2024, CHERI)), vec!["life + 70"]);
    }

    #[test]
    fn mort_pour_la_france_adds_thirty_years() {
        // Alain-Fournier (d. 1914, mort pour la France): 70 + 30 years ran
        // out at the end of 2014.
        let meaulnes = r#"
id = "fr-alain-fournier-le-grand-meaulnes"
title = "Le Grand Meaulnes"
title_en = "The Wanderer"
lang = "fr"
creators = [{ name = "Alain-Fournier", role = "author", died = 1914, mort_pour_la_france = true }]
published = 1913
status = "sourcing"
source = { kind = "gutenberg", ebook = 5781 }
"#;
        assert!(rules(&manifest(2015, meaulnes)).is_empty());
        assert_eq!(rules(&manifest(2014, meaulnes)), vec!["life + 70"]);
        // A 1930 death: free in 2001 without the extension, 2031 with it.
        let later = meaulnes.replace("died = 1914", "died = 1930");
        assert_eq!(rules(&manifest(2026, &later)), vec!["life + 70"]);
        let plain = later.replace(", mort_pour_la_france = true", "");
        assert!(rules(&manifest(2026, &plain)).is_empty());
    }

    #[test]
    fn us_term_follows_the_translation() {
        let garnett = |year: i32| {
            format!(
                r#"
id = "en-tolstoy-anna-karenina"
title = "Anna Karenina"
lang = "en"
creators = [
  {{ name = "Leo Tolstoy", role = "author", died = 1910 }},
  {{ name = "Constance Garnett", role = "translator", died = 1946 }},
]
published = {year}
status = "typesetting"
source = {{ kind = "standard-ebooks", url = "https://standardebooks.org/ebooks/leo-tolstoy/anna-karenina/constance-garnett" }}
"#
            )
        };
        assert!(rules(&manifest(2026, &garnett(1901))).is_empty());
        assert_eq!(rules(&manifest(2026, &garnett(1931))), vec!["US 95 years"]);
    }

    #[test]
    fn anonymous_works_run_from_publication() {
        let nights = r#"
id = "ar-alf-layla-sindbad"
title = "السندباد البحري"
title_en = "Sindbad the Sailor"
lang = "ar"
anonymous = true
published = 1835
status = "collation"
source = { kind = "wikisource", wiki = "ar", page = "ألف ليلة وليلة/السندباد البحري" }
"#;
        let m = manifest(2026, nights);
        assert!(rules(&m).is_empty());
        assert_eq!(
            m.titles[0].source.url(),
            "https://ar.wikisource.org/wiki/ألف_ليلة_وليلة/السندباد_البحري"
        );
        assert_eq!(
            rules(&manifest(2026, &nights.replace("anonymous = true\n", ""))),
            vec!["creators"]
        );
    }

    #[test]
    fn typesetting_needs_provenance_unless_the_source_is_collated() {
        let typesetting = CHERI.replace("\"collation\"", "\"typesetting\"");
        assert_eq!(rules(&manifest(2026, &typesetting)), vec!["provenance"]);
        let with_base = format!(
            "{typesetting}base_edition = {{ citation = \"Paris, Arthème Fayard, 1920\", year = 1920 }}\n"
        );
        assert!(rules(&manifest(2026, &with_base)).is_empty());
    }

    #[test]
    fn source_and_id_rules() {
        let bad = CHERI
            .replace("fr-colette-cheri", "en-Colette--cheri")
            .replace("title_en = \"Chéri\"\n", "");
        assert_eq!(rules(&manifest(2026, &bad)), vec!["id format", "title_en"]);
        let se_fr = CHERI.replace(
            "{ kind = \"gutenberg\", ebook = 6484 }",
            "{ kind = \"standard-ebooks\", url = \"https://standardebooks.org/ebooks/colette/cheri\" }",
        );
        assert_eq!(rules(&manifest(2026, &se_fr)), vec!["source"]);
        let unknown = CHERI.replace(
            "status = \"collation\"",
            "status = \"collation\"\nisbn = \"x\"",
        );
        assert!(
            Manifest::parse(&format!(
                "schema = 1\nrights_as_of = 2026\n[[title]]\n{unknown}"
            ))
            .is_err()
        );
    }
}
