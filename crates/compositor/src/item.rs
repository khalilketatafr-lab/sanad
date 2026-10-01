//! Paragraph model: shaped glyph runs → Knuth–Plass items.
//!
//! The Compositor never sees Unicode. Everything it needs comes from
//! per-glyph data the server computed while shaping (Folio `Run`): permuted
//! glyph id, advance, flags (cluster/word/sentence starts, break, glue,
//! hyphenation and kashida opportunities) and bidi level.
//!
//! Justification model, by item kind:
//! - `Box`: one glyph. Fixed width. **No stretch, no shrink, ever.** This
//!   is what "no letter-spacing" means structurally: there is no field
//!   through which a box could grow.
//! - `Glue`: inter-word space. Stretches and shrinks (TeX proportions).
//! - `Kashida`: zero-width, stretch-only elongation after a joining glyph
//!   (Arabic). Never a break point. Consumed *before* glue (see `linebreak`).
//!   At most one per word: the highest-priority opportunity.
//! - `Penalty`: a break opportunity with a cost (hyphenation: flagged,
//!   carries the hyphen's width), or a forced or forbidden break.

/// GlyphFlags bit values (must mirror `crates/folio/schemas/folio.fbs`).
pub mod flags {
    pub const CLUSTER_START: u8 = 1 << 0;
    pub const BREAK_OK: u8 = 1 << 1;
    pub const GLUE: u8 = 1 << 2;
    pub const HYPHEN_POINT: u8 = 1 << 3;
    pub const KASHIDA_OK: u8 = 1 << 4;
    pub const WORD_START: u8 = 1 << 5;
    pub const SENTENCE_START: u8 = 1 << 6;
    pub const NO_JUSTIFY: u8 = 1 << 7;
}

/// Penalty values at or beyond these bounds mean "never" / "always" (TeX).
pub const INFINITE_PENALTY: f32 = 10_000.0;

/// Borrowed structure-of-arrays view of one shaped run (logical order).
#[derive(Debug, Clone, Copy)]
pub struct RunView<'a> {
    /// Device px per font unit: font size (px) / units-per-em.
    pub scale: f32,
    pub gids: &'a [u16],
    pub advances: &'a [i16],
    /// Empty, or one `[x, y]` displacement per glyph (font units, y up): GPOS
    /// mark attachment and cursive offsets. Never affects the pen.
    pub offsets: &'a [[i16; 2]],
    pub flags: &'a [u8],
    pub bidi_levels: &'a [u8],
    /// Empty, or one entry per glyph (0 = none, 1 = highest … 7 = lowest).
    pub kashida_priority: &'a [u8],
    /// Empty, or one entry per glyph: max elongation, font units.
    pub kashida_max: &'a [u16],
    /// Permuted id and advance (font units) of the font's hyphen glyph.
    pub hyphen: Option<(u16, i16)>,
    /// Permuted id and advance (font units) of the font's tatweel (U+0640) glyph.
    pub tatweel: Option<(u16, i16)>,
}

/// Locates a glyph: run index and glyph index within the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphRef {
    pub run: u16,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemKind {
    Box,
    Glue,
    Kashida,
    Penalty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub width: f32,
    /// Glue: stretchability. Kashida: maximum elongation. Otherwise 0.
    pub stretch: f32,
    /// Glue only.
    pub shrink: f32,
    /// Infinite stretch (paragraph fill). Glue only.
    pub fil: bool,
    /// Penalty only.
    pub penalty: f32,
    /// Penalty only: hyphenation (consecutive flagged breaks cost extra).
    pub flagged: bool,
    /// Box/Glue: the glyph. Penalty: the run whose hyphen is inserted. Kashida: the joining glyph.
    pub glyph: GlyphRef,
}

impl Item {
    const fn base(kind: ItemKind, glyph: GlyphRef) -> Self {
        Self {
            kind,
            width: 0.0,
            stretch: 0.0,
            shrink: 0.0,
            fil: false,
            penalty: 0.0,
            flagged: false,
            glyph,
        }
    }

    #[must_use]
    pub const fn boxed(width: f32, glyph: GlyphRef) -> Self {
        Self {
            width,
            ..Self::base(ItemKind::Box, glyph)
        }
    }

    #[must_use]
    pub const fn glue(width: f32, stretch: f32, shrink: f32, glyph: GlyphRef) -> Self {
        Self {
            width,
            stretch,
            shrink,
            ..Self::base(ItemKind::Glue, glyph)
        }
    }

    #[must_use]
    pub const fn kashida(max: f32, glyph: GlyphRef) -> Self {
        Self {
            stretch: max,
            ..Self::base(ItemKind::Kashida, glyph)
        }
    }

