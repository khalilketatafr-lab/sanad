//! Typographic QA, the automated half of `docs/qa/typographic-qa.md`.
//!
//! Three layers, every finding keyed by the checklist's `TQ-*` id:
//!
//! - **text** ([`lint_paragraph`], [`lint_book`]): the source before shaping.
//!   Whatever these catch, typesetting cannot fix (straight quotes, French
//!   spacing, Persian letters in Arabic, stray tatweel).
//! - **coverage**: every character against the face the [`Typesetter`]
//!   actually picks for it, fallback included ([`Typesetter::uncovered`]).
//! - **layout** ([`check_layout`]): each paragraph set by the Compositor at
//!   the [`READING_MEASURES`] readers really get, from a phone at large text
//!   to a tablet, then inspected line by line.
//!
//! Findings point at source byte offsets for editors. This is ingest
//! tooling: reports quote the text and never leave the server (P1).

use std::collections::BTreeMap;

use sanad_compositor::item::{ItemParams, RunView};
use sanad_compositor::layout::{GlyphKind, LineLayout, Measure, layout_paragraph_with};
use sanad_compositor::linebreak::{BreakParams, Quality};
use serde::{Deserialize, Serialize, Serializer};
use unicode_normalization::is_nfc;

use crate::script::is_arabic;
use crate::shape::{ShapeError, Typesetter};

/// A title's language: picks the language-specific checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    Fr,
    Ar,
}

impl Lang {
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "en" => Some(Self::En),
            "fr" => Some(Self::Fr),
            "ar" => Some(Self::Ar),
            _ => None,
        }
    }

    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
            Self::Ar => "ar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Wrong output for every reader (missing glyphs, broken text). Never shipped.
    Blocker,
    /// A typographic error a careful reader notices. Fixed or waived before sign-off.
    Major,
    /// A judgment call, reviewed in context by the editor.
    Minor,
}

/// One automated check of the checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Check {
    Nfc,
    Invisible,
    Coverage,
    Whitespace,
    MarkupResidue,
    StraightQuotes,
    AsciiDashes,
    AsciiEllipsis,
    FrenchSpacing,
    FrenchQuotes,
    FrenchLigatures,
    LatinPunctuation,
    Tatweel,
    ArabicSpacing,
    PersianForms,
    MixedDigits,
    StackedMarks,
    Overfull,
    Loose,
    HyphenLadder,
    PenultimateHyphen,
    Runt,
    KashidaDensity,
    KashidaLastLine,
}

impl Check {
    pub const ALL: [Self; 24] = [
        Self::Nfc,
        Self::Invisible,
        Self::Coverage,
        Self::Whitespace,
        Self::MarkupResidue,
        Self::StraightQuotes,
        Self::AsciiDashes,
        Self::AsciiEllipsis,
        Self::FrenchSpacing,
        Self::FrenchQuotes,
        Self::FrenchLigatures,
        Self::LatinPunctuation,
        Self::Tatweel,
        Self::ArabicSpacing,
        Self::PersianForms,
        Self::MixedDigits,
        Self::StackedMarks,
        Self::Overfull,
        Self::Loose,
        Self::HyphenLadder,
        Self::PenultimateHyphen,
        Self::Runt,
        Self::KashidaDensity,
        Self::KashidaLastLine,
    ];

