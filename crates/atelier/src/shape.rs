//! Ingest stage ②: paragraph text → shaped runs in the Compositor's model.
//!
//! The only place Unicode exists. Text is resolved for bidi (UAX #9),
//! split into runs by font and embedding level, shaped (rustybuzz, a HarfBuzz
//! port), and annotated with everything the client needs to lay the
//! paragraph out without ever seeing text: cluster, word and sentence
//! starts, break opportunities, hyphenation points (Knuth–Liang) and kashida
//! opportunities with priorities (Arabic joining, see [`crate::script`]).
//!
//! Glyphs are emitted in LOGICAL order (RTL runs are reversed back from the
//! shaper's visual order); the Compositor applies UAX #9 L2 per line. Glyph
//! ids here are FONT ids; [`crate::permute`] maps them to edition ids.

use hyphenation::{Hyphenator, Language as HyphenLanguage, Load, Standard};
use rustybuzz::{Direction, Face, Language, Script, UnicodeBuffer};
use sanad_compositor::item::flags;
use thiserror::Error;
use unicode_bidi::BidiInfo;

use crate::script::{Joining, is_arabic, joining, joins_next, joins_prev, kashida_priority};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub u8);

/// A face at a size, with the script and language it sets.
pub struct FontFace<'a> {
    pub face: Face<'a>,
    /// Font size in device px (the em).
    pub size_px: f32,
    pub script: Script,
    pub language: Language,
}

impl core::fmt::Debug for FontFace<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FontFace")
            .field("units_per_em", &self.face.units_per_em())
            .field("size_px", &self.size_px)
            .field("script", &self.script)
            .finish_non_exhaustive()
    }
}

impl FontFace<'_> {
    /// Device px per font unit.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.size_px / self.face.units_per_em() as f32
    }

    fn glyph(&self, c: char) -> Option<(u16, i16)> {
        let gid = self.face.glyph_index(c)?;
        let adv = self.face.glyph_hor_advance(gid)?;
        Some((gid.0, i16::try_from(adv).ok()?))
    }
}

#[derive(Debug, Error)]
pub enum ShapeError {
    #[error("hyphenation dictionary: {0}")]
    Hyphenation(String),
    #[error("{0}: value does not fit the Folio run format")]
    Overflow(&'static str),
    #[error("empty paragraph")]
    Empty,
}

/// One shaped run: one font, one embedding level. Mirrors Folio's `Run`
/// before glyph permutation.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedRun {
    pub font: FontId,
    /// Device px per font unit.
    pub scale: f32,
    pub gids: Vec<u16>,
    pub advances: Vec<i16>,
    pub offsets: Vec<[i16; 2]>,
    pub flags: Vec<u8>,
    pub levels: Vec<u8>,
    pub kashida_priority: Vec<u8>,
    pub kashida_max: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapedParagraph {
    /// Paragraph embedding level (0 LTR, 1 RTL).
    pub base_level: u8,
    pub runs: Vec<ShapedRun>,
}

impl ShapedParagraph {
    pub fn glyph_count(&self) -> usize {
        self.runs.iter().map(|r| r.gids.len()).sum()
    }
}

/// Glyphs the Compositor inserts itself: the hyphen and the tatweel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Specials {
    pub hyphen: Option<(u16, i16)>,
    pub tatweel: Option<(u16, i16)>,
}

/// Fonts and dictionaries for one edition.
pub struct Typesetter<'a> {
    fonts: Vec<FontFace<'a>>,
    latin: FontId,
    arabic: FontId,
    hyphenator: Standard,
    /// Maximum kashida elongation per opportunity, in ems.
    pub kashida_max_em: f32,
}

impl core::fmt::Debug for Typesetter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Typesetter")
            .field("fonts", &self.fonts)
            .field("latin", &self.latin)
            .field("arabic", &self.arabic)
            .finish_non_exhaustive()
    }
}

/// Where a cluster's glyphs ended up: run and glyph range in logical order.
#[derive(Debug, Clone, Copy)]
struct ClusterSpan {
    byte: usize,
    run: usize,
    first: usize,
    last: usize,
}