    #[must_use]
    pub const fn penalty(width: f32, penalty: f32, flagged: bool, glyph: GlyphRef) -> Self {
        Self {
            width,
            penalty,
            flagged,
            ..Self::base(ItemKind::Penalty, glyph)
        }
    }

    #[must_use]
    pub const fn is_forced_break(&self) -> bool {
        matches!(self.kind, ItemKind::Penalty) && self.penalty <= -INFINITE_PENALTY
    }
}

/// Glue proportions relative to the natural space (TeX: stretch ½, shrink ⅓).
#[derive(Debug, Clone, Copy)]
pub struct GlueModel {
    pub stretch: f32,
    pub shrink: f32,
}

impl Default for GlueModel {
    fn default() -> Self {
        Self {
            stretch: 0.5,
            shrink: 1.0 / 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemParams {
    pub glue: GlueModel,
    pub hyphen_penalty: f32,
    /// Hyphenation disabled by the reader (or script): HYPHEN_POINT ignored.
    pub hyphenate: bool,
}

impl Default for ItemParams {
    fn default() -> Self {
        Self {
            glue: GlueModel::default(),
            hyphen_penalty: 50.0,
            hyphenate: true,
        }
    }
}

/// Builds the item list for one paragraph (all runs, logical order), ending
/// with the standard paragraph fill: `penalty(∞) · glue(fil) · penalty(−∞)`.
#[must_use]
pub fn build_items(runs: &[RunView<'_>], params: &ItemParams) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    // Best kashida opportunity of the current word: (insert after item index, priority, item).
    let mut word_kashida: Option<(usize, u8, Item)> = None;
    let mut inserts: Vec<(usize, Item)> = Vec::new();
    let mut last = GlyphRef { run: 0, index: 0 };

    let close_word = |word: &mut Option<(usize, u8, Item)>, inserts: &mut Vec<(usize, Item)>| {
        if let Some((after, _, k)) = word.take() {
            inserts.push((after, k));
        }
    };

    for (r, run) in runs.iter().enumerate() {
        let n = run.gids.len();
        for i in 0..n {
            let glyph = GlyphRef {
                run: r as u16,
                index: i as u32,
            };
            last = glyph;
            let f = run.flags.get(i).copied().unwrap_or(0);
            let advance = f32::from(run.advances.get(i).copied().unwrap_or(0)) * run.scale;

            if f & flags::GLUE != 0 {
                close_word(&mut word_kashida, &mut inserts);
                let (stretch, shrink) = if f & flags::NO_JUSTIFY != 0 {
                    (0.0, 0.0)
                } else {
                    (advance * params.glue.stretch, advance * params.glue.shrink)
                };
                if f & flags::BREAK_OK == 0 {
                    // Non-breaking space: forbid a break at this glue.
                    items.push(Item::penalty(0.0, INFINITE_PENALTY, false, glyph));
                }
                items.push(Item::glue(advance, stretch, shrink, glyph));
                continue;
            }

            items.push(Item::boxed(advance, glyph));

            if f & flags::KASHIDA_OK != 0 && f & flags::NO_JUSTIFY == 0 {
                let prio = run.kashida_priority.get(i).copied().unwrap_or(0);
                let max_units = run.kashida_max.get(i).copied().unwrap_or(0);
                if prio > 0 && max_units > 0 && run.tatweel.is_some() {
                    let better = word_kashida.as_ref().is_none_or(|(_, p, _)| prio < *p);
                    if better {
                        let k = Item::kashida(f32::from(max_units) * run.scale, glyph);
                        word_kashida = Some((items.len(), prio, k));
                    }
                }
            }

            if f & flags::HYPHEN_POINT != 0 && params.hyphenate {
                if let Some((_, hyphen_adv)) = run.hyphen {
                    items.push(Item::penalty(
                        f32::from(hyphen_adv) * run.scale,
                        params.hyphen_penalty,
                        true,
                        glyph,
                    ));
                }
            } else if f & flags::BREAK_OK != 0 {
                // Break opportunity after a visible glyph (e.g. after a dash or slash).
                items.push(Item::penalty(0.0, 0.0, false, glyph));
            }
        }
    }
    close_word(&mut word_kashida, &mut inserts);

    // Splice kashida items in (indices ascending; shift as we go).
    for (shift, (after, k)) in inserts.into_iter().enumerate() {
        items.insert(after + shift, k);
    }

    // Paragraph fill (TeX's \parfillskip): last line set ragged.
    items.push(Item::penalty(0.0, INFINITE_PENALTY, false, last));
    items.push(Item {
        fil: true,
        ..Item::glue(0.0, 0.0, 0.0, last)
    });
    items.push(Item::penalty(0.0, -INFINITE_PENALTY, false, last));
    items
}