    /// The checklist id (`docs/qa/typographic-qa.md`).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Nfc => "TQ-S1",
            Self::Invisible => "TQ-S2",
            Self::Coverage => "TQ-S3",
            Self::Whitespace => "TQ-S4",
            Self::MarkupResidue => "TQ-S5",
            Self::StraightQuotes => "TQ-L1",
            Self::AsciiDashes => "TQ-L2",
            Self::AsciiEllipsis => "TQ-L3",
            Self::FrenchSpacing => "TQ-F1",
            Self::FrenchQuotes => "TQ-F2",
            Self::FrenchLigatures => "TQ-F3",
            Self::LatinPunctuation => "TQ-A1",
            Self::Tatweel => "TQ-A2",
            Self::ArabicSpacing => "TQ-A3",
            Self::PersianForms => "TQ-A4",
            Self::MixedDigits => "TQ-A5",
            Self::StackedMarks => "TQ-A6",
            Self::Overfull => "TQ-P1",
            Self::Loose => "TQ-P2",
            Self::HyphenLadder => "TQ-P3",
            Self::PenultimateHyphen => "TQ-P4",
            Self::Runt => "TQ-P5",
            Self::KashidaDensity => "TQ-P6",
            Self::KashidaLastLine => "TQ-P7",
        }
    }

    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::Nfc | Self::Invisible | Self::Coverage | Self::KashidaLastLine => {
                Severity::Blocker
            }
            Self::MarkupResidue
            | Self::StraightQuotes
            | Self::AsciiDashes
            | Self::FrenchSpacing
            | Self::LatinPunctuation
            | Self::Tatweel
            | Self::ArabicSpacing
            | Self::PersianForms
            | Self::StackedMarks
            | Self::Overfull => Severity::Major,
            Self::Whitespace
            | Self::AsciiEllipsis
            | Self::FrenchQuotes
            | Self::FrenchLigatures
            | Self::MixedDigits
            | Self::Loose
            | Self::HyphenLadder
            | Self::PenultimateHyphen
            | Self::Runt
            | Self::KashidaDensity => Severity::Minor,
        }
    }
}

impl Serialize for Check {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

/// A reading measure, in ems of the paragraph's base face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingMeasure {
    pub name: &'static str,
    pub ems: f32,
}

/// The measures every paragraph is set at: a phone at the largest text size,
/// a phone at the default size, and a tablet column. Knuth–Plass output
/// depends only on the measure-to-em ratio, so these cover every font size.
pub const READING_MEASURES: [ReadingMeasure; 3] = [
    ReadingMeasure {
        name: "narrow",
        ems: 15.0,
    },
    ReadingMeasure {
        name: "phone",
        ems: 19.0,
    },
    ReadingMeasure {
        name: "wide",
        ems: 32.0,
    },
];

/// A last line shorter than this many ems reads as a runt: shorter than the
/// next paragraph's indent (the golden pages' `PageSpec::indent_em`).
pub const RUNT_EMS: f32 = 1.5;
/// More elongations than this on one line read as stretched text: Naskh
/// practice favors a few long kashidas over many short ones.
pub const MAX_KASHIDAS_PER_LINE: usize = 3;
/// More consecutive hyphenated lines than this form a ladder.
pub const MAX_HYPHEN_LADDER: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub check: Check,
    /// Byte offset in the source text.
    pub byte: usize,
    /// The reading measure, for layout checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure: Option<&'static str>,
    pub note: String,
}

impl Finding {
    fn text(check: Check, byte: usize, note: impl Into<String>) -> Self {
        Self {
            check,
            byte,
            measure: None,
            note: note.into(),
        }
    }
}

/// Paragraphs of a plain-text source: blank-line separated and trimmed,
/// each with its byte offset in `source`. The model the golden pages and the
/// reflow fixture use.
pub fn paragraphs(source: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    let base = source.as_ptr() as usize;
    source
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(move |p| (p.as_ptr() as usize - base, p))
}

fn codepoint(c: char) -> String {
    format!("U+{:04X}", u32::from(c))
}

const NBSP: char = '\u{00A0}';
const NNBSP: char = '\u{202F}';
const fn is_fixed_space(c: char) -> bool {
    matches!(c, NBSP | NNBSP)
}
const fn is_arabic_mark(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}')
}
fn is_arabic_letter(c: char) -> bool {
    is_arabic(c) && c.is_alphabetic()
}
/// Arabic-script punctuation that takes no space before it.
const fn is_arabic_punct(c: char) -> bool {
    matches!(c, '\u{060C}' | '\u{061B}' | '\u{061F}')
}

/// Source-format leftovers: wikitext (Wikisource), HTML, entities.
const MARKUP: [&str; 10] = [
    "[[", "]]", "{{", "}}", "<ref", "</", "<br", "&nbsp;", "&amp;", "&#",
];