impl<'a> Typesetter<'a> {
    /// `latin` sets Latin and neutral text in LTR context; `arabic` sets
    /// Arabic and neutrals in RTL context.
    pub fn new(latin: FontFace<'a>, arabic: FontFace<'a>) -> Result<Self, ShapeError> {
        let hyphenator = Standard::from_embedded(HyphenLanguage::EnglishUS)
            .map_err(|e| ShapeError::Hyphenation(e.to_string()))?;
        Ok(Self {
            fonts: vec![latin, arabic],
            latin: FontId(0),
            arabic: FontId(1),
            hyphenator,
            kashida_max_em: 0.5,
        })
    }

    #[must_use]
    pub fn fonts(&self) -> &[FontFace<'a>] {
        &self.fonts
    }

    #[must_use]
    pub fn latin_font(&self) -> FontId {
        self.latin
    }

    #[must_use]
    pub fn arabic_font(&self) -> FontId {
        self.arabic
    }

    #[must_use]
    pub fn font(&self, id: FontId) -> &FontFace<'a> {
        &self.fonts[usize::from(id.0)]
    }

    /// The hyphen (Latin fonts) and tatweel (Arabic fonts) of a font, as font
    /// glyph ids and advances.
    #[must_use]
    pub fn specials(&self, id: FontId) -> Specials {
        let f = self.font(id);
        if id == self.arabic {
            Specials {
                hyphen: None,
                tatweel: f.glyph('\u{0640}'),
            }
        } else {
            Specials {
                hyphen: f.glyph('\u{2010}').or_else(|| f.glyph('-')),
                tatweel: None,
            }
        }
    }

    /// Picks a face per character:
    /// - letters: Arabic letters → Arabic face, other letters → Latin face;
    /// - digits: the face of the nearest preceding letter (Western digits in
    ///   Arabic text are set in the Arabic face, matching its size and color);
    /// - other neutrals (spaces, punctuation): the face of an adjacent
    ///   character *at the same embedding level*, so a space never forms a
    ///   stray run between a Latin phrase and the Arabic text around it;
    /// - otherwise by direction: odd level → Arabic face, even → Latin face.
    ///
    /// Falls back to the other face when the chosen one has no glyph.
    fn assign_fonts(&self, chars: &[(usize, char)], levels: &[u8]) -> Vec<FontId> {
        let letter_font = |c: char| -> Option<FontId> {
            if !c.is_alphabetic() {
                None
            } else if is_arabic(c) {
                Some(self.arabic)
            } else {
                Some(self.latin)
            }
        };
        let by_level = |level: u8| {
            if level % 2 == 1 {
                self.arabic
            } else {
                self.latin
            }
        };
        let mut out: Vec<FontId> = Vec::with_capacity(chars.len());
        let mut last_letter: Option<FontId> = None;
        for (i, &(_, c)) in chars.iter().enumerate() {
            let level = levels[i];
            let pick = if let Some(f) = letter_font(c) {
                last_letter = Some(f);
                f
            } else if c.is_ascii_digit() {
                last_letter
                    .or_else(|| chars[i..].iter().find_map(|&(_, n)| letter_font(n)))
                    .unwrap_or_else(|| by_level(level))
            } else if i > 0 && levels[i - 1] == level {
                out[i - 1]
            } else {
                chars[i..]
                    .iter()
                    .zip(&levels[i..])
                    .take_while(|&(_, &l)| l == level)
                    .find_map(|(&(_, n), _)| letter_font(n))
                    .unwrap_or_else(|| by_level(level))
            };
            let other = if pick == self.arabic {
                self.latin
            } else {
                self.arabic
            };
            let missing = |f: FontId| self.font(f).face.glyph_index(c).is_none();
            out.push(if missing(pick) && !missing(other) {
                other
            } else {
                pick
            });
        }
        out
    }

    /// Shapes one paragraph.
    pub fn shape_paragraph(&self, text: &str) -> Result<ShapedParagraph, ShapeError> {
        if text.trim().is_empty() {
            return Err(ShapeError::Empty);
        }
        let bidi = BidiInfo::new(text, None);
        let base_level = bidi.paragraphs.first().map_or(0, |p| p.level.number());
        let level_at = |byte: usize| {
            bidi.levels
                .get(byte)
                .map_or(base_level, unicode_bidi::Level::number)
        };
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let levels: Vec<u8> = chars.iter().map(|&(b, _)| level_at(b)).collect();
        let fonts = self.assign_fonts(&chars, &levels);

        // Runs: maximal spans of equal (font, level).
        let mut spans: Vec<(usize, usize, FontId, u8)> = Vec::new();
        for (i, &(byte, _)) in chars.iter().enumerate() {
            let key = (fonts[i], levels[i]);
            match spans.last_mut() {
                Some(s) if (s.2, s.3) == key => {
                    s.1 = byte + text[byte..].chars().next().map_or(0, char::len_utf8);
                }
                _ => spans.push((
                    byte,
                    byte + text[byte..].chars().next().map_or(0, char::len_utf8),
                    key.0,
                    key.1,
                )),
            }
        }

        let mut runs = Vec::with_capacity(spans.len());
        let mut clusters: Vec<ClusterSpan> = Vec::new();
        for (r, &(start, end, font_id, level)) in spans.iter().enumerate() {
            let font = self.font(font_id);
            let mut buf = UnicodeBuffer::new();
            buf.push_str(&text[start..end]);
            buf.set_pre_context(&text[..start]);
            buf.set_post_context(&text[end..]);
            buf.set_direction(if level % 2 == 1 {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            });
            buf.set_script(font.script);
            buf.set_language(font.language.clone());
            let shaped = rustybuzz::shape(&font.face, &[], buf);
            let mut glyphs: Vec<(u32, u32, [i32; 3])> = shaped
                .glyph_infos()
                .iter()
                .zip(shaped.glyph_positions())
                .map(|(i, p)| (i.glyph_id, i.cluster, [p.x_advance, p.x_offset, p.y_offset]))
                .collect();
            if level % 2 == 1 {
                glyphs.reverse(); // shaper output is visual; Folio stores logical
            }
            let n = glyphs.len();
            let mut run = ShapedRun {
                font: font_id,
                scale: font.scale(),
                gids: Vec::with_capacity(n),
                advances: Vec::with_capacity(n),
                offsets: Vec::with_capacity(n),
                flags: vec![0; n],
                levels: vec![level; n],
                kashida_priority: vec![0; n],
                kashida_max: vec![0; n],
            };
            for (k, &(gid, cluster, [adv, dx, dy])) in glyphs.iter().enumerate() {
                run.gids
                    .push(u16::try_from(gid).map_err(|_| ShapeError::Overflow("glyph id"))?);
                run.advances
                    .push(i16::try_from(adv).map_err(|_| ShapeError::Overflow("advance"))?);
                run.offsets.push([
                    i16::try_from(dx).map_err(|_| ShapeError::Overflow("x offset"))?,
                    i16::try_from(dy).map_err(|_| ShapeError::Overflow("y offset"))?,
                ]);
                let byte = start + cluster as usize;
                match clusters.last_mut() {
                    Some(c) if c.run == r && c.byte == byte => c.last = k,
                    _ => {
                        run.flags[k] |= flags::CLUSTER_START;
                        clusters.push(ClusterSpan {
                            byte,
                            run: r,
                            first: k,
                            last: k,
                        });
                    }
                }
            }
            runs.push(run);
        }

        self.annotate(text, &mut runs, &clusters);
        Ok(ShapedParagraph { base_level, runs })
    }

    /// Word/sentence starts, glue and breaks, hyphenation and kashida.
    fn annotate(&self, text: &str, runs: &mut [ShapedRun], clusters: &[ClusterSpan]) {
        mark_words(text, runs, clusters);
        self.mark_hyphenation(text, runs, clusters);
        self.mark_kashida(text, runs, clusters);
    }

    /// Knuth–Liang points in alphabetic Latin words set in the Latin face.
    fn mark_hyphenation(&self, text: &str, runs: &mut [ShapedRun], clusters: &[ClusterSpan]) {
        let mut word_start: Option<usize> = None;
        let bytes = text
            .char_indices()
            .chain(core::iter::once((text.len(), ' ')));
        for (byte, ch) in bytes {
            let latin_letter = ch.is_alphabetic() && !is_arabic(ch);
            match (word_start, latin_letter) {
                (None, true) => word_start = Some(byte),
                (Some(ws), false) => {
                    self.hyphenate(&text[ws..byte], ws, runs, clusters);
                    word_start = None;
                }
                _ => {}
            }
        }
    }

    fn hyphenate(
        &self,
        word: &str,
        offset: usize,
        runs: &mut [ShapedRun],
        clusters: &[ClusterSpan],
    ) {
        if word.chars().count() < 5 || word.chars().all(char::is_uppercase) {
            return;
        }
        for b in self.hyphenator.hyphenate(word).breaks {
            let at = offset + b;
            let (Some(before), Some(after)) =
                (containing(clusters, at - 1), containing(clusters, at))
            else {
                continue;
            };
            // A break inside one cluster (a ligature) is impossible; a break
            // across a run boundary has no single run to carry the hyphen.
            if before.byte == after.byte
                || before.run != after.run
                || runs[before.run].font != self.latin
            {
                continue;
            }
            runs[before.run].flags[before.last] |= flags::HYPHEN_POINT;
        }
    }

    /// Kashida opportunities between joined Arabic letters.
    fn mark_kashida(&self, text: &str, runs: &mut [ShapedRun], clusters: &[ClusterSpan]) {
        let only_marks_between = |from: usize, to: usize| {
            text[from..to]
                .chars()
                .skip(1)
                .all(|c| joining(c) == Joining::T)
        };
        let letters: Vec<(usize, char)> = text
            .char_indices()
            .filter(|&(_, c)| is_arabic(c) && joining(c) != Joining::T)
            .collect();
        for w in 0..letters.len() {
            let (a_byte, a) = letters[w];
            let Some(&(b_byte, b)) = letters.get(w + 1) else {
                break;
            };
            if !only_marks_between(a_byte, b_byte) {
                continue; // not adjacent letters
            }
            let after_b = letters
                .get(w + 2)
                .filter(|&&(c_byte, _)| only_marks_between(b_byte, c_byte))
                .map(|&(_, c)| c);
            let b_final = !(joins_next(b) && after_b.is_some_and(joins_prev));
            let Some(prio) = kashida_priority(a, b, b_final, after_b) else {
                continue;
            };
            let (Some(ca), Some(cb)) = (containing(clusters, a_byte), containing(clusters, b_byte))
            else {
                continue;
            };
            if ca.byte == cb.byte || ca.run != cb.run {
                continue; // ligature, or the joint straddles a font/level change
            }
            let run = &mut runs[ca.run];
            if self.specials(run.font).tatweel.is_none() {
                continue;
            }
            let upem = self.font(run.font).face.units_per_em() as f32;
            let i = ca.last;
            run.flags[i] |= flags::KASHIDA_OK;
            let existing = run.kashida_priority[i];
            run.kashida_priority[i] = if existing == 0 {
                prio
            } else {
                existing.min(prio)
            };
            run.kashida_max[i] = (upem * self.kashida_max_em) as u16;
        }
    }
}

