//! The bridge from intake (stage ①) to typesetting (stage ②): a normalized
//! [`Chapter`](crate::epub::Chapter) becomes shaped, logical-order glyph runs,
//! which the rest of the pipeline permutes, shreds and composes.
//!
//! ```text
//! epub::Book ─ typeset_book ─▶ [TypesetChapter { blocks: [TypesetBlock{ shaped }] }]
//!                                   │
//!                   edition_permutation  (π over every glyph used in the book)
//!                                   ▼
//!              permute ─▶ shred ─▶ atlas ─▶ compose_page   (existing pipeline)
//! ```
//!
//! Each text-bearing leaf block (heading, paragraph, list item, caption, note)
//! is shaped on its own with the edition [`Typesetter`], which resolves bidi,
//! assigns Latin/Arabic faces per run and annotates break, hyphenation and
//! kashida opportunities. This is still server-side: the output holds font
//! glyph ids, which [`edition_permutation`] maps to secret per-edition ids so
//! nothing downstream carries Unicode or stable glyph ids (P1).

use crate::epub::{Block, BlockKind, Chapter, Content};
use crate::page::used_glyphs;
use crate::permute::Permutation;
use crate::shape::{ShapeError, ShapedParagraph, Typesetter};

/// A shaped text-bearing block, keyed back to its source for anchors.
#[derive(Debug, Clone)]
pub struct TypesetBlock {
    pub role: BlockKind,
    /// Source element `id`, when it had one (anchor target).
    pub id: Option<String>,
    /// 1-based source line, for diagnostics.
    pub line: u32,
    pub shaped: ShapedParagraph,
}

/// One chapter's shaped blocks, in reading order.
#[derive(Debug, Clone)]
pub struct TypesetChapter {
    pub path: String,
    pub blocks: Vec<TypesetBlock>,
}

impl TypesetChapter {
    /// Every shaped paragraph, for permutation and glyph accounting.
    pub fn shaped(&self) -> impl Iterator<Item = &ShapedParagraph> {
        self.blocks.iter().map(|b| &b.shaped)
    }

    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.blocks.iter().map(|b| b.shaped.glyph_count()).sum()
    }
}

/// Whether a block kind carries running text that is set as a paragraph.
#[must_use]
pub const fn is_text_bearing(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Heading { .. }
            | BlockKind::Paragraph
            | BlockKind::ListItem { .. }
            | BlockKind::Caption
    )
}

/// Collects the text-bearing leaf blocks of a chapter in reading order,
/// descending into containers (sections, lists, blockquotes, figures, notes).
fn text_blocks<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
    for b in blocks {
        match &b.content {
            Content::Inlines(items) if is_text_bearing(&b.kind) && !items.is_empty() => {
                out.push(b);
            }
            Content::Blocks(children) => text_blocks(children, out),
            _ => {}
        }
    }
}