/// Words a French source should spell with œ (cœur, sœur, œuvre…), by stem.
const OE_STEMS: [&str; 12] = [
    "coeur", "soeur", "oeuvr", "oeil", "boeuf", "voeu", "noeud", "oeuf", "moeurs", "choeur",
    "oesophag", "oecum",
];

/// Text checks for one paragraph (`text` starts at byte `base` of the source).
pub fn lint_paragraph(lang: Lang, base: usize, text: &str, out: &mut Vec<Finding>) {
    if !is_nfc(text) {
        out.push(Finding::text(Check::Nfc, base, "paragraph is not NFC"));
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let mut quote_depth = 0usize;
    let mut marks_on_base = 0usize;
    // Last character that is neither a space nor a mark.
    let mut prev_strong: Option<char> = None;
    for (i, &(b, c)) in chars.iter().enumerate() {
        let byte = base + b;
        let prev = i.checked_sub(1).and_then(at);
        let next = at(i + 1);
        lint_source_char(c, prev, byte, out);
        lint_latin_char(c, prev, next, byte, out);
        if lang == Lang::Fr {
            lint_french_char(c, prev, next, byte, &mut quote_depth, out);
        }
        // Arabic checks key off the surrounding letters, so an Arabic
        // quotation in an English book is checked too.
        lint_arabic_char(lang, c, prev, prev_strong, next, byte, out);
        if !c.is_whitespace() && !is_arabic_mark(c) {
            prev_strong = Some(c);
        }
        if is_arabic_mark(c) {
            marks_on_base += 1;
            if prev == Some(c) {
                out.push(Finding::text(
                    Check::StackedMarks,
                    byte,
                    format!("{} doubled", codepoint(c)),
                ));
            } else if marks_on_base == 4 {
                out.push(Finding::text(
                    Check::StackedMarks,
                    byte,
                    "four or more marks on one letter",
                ));
            }
        } else {
            marks_on_base = 0;
        }
    }
    if lang == Lang::Fr {
        lint_oe_ligatures(base, text, out);
    }
    lint_markup(base, text, out);
}

/// Wikitext, HTML and entity leftovers, and `_emphasis_` underscores from
/// plain-text transcriptions (an underscore touching a letter).
fn lint_markup(base: usize, text: &str, out: &mut Vec<Finding>) {
    for m in MARKUP {
        for (b, _) in text.match_indices(m) {
            out.push(Finding::text(
                Check::MarkupResidue,
                base + b,
                format!("\"{m}\""),
            ));
        }
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(b, c)) in chars.iter().enumerate() {
        let touches_letter = |j: Option<usize>| {
            j.and_then(|j| chars.get(j))
                .is_some_and(|&(_, n)| n.is_alphabetic())
        };
        if c == '_' && (touches_letter(i.checked_sub(1)) || touches_letter(Some(i + 1))) {
            out.push(Finding::text(
                Check::MarkupResidue,
                base + b,
                "_ emphasis marker",
            ));
        }
    }
}

fn lint_source_char(c: char, prev: Option<char>, byte: usize, out: &mut Vec<Finding>) {
    match c {
        '\u{FB00}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFE}' => out.push(Finding::text(
            Check::Nfc,
            byte,
            format!("presentation form {}", codepoint(c)),
        )),
        '\n' | '\r' => out.push(Finding::text(
            Check::Invisible,
            byte,
            "line break inside a paragraph",
        )),
        '\t' => out.push(Finding::text(Check::Whitespace, byte, "tab")),
        '\u{00AD}'
        | '\u{200B}'
        | '\u{FEFF}'
        | '\u{FFFD}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => {
            out.push(Finding::text(Check::Invisible, byte, codepoint(c)));
        }
        _ if c.is_control() => out.push(Finding::text(Check::Invisible, byte, codepoint(c))),
        ' ' if prev == Some(' ') => {
            out.push(Finding::text(Check::Whitespace, byte, "double space"));
        }
        _ => {}
    }
}