/// The cluster that contains `byte`: the last cluster starting at or before it.
fn containing(clusters: &[ClusterSpan], byte: usize) -> Option<&ClusterSpan> {
    let i = clusters.partition_point(|c| c.byte <= byte);
    i.checked_sub(1).and_then(|i| clusters.get(i))
}

/// Word and sentence starts, glue (spaces) and break opportunities.
fn mark_words(text: &str, runs: &mut [ShapedRun], clusters: &[ClusterSpan]) {
    let mut prev_char: Option<char> = None;
    let mut sentence_open = true;
    for (ci, c) in clusters.iter().enumerate() {
        let end = clusters
            .get(ci + 1)
            .map_or(text.len(), |n| n.byte.max(c.byte));
        let ch = text[c.byte..].chars().next().unwrap_or(' ');
        let run = &mut runs[c.run];
        match ch {
            ' ' => run.flags[c.first] |= flags::GLUE | flags::BREAK_OK,
            '\u{00A0}' | '\u{202F}' => run.flags[c.first] |= flags::GLUE,
            _ => {
                if prev_char.is_none_or(char::is_whitespace) {
                    run.flags[c.first] |= flags::WORD_START;
                    if sentence_open {
                        run.flags[c.first] |= flags::SENTENCE_START;
                        sentence_open = false;
                    }
                }
                if matches!(ch, '-' | '\u{2013}' | '\u{2014}' | '/') {
                    run.flags[c.last] |= flags::BREAK_OK;
                }
            }
        }
        let last = text[c.byte..end].chars().last().unwrap_or(ch);
        if matches!(last, '.' | '!' | '?' | '\u{061F}') {
            sentence_open = true;
        }
        prev_char = Some(last);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::str::FromStr;

    const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
    const ARABIC: &[u8] =
        include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

    fn typesetter() -> Typesetter<'static> {
        Typesetter::new(
            FontFace {
                face: Face::from_slice(LATIN, 0).unwrap(),
                size_px: 20.0,
                script: rustybuzz::script::LATIN,
                language: Language::from_str("en").unwrap(),
            },
            FontFace {
                face: Face::from_slice(ARABIC, 0).unwrap(),
                size_px: 24.0,
                script: rustybuzz::script::ARABIC,
                language: Language::from_str("ar").unwrap(),
            },
        )
        .unwrap()
    }

    fn flagged(p: &ShapedParagraph, bit: u8) -> usize {
        p.runs
            .iter()
            .flat_map(|r| &r.flags)
            .filter(|f| *f & bit != 0)
            .count()
    }

    #[test]
    fn latin_words_spaces_and_hyphenation() {
        let t = typesetter();
        let p = t
            .shape_paragraph("Extraordinary neighbourhood is universally acknowledged.")
            .unwrap();
        assert_eq!(p.base_level, 0);
        assert_eq!(p.runs.len(), 1);
        assert_eq!(flagged(&p, flags::WORD_START), 5);
        assert_eq!(flagged(&p, flags::GLUE), 4);
        assert_eq!(flagged(&p, flags::SENTENCE_START), 1);
        assert!(
            flagged(&p, flags::HYPHEN_POINT) >= 6,
            "long words get Knuth–Liang points"
        );
        assert!(t.specials(FontId(0)).hyphen.is_some());
    }

    #[test]
    fn arabic_is_rtl_logical_order_with_kashida_opportunities() {
        let t = typesetter();
        // "الإخوان هم الأعوان على الخير كله"
        let p = t.shape_paragraph("الإخوان هم الأعوان على الخير كله").unwrap();
        assert_eq!(p.base_level, 1);
        assert!(
            p.runs
                .iter()
                .all(|r| r.font == FontId(1) && r.levels.iter().all(|l| l % 2 == 1))
        );
        assert_eq!(flagged(&p, flags::WORD_START), 6);
        assert!(
            flagged(&p, flags::KASHIDA_OK) >= 4,
            "most words offer a kashida"
        );
        assert_eq!(
            flagged(&p, flags::HYPHEN_POINT),
            0,
            "Arabic is never hyphenated"
        );
        let r = &p.runs[0];
        for (i, f) in r.flags.iter().enumerate() {
            if f & flags::KASHIDA_OK != 0 {
                assert!((1..=6).contains(&r.kashida_priority[i]));
                assert_eq!(r.kashida_max[i], 500, "half an em of a 1000-unit font");
            }
        }
        assert!(t.specials(FontId(1)).tatweel.is_some());
    }

    #[test]
    fn marks_stay_in_their_base_cluster_with_offsets() {
        let t = typesetter();
        let p = t.shape_paragraph("قالَ الفيلسوفُ").unwrap();
        let r = &p.runs[0];
        let clusters = r
            .flags
            .iter()
            .filter(|f| *f & flags::CLUSTER_START != 0)
            .count();
        assert!(
            r.gids.len() > clusters,
            "harakat are extra glyphs inside clusters"
        );
        assert!(
            r.offsets.iter().any(|o| o[1] != 0),
            "GPOS places marks above/below"
        );
    }

    #[test]
    fn mixed_bidi_splits_runs_by_font_and_level() {
        let t = typesetter();
        let p = t
            .shape_paragraph("صدرت رواية Pride and Prejudice سنة 1813.")
            .unwrap();
        assert_eq!(p.base_level, 1);
        let latin: Vec<&ShapedRun> = p.runs.iter().filter(|r| r.font == FontId(0)).collect();
        assert_eq!(
            latin.len(),
            1,
            "the English title is one LTR run, spaces included"
        );
        assert!(latin[0].levels.iter().all(|l| *l == 2));
        assert_eq!(
            latin[0]
                .flags
                .iter()
                .filter(|f| *f & flags::GLUE != 0)
                .count(),
            2
        );
        let digits = p
            .runs
            .iter()
            .find(|r| r.levels[0] == 2 && r.font == FontId(1));
        assert!(
            digits.is_some(),
            "Western digits in Arabic text use the Arabic face"
        );
    }
}