/// Shapes one chapter. A block whose text is only whitespace (e.g. a heading
/// that held just an image) is skipped rather than failing the chapter.
pub fn typeset_chapter(
    ts: &Typesetter<'_>,
    chapter: &Chapter,
) -> Result<TypesetChapter, ShapeError> {
    let mut leaves = Vec::new();
    text_blocks(&chapter.blocks, &mut leaves);
    let mut blocks = Vec::with_capacity(leaves.len());
    for b in leaves {
        let text = b.text();
        match ts.shape_paragraph(&text) {
            Ok(shaped) => blocks.push(TypesetBlock {
                role: b.kind.clone(),
                id: b.id.clone(),
                line: b.line,
                shaped,
            }),
            Err(ShapeError::Empty) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(TypesetChapter {
        path: chapter.path.clone(),
        blocks,
    })
}

/// Shapes every chapter of a book.
pub fn typeset_book(
    ts: &Typesetter<'_>,
    chapters: &[Chapter],
) -> Result<Vec<TypesetChapter>, ShapeError> {
    chapters.iter().map(|c| typeset_chapter(ts, c)).collect()
}

/// The edition permutation π over every font glyph used across the book,
/// including the inserted hyphen and tatweel. `seed` is the edition's secret.
pub fn edition_permutation(
    ts: &Typesetter<'_>,
    book: &[TypesetChapter],
    seed: [u8; 32],
) -> Result<Permutation, crate::permute::PermuteError> {
    let all: Vec<ShapedParagraph> = book.iter().flat_map(|c| c.shaped().cloned()).collect();
    Permutation::new(used_glyphs(ts, &all), seed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::str::FromStr;

    use rustybuzz::{Face, Language};

    use super::*;
    use crate::epub::{self, Limits};
    use crate::permute::GlyphKey;
    use crate::shape::FontFace;

    const SAMPLE: &[u8] = include_bytes!("../../../fixtures/epub/sanad-sample.epub");
    const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
    const ARABIC: &[u8] =
        include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

    fn typesetter() -> Typesetter<'static> {
        Typesetter::new(
            FontFace {
                face: Face::from_slice(LATIN, 0).unwrap(),
                size_px: 36.0,
                script: rustybuzz::script::LATIN,
                language: Language::from_str("en").unwrap(),
            },
            FontFace {
                face: Face::from_slice(ARABIC, 0).unwrap(),
                size_px: 42.0,
                script: rustybuzz::script::ARABIC,
                language: Language::from_str("ar").unwrap(),
            },
        )
        .unwrap()
    }

    #[test]
    fn shapes_the_sample_epub_end_to_end() {
        let book = epub::intake(SAMPLE, &Limits::default()).unwrap();
        let ts = typesetter();
        let typeset = typeset_book(&ts, &book.chapters).unwrap();
        assert_eq!(typeset.len(), 2);

        // Every text-bearing block of chapter 1 is shaped: h1, epigraph p,
        // 5 story paragraphs, 4 list items, the footnote p = 12.
        assert_eq!(typeset[0].blocks.len(), 12);
        assert!(typeset.iter().all(|c| c.glyph_count() > 0));

        // Headings and list items are shaped, not just paragraphs.
        let roles: Vec<_> = typeset[0].blocks.iter().map(|b| &b.role).collect();
        assert!(roles.contains(&&BlockKind::Heading { level: 1 }));
        assert!(
            roles
                .iter()
                .any(|r| matches!(r, BlockKind::ListItem { .. }))
        );

        // The epigraph is English (LTR); the story paragraphs are Arabic (RTL).
        let epigraph = &typeset[0].blocks[1].shaped;
        assert_eq!(epigraph.base_level % 2, 0);
        let story = &typeset[0].blocks[2].shaped;
        assert_eq!(story.base_level % 2, 1);
    }

    #[test]
    fn permutation_covers_every_glyph_and_hides_identity() {
        let book = epub::intake(SAMPLE, &Limits::default()).unwrap();
        let ts = typesetter();
        let typeset = typeset_book(&ts, &book.chapters).unwrap();
        let perm = edition_permutation(&ts, &typeset, [0x5A; 32]).unwrap();

        let mut mapped = 0;
        let mut identity = 0;
        for chapter in &typeset {
            for block in &chapter.blocks {
                for run in &block.shaped.runs {
                    for &gid in &run.gids {
                        let key = GlyphKey {
                            font: run.font,
                            gid,
                        };
                        let pid = perm.get(key).unwrap(); // every used glyph is permuted
                        assert_eq!(perm.key(pid), Some(key), "π is a bijection");
                        mapped += 1;
                        identity += usize::from(pid == gid);
                    }
                }
            }
        }
        assert!(mapped > 500, "shaped {mapped} glyphs");
        // A secret permutation leaves almost nothing at its own id.
        assert!(identity * 20 < mapped, "{identity}/{mapped} glyphs unmoved");
    }

    #[test]
    fn permutation_is_deterministic_per_seed_and_differs_across_editions() {
        let book = epub::intake(SAMPLE, &Limits::default()).unwrap();
        let ts = typesetter();
        let typeset = typeset_book(&ts, &book.chapters).unwrap();
        let key = {
            let r = &typeset[0].blocks[0].shaped.runs[0];
            GlyphKey {
                font: r.font,
                gid: r.gids[0],
            }
        };
        let a = edition_permutation(&ts, &typeset, [0x5A; 32]).unwrap();
        let a2 = edition_permutation(&ts, &typeset, [0x5A; 32]).unwrap();
        let b = edition_permutation(&ts, &typeset, [0xC3; 32]).unwrap();
        assert_eq!(a.get(key), a2.get(key), "same seed, same π");
        assert_ne!(a.get(key), b.get(key), "different edition, different π");
    }
}