fn lint_latin_char(
    c: char,
    prev: Option<char>,
    next: Option<char>,
    byte: usize,
    out: &mut Vec<Finding>,
) {
    match c {
        '"' => out.push(Finding::text(Check::StraightQuotes, byte, "straight \"")),
        '\'' => out.push(Finding::text(Check::StraightQuotes, byte, "straight '")),
        '-' if next == Some('-') && prev != Some('-') => {
            out.push(Finding::text(Check::AsciiDashes, byte, "\"--\" for a dash"));
        }
        '-' if prev == Some(' ') && next == Some(' ') => {
            out.push(Finding::text(
                Check::AsciiDashes,
                byte,
                "spaced hyphen for a dash",
            ));
        }
        '.' if next == Some('.') && prev != Some('.') => {
            out.push(Finding::text(Check::AsciiEllipsis, byte, "\"...\" for …"));
        }
        _ => {}
    }
}

fn lint_french_char(
    c: char,
    prev: Option<char>,
    next: Option<char>,
    byte: usize,
    quote_depth: &mut usize,
    out: &mut Vec<Finding>,
) {
    // Imprimerie nationale: a fixed thin space before ; ! ? and », a fixed
    // word space before :, and after «. A breakable space would let the
    // punctuation start a line; no space at all is simply wrong.
    let wants_space_before = match c {
        ';' | '!' | '?' => !matches!(prev, None | Some('?' | '!' | '…' | '.' | '(' | '[')),
        ':' => {
            let clock = prev.is_some_and(|p| p.is_ascii_digit())
                && next.is_some_and(|n| n.is_ascii_digit());
            prev.is_some() && !clock
        }
        '»' => true,
        _ => false,
    };
    if wants_space_before && !prev.is_some_and(is_fixed_space) {
        let what = if prev == Some(' ') {
            "breakable space"
        } else {
            "no space"
        };
        out.push(Finding::text(
            Check::FrenchSpacing,
            byte,
            format!("{what} before {c}"),
        ));
    }
    if c == '«' && !next.is_some_and(is_fixed_space) {
        out.push(Finding::text(
            Check::FrenchSpacing,
            byte,
            "no fixed space after «",
        ));
    }
    match c {
        '«' => *quote_depth += 1,
        '»' => *quote_depth = quote_depth.saturating_sub(1),
        '\u{201C}' if *quote_depth == 0 => out.push(Finding::text(
            Check::FrenchQuotes,
            byte,
            "“ outside « » (first-level quotes are guillemets)",
        )),
        _ => {}
    }
}

fn lint_arabic_char(
    lang: Lang,
    c: char,
    prev: Option<char>,
    prev_strong: Option<char>,
    next: Option<char>,
    byte: usize,
    out: &mut Vec<Finding>,
) {
    let after_arabic = prev_strong.is_some_and(is_arabic_letter);
    match c {
        '\u{0640}' => out.push(Finding::text(
            Check::Tatweel,
            byte,
            "tatweel in the source (kashida is the Compositor's)",
        )),
        ',' | ';' | '?' if after_arabic => out.push(Finding::text(
            Check::LatinPunctuation,
            byte,
            format!("Latin {c} after Arabic text"),
        )),
        // Persian/Urdu letters that look like (or shape unlike) their Arabic
        // counterparts and break search: ک ی ہ and Extended Arabic-Indic digits.
        '\u{06A9}' | '\u{06CC}' | '\u{06C1}' | '\u{06F0}'..='\u{06F9}' if lang == Lang::Ar => {
            out.push(Finding::text(
                Check::PersianForms,
                byte,
                format!("Persian form {}", codepoint(c)),
            ));
        }
        _ => {}
    }
    let arabic_context = after_arabic || is_arabic_punct(c);
    if arabic_context && (is_arabic_punct(c) || matches!(c, '.' | ':' | '!')) && prev == Some(' ') {
        out.push(Finding::text(
            Check::ArabicSpacing,
            byte,
            format!("space before {c}"),
        ));
    }
    if is_arabic_punct(c) && next.is_some_and(is_arabic_letter) {
        out.push(Finding::text(
            Check::ArabicSpacing,
            byte,
            format!("no space after {c}"),
        ));
    }
}

fn lint_oe_ligatures(base: usize, text: &str, out: &mut Vec<Finding>) {
    let start = text.as_ptr() as usize;
    for word in text.split(|c: char| !c.is_alphabetic()) {
        let lower = word.to_lowercase();
        if OE_STEMS.iter().any(|s| lower.contains(s)) {
            out.push(Finding::text(
                Check::FrenchLigatures,
                base + (word.as_ptr() as usize - start),
                format!("\"{word}\" without œ"),
            ));
        }
    }
}

/// Book-level text checks: digit sets mixed across Arabic paragraphs.
pub fn lint_book(source: &str, out: &mut Vec<Finding>) {
    let mut first_western: Option<usize> = None;
    let mut first_indic: Option<usize> = None;
    for (base, p) in paragraphs(source) {
        let arabic = p.chars().filter(|&c| is_arabic_letter(c)).count();
        let latin = p
            .chars()
            .filter(|&c| c.is_alphabetic() && !is_arabic(c))
            .count();
        if arabic <= latin {
            continue;
        }
        for (b, c) in p.char_indices() {
            if c.is_ascii_digit() {
                first_western.get_or_insert(base + b);
            } else if matches!(c, '\u{0660}'..='\u{0669}') {
                first_indic.get_or_insert(base + b);
            }
        }
    }
    if let (Some(w), Some(i)) = (first_western, first_indic) {
        out.push(Finding::text(
            Check::MixedDigits,
            w.max(i),
            "Arabic text uses both Western and Arabic-Indic digits",
        ));
    }
}

/// Lines that end with an inserted hyphen.
fn hyphenated(line: &LineLayout) -> bool {
    line.glyphs.iter().any(|g| g.kind == GlyphKind::Hyphen)
}

/// Sets one paragraph at every reading measure and inspects the lines.
pub fn check_layout(
    ts: &Typesetter<'_>,
    base: usize,
    text: &str,
    out: &mut Vec<Finding>,
) -> Result<(), ShapeError> {
    let para = ts.shape_paragraph(text)?;
    let views: Vec<RunView<'_>> = para
        .runs
        .iter()
        .map(|r| r.view(ts.specials(r.font)))
        .collect();
    let base_font = if para.base_level % 2 == 1 {
        ts.arabic_font()
    } else {
        ts.latin_font()
    };
    let em = ts.font(base_font).size_px;
    for m in READING_MEASURES {
        let layout = layout_paragraph_with(
            &views,
            Measure {
                width: m.ems * em,
                first_indent: 0.0,
            },
            para.base_level,
            &ItemParams::default(),
            &BreakParams::default(),
        );
        let mut push = |check: Check, note: String| {
            out.push(Finding {
                check,
                byte: base,
                measure: Some(m.name),
                note,
            });
        };
        match layout.quality {
            Quality::Optimal => {}
            Quality::Relaxed => push(Check::Loose, "needed the relaxed pass".into()),
            Quality::Emergency => push(Check::Overfull, "a line could not be set".into()),
        }
        let lines = &layout.lines;
        let mut ladder = 0usize;
        for (n, line) in lines.iter().enumerate() {
            ladder = if hyphenated(line) { ladder + 1 } else { 0 };
            if ladder == MAX_HYPHEN_LADDER + 1 {
                push(
                    Check::HyphenLadder,
                    format!("lines {}–{} all hyphenated", n + 1 - ladder + 1, n + 1),
                );
            }
            let kashidas = line
                .glyphs
                .iter()
                .filter(|g| g.kind == GlyphKind::Kashida)
                .count();
            if kashidas > MAX_KASHIDAS_PER_LINE {
                push(
                    Check::KashidaDensity,
                    format!("line {}: {kashidas} kashidas", n + 1),
                );
            }
            if kashidas > 0 && n + 1 == lines.len() {
                push(
                    Check::KashidaLastLine,
                    "kashida on the ragged last line".into(),
                );
            }
        }
        if lines.len() >= 2 {
            if hyphenated(&lines[lines.len() - 2]) {
                push(
                    Check::PenultimateHyphen,
                    "last full line ends in a hyphen".into(),
                );
            }
            let last = &lines[lines.len() - 1];
            if last.advance < RUNT_EMS * em {
                push(
                    Check::Runt,
                    format!("last line {:.1} em", last.advance / em),
                );
            }
        }
    }
    Ok(())
}

/// What [`check_book`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QaOptions {
    pub layout: bool,
}

impl Default for QaOptions {
    fn default() -> Self {
        Self { layout: true }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub lang: Lang,
    pub paragraphs: usize,
    pub characters: usize,
    /// Findings per check id.
    pub counts: BTreeMap<&'static str, usize>,
    pub findings: Vec<Finding>,
    /// Hyphenation patterns in effect (`None`: Latin text set unhyphenated).
    pub hyphenation: Option<String>,
}

impl Report {
    #[must_use]
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.check.severity() == severity)
            .count()
    }

    /// Sign-off rule: no blocker and no unwaived major finding.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.count(Severity::Blocker) == 0 && self.count(Severity::Major) == 0
    }
}

/// Runs every automated check over a plain-text source (see [`paragraphs`]).
pub fn check_book(
    ts: &Typesetter<'_>,
    lang: Lang,
    source: &str,
    options: QaOptions,
) -> Result<Report, ShapeError> {
    let mut findings = Vec::new();
    let mut count = 0usize;
    for (base, p) in paragraphs(source) {
        count += 1;
        lint_paragraph(lang, base, p, &mut findings);
        for (b, c) in ts.uncovered(p) {
            findings.push(Finding::text(
                Check::Coverage,
                base + b,
                format!("{} {c} has no glyph", codepoint(c)),
            ));
        }
        if options.layout {
            check_layout(ts, base, p, &mut findings)?;
        }
    }
    lint_book(source, &mut findings);
    findings.sort_by_key(|f| (f.byte, f.check));
    let mut counts = BTreeMap::new();
    for f in &findings {
        *counts.entry(f.check.id()).or_insert(0) += 1;
    }
    Ok(Report {
        lang,
        paragraphs: count,
        characters: source.chars().count(),
        counts,
        findings,
        hyphenation: ts.hyphenation_language().map(|l| format!("{l:?}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(lang: Lang, text: &str) -> Vec<(Check, usize)> {
        let mut out = Vec::new();
        lint_paragraph(lang, 0, text, &mut out);
        out.iter().map(|f| (f.check, f.byte)).collect()
    }

    fn checks(lang: Lang, text: &str) -> Vec<Check> {
        lint(lang, text).into_iter().map(|(c, _)| c).collect()
    }

    #[test]
    fn ids_are_unique_and_well_formed() {
        let mut ids: Vec<_> = Check::ALL.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Check::ALL.len());
        assert!(ids.iter().all(|id| id.starts_with("TQ-") && id.len() == 5));
    }

    #[test]
    fn paragraphs_report_source_offsets() {
        let src = "  One.\n\n\n\nTwo  two.\n\n";
        let ps: Vec<_> = paragraphs(src).collect();
        assert_eq!(ps, vec![(2, "One."), (10, "Two  two.")]);
    }

    #[test]
    fn clean_english_passes() {
        assert!(
            checks(
                Lang::En,
                "“It is a truth universally acknowledged…” — she said, and wasn’t."
            )
            .is_empty()
        );
    }

    #[test]
    fn english_source_errors() {
        assert_eq!(
            lint(Lang::En, "\"Yes--no...\" it's  odd - isn't it"),
            vec![
                (Check::StraightQuotes, 0),
                (Check::AsciiDashes, 4),
                (Check::AsciiEllipsis, 8),
                (Check::StraightQuotes, 11),
                (Check::StraightQuotes, 15),
                (Check::Whitespace, 18),
                (Check::AsciiDashes, 23),
                (Check::StraightQuotes, 28),
            ]
        );
    }

    #[test]
    fn source_hygiene() {
        assert_eq!(checks(Lang::En, "e\u{301}t\u{e9}"), vec![Check::Nfc]);
        assert_eq!(checks(Lang::En, "of\u{FB01}ce"), vec![Check::Nfc]);
        assert_eq!(checks(Lang::En, "soft\u{AD}hyphen"), vec![Check::Invisible]);
        assert_eq!(checks(Lang::En, "mojibake\u{FFFD}"), vec![Check::Invisible]);
        assert_eq!(checks(Lang::En, "wrapped\nline"), vec![Check::Invisible]);
    }

    #[test]
    fn markup_residue() {
        assert_eq!(
            checks(
                Lang::En,
                "See _Mélanges_ and [[Page|link]] {{tpl}}<ref>x</ref>&nbsp;"
            ),
            vec![
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
                Check::MarkupResidue,
            ]
        );
        // A blank to fill in is not emphasis.
        assert_eq!(checks(Lang::En, "Mr. ____ of ____shire").len(), 1);
    }

    #[test]
    fn french_spacing_wants_fixed_spaces() {
        let ok = "«\u{a0}Vraiment\u{202f}? Oui\u{202f}; non\u{202f}! Il dit\u{a0}: «\u{a0}bah\u{202f}»\u{202f}?!\u{a0}» À 10:30.";
        assert!(checks(Lang::Fr, ok).is_empty(), "{:?}", lint(Lang::Fr, ok));
        assert_eq!(
            lint(Lang::Fr, "Vraiment ? Oui; « non »"),
            vec![
                (Check::FrenchSpacing, 9),
                (Check::FrenchSpacing, 14),
                (Check::FrenchSpacing, 16),
                (Check::FrenchSpacing, 23),
            ]
        );
        // The same text in English is fine.
        assert!(checks(Lang::En, "Really? Yes; no!").is_empty());
    }

    #[test]
    fn french_quotes_and_ligatures() {
        assert_eq!(
            checks(Lang::Fr, "Il dit “oui” de bon coeur."),
            vec![Check::FrenchQuotes, Check::FrenchLigatures]
        );
        assert!(
            checks(
                Lang::Fr,
                "«\u{a0}Il dit “oui” de bon cœur, moelle et coexistence.\u{202f}»"
            )
            .is_empty()
        );
    }

    #[test]
    fn arabic_source_errors() {
        assert!(checks(Lang::Ar, "قال: هذا كتابٌ، وهل تقرؤه؟ نعم.").is_empty());
        assert_eq!(
            checks(Lang::Ar, "قال , هذا كتـاب? ک"),
            vec![
                Check::LatinPunctuation,
                Check::Tatweel,
                Check::LatinPunctuation,
                Check::PersianForms,
            ]
        );
        assert_eq!(
            checks(Lang::Ar, "كتاب ، وقلم ؟"),
            vec![Check::ArabicSpacing, Check::ArabicSpacing]
        );
        assert_eq!(checks(Lang::Ar, "كتاب،وقلم"), vec![Check::ArabicSpacing]);
        // Persian letters inside an English book are a quotation, not an error.
        assert!(checks(Lang::En, "ک").is_empty());
    }

    #[test]
    fn stacked_marks() {
        // Canonical (NFC) order: fatha (ccc 30) before shadda (ccc 33).
        let madda = "م\u{64F}د\u{64E}\u{651}ة\u{64C}";
        assert!(
            checks(Lang::Ar, madda).is_empty(),
            "shadda + vowel is normal"
        );
        // Keyboard order (shadda first) is valid Unicode but not NFC.
        assert_eq!(
            checks(Lang::Ar, "م\u{64F}د\u{651}\u{64E}ة\u{64C}"),
            vec![Check::Nfc]
        );
        assert_eq!(checks(Lang::Ar, "كتابًً"), vec![Check::StackedMarks]);
    }

    #[test]
    fn mixed_digits_are_a_book_level_finding() {
        let mut out = Vec::new();
        lint_book("سنة 1813 للميلاد\n\nسنة ١٨١٣ للميلاد", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].check, Check::MixedDigits);
        out.clear();
        lint_book("سنة 1813 للميلاد\n\nIn 1813, ١٨١٣.", &mut out);
        assert!(out.is_empty(), "the Latin paragraph does not count");
    }
}
