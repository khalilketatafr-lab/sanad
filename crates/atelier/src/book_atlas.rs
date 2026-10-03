//! Frequency-ordered, multi-page MSDF atlas for a whole book (ingest stage
//! ⑤, blueprint 01 §5).
//!
//! The client downloads atlas pages on demand, so the most-read glyphs must
//! land on the lowest-numbered page: page 0 holds the smallest prefix of
//! glyphs, in descending occurrence order, whose cumulative share reaches
//! [`PAGE0_COVERAGE`] (~95% of the book's rendered glyphs). The long tail
//! fills later pages, packed by area.
//!
//! Each page is built by [`crate::atlas::build_atlas`], which shreds every
//! glyph into fragments and shuffles them within a size class, so the page's
//! spatial order reveals nothing. We do **not** deduplicate fragments by
//! content hash: [`crate::msdf::glyph_field`] colours each glyph's MSDF with
//! a per-glyph seed and [`build_atlas`](crate::atlas::build_atlas) cuts it
//! with another, so two copies of one outline bake to different bytes. Dedup
//! would therefore save nothing unless the seeds were shared, which is
//! exactly the correlation shredding exists to destroy. Size is bounded by
//! paging instead.

use std::collections::BTreeMap;

use crate::atlas::{Atlas, AtlasError, AtlasParams, build_atlas};
use crate::ingest::TypesetChapter;
use crate::msdf::{FieldParams, GlyphField, glyph_field};
use crate::permute::{GlyphKey, Permutation};
use crate::shape::Typesetter;

/// Share of rendered glyphs page 0 must cover (blueprint: "page 0 covers
/// ~95% of text").
pub const PAGE0_COVERAGE: f64 = 0.95;
/// Fraction of a page's texels assumed usable after shelf fragmentation and
/// power-of-two rounding; the tail is partitioned conservatively against it.
const PAGE_FILL: f64 = 0.70;
/// Fragment overlap inflates a glyph's slot area by about this much.
const FRAGMENT_OVERHEAD: f64 = 1.4;

/// Occurrence counts and the page-0 coverage cut.
#[derive(Debug, Clone)]
pub struct FrequencyPlan {
    /// `(permuted id, occurrences)` for inkful glyphs, most frequent first.
    pub ordered: Vec<(u16, u64)>,
    /// Total occurrences of inkful glyphs in the book.
    pub total: u64,
    /// How many leading glyphs reach [`PAGE0_COVERAGE`].
    pub page0_len: usize,
}

impl FrequencyPlan {
    /// Cumulative share after the first `n` glyphs.
    #[must_use]
    pub fn coverage(&self, n: usize) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        let covered: u64 = self.ordered.iter().take(n).map(|&(_, c)| c).sum();
        covered as f64 / self.total as f64
    }
}

/// Counts how often each permuted glyph is rendered across the book, orders
/// them by descending frequency, and marks the page-0 coverage cut.
/// `inkful` decides which glyphs get an atlas slot (non-ink glyphs such as
/// the space are excluded).
#[must_use]
pub fn plan_frequencies(
    book: &[TypesetChapter],
    perm: &Permutation,
    inkful: &dyn Fn(u16) -> bool,
    target: f64,
) -> FrequencyPlan {
    let mut count: BTreeMap<u16, u64> = BTreeMap::new();
    for chapter in book {
        for block in &chapter.blocks {
            for run in &block.shaped.runs {
                for &gid in &run.gids {
                    let Some(id) = perm.get(GlyphKey {
                        font: run.font,
                        gid,
                    }) else {
                        continue;
                    };
                    if inkful(id) {
                        *count.entry(id).or_default() += 1;
                    }
                }
            }
        }
    }
    // Descending by count, then by id for a stable, reproducible order.
    let mut ordered: Vec<(u16, u64)> = count.into_iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total: u64 = ordered.iter().map(|&(_, c)| c).sum();

    let cut = (total as f64 * target).ceil() as u64;
    let mut acc = 0u64;
    let mut page0_len = 0;
    for &(_, c) in &ordered {
        if acc >= cut {
            break;
        }
        acc += c;
        page0_len += 1;
    }
    FrequencyPlan {
        ordered,
        total,
        page0_len,
    }
}

/// The book's atlas: pages in frequency order, and where each glyph lives.
#[derive(Debug)]
pub struct BookAtlas {
    pub pages: Vec<Atlas>,
    /// Permuted glyph id → page index.
    pub page_of: BTreeMap<u16, usize>,
    /// Cumulative occurrence share covered through each page.
    pub coverage: Vec<f64>,
    pub plan: FrequencyPlan,
}

impl BookAtlas {
    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.page_of.len()
    }
}

fn permuted_to_font(perm: &Permutation) -> BTreeMap<u16, GlyphKey> {
    perm.iter().collect()
}

/// Builds the MSDF field for one permuted glyph, seeded by its edition id.
fn field_for(ts: &Typesetter<'_>, id: u16, key: GlyphKey, fp: &FieldParams) -> Option<GlyphField> {
    glyph_field(&ts.font(key.font).face, key.gid, fp, u64::from(id))
}

fn est_area(f: &GlyphField) -> f64 {
    (f.image.width * f.image.height) as f64 * FRAGMENT_OVERHEAD
}

/// Groups `ordered` fields by estimated area against the page budget,
/// keeping frequency order (so earlier groups hold the most-read glyphs). A
/// group is a first guess at a page; [`build_group`] splits it further if the
/// real shelf packing overflows. No forced 95% break: a book that fits stays
/// one page, and page 0 then covers everything (≥ the target).
fn group_by_area(ordered: Vec<(u16, GlyphField)>, budget: f64) -> Vec<Vec<(u16, GlyphField)>> {
    let mut groups: Vec<Vec<(u16, GlyphField)>> = Vec::new();
    let mut current: Vec<(u16, GlyphField)> = Vec::new();
    let mut area = 0.0;
    for (id, field) in ordered {
        let a = est_area(&field);
        if !current.is_empty() && area + a > budget {
            groups.push(std::mem::take(&mut current));
            area = 0.0;
        }
        area += a;
        current.push((id, field));
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Builds one group into one or more pages. The area estimate can still
/// overflow the shelf packer, so on [`AtlasError::Full`] the group is split
/// in half (frequency order preserved: the more-read half stays first) and
/// each half built on its own. A single glyph always fits, so this
/// terminates; `build_atlas` is given a clone so the fields survive a retry.
fn build_group(
    mut fields: Vec<(u16, GlyphField)>,
    params: &AtlasParams,
    seed: [u8; 32],
) -> Result<Vec<(Atlas, Vec<u16>)>, AtlasError> {
    let ids: Vec<u16> = fields.iter().map(|&(id, _)| id).collect();
    match build_atlas(fields.clone(), params, seed) {
        Ok(atlas) => Ok(vec![(atlas, ids)]),
        Err(AtlasError::Full { .. }) if fields.len() > 1 => {
            let right = fields.split_off(fields.len().div_ceil(2));
            let (mut ls, mut rs) = (seed, seed);
            ls[1] ^= 0x11;
            rs[1] ^= 0x22;
            let mut pages = build_group(fields, params, ls)?;
            pages.extend(build_group(right, params, rs)?);
            Ok(pages)
        }
        Err(e) => Err(e),
    }
}

/// Builds the whole book's atlas. `field_params` and `atlas_params` are the
/// MSDF and packing settings; `seed` is the edition's secret (each page is
/// shredded from a distinct derived seed).
pub fn build_book_atlas(
    ts: &Typesetter<'_>,
    book: &[TypesetChapter],
    perm: &Permutation,
    field_params: &FieldParams,
    atlas_params: &AtlasParams,
    target_coverage: f64,
    seed: [u8; 32],
) -> Result<BookAtlas, AtlasError> {
    let inverse = permuted_to_font(perm);
    let inkful = |id: u16| {
        inverse
            .get(&id)
            .is_some_and(|&key| field_for(ts, id, key, field_params).is_some())
    };
    let plan = plan_frequencies(book, perm, &inkful, target_coverage);

    // Build fields in frequency order (inkful glyphs only).
    let mut ordered_fields: Vec<(u16, GlyphField)> = Vec::with_capacity(plan.ordered.len());
    for &(id, _) in &plan.ordered {
        if let Some(key) = inverse.get(&id).copied()
            && let Some(field) = field_for(ts, id, key, field_params)
        {
            ordered_fields.push((id, field));
        }
    }

    let budget = f64::from(atlas_params.width) * f64::from(atlas_params.max_height) * PAGE_FILL;
    let groups = group_by_area(ordered_fields, budget);

    // Build each group (splitting on overflow) into final pages, in order.
    let mut built: Vec<(Atlas, Vec<u16>)> = Vec::new();
    for (g, group) in groups.into_iter().enumerate() {
        let mut group_seed = seed;
        group_seed[0] ^= g as u8;
        built.extend(build_group(group, atlas_params, group_seed)?);
    }

    let freq: BTreeMap<u16, u64> = plan.ordered.iter().copied().collect();
    let mut pages = Vec::with_capacity(built.len());
    let mut page_of = BTreeMap::new();
    let mut coverage = Vec::with_capacity(built.len());
    let mut covered = 0u64;
    for (page_index, (atlas, ids)) in built.into_iter().enumerate() {
        for id in ids {
            page_of.insert(id, page_index);
            covered += freq.get(&id).copied().unwrap_or(0);
        }
        pages.push(atlas);
        coverage.push(if plan.total == 0 {
            1.0
        } else {
            covered as f64 / plan.total as f64
        });
    }

    Ok(BookAtlas {
        pages,
        page_of,
        coverage,
        plan,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::str::FromStr;

    use rustybuzz::{Face, Language};

    use super::*;
    use crate::epub::{self, Limits};
    use crate::ingest::{edition_permutation, typeset_book};
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

    fn book() -> (Typesetter<'static>, Vec<TypesetChapter>, Permutation) {
        let parsed = epub::intake(SAMPLE, &Limits::default()).unwrap();
        let ts = typesetter();
        let typeset = typeset_book(&ts, &parsed.chapters).unwrap();
        let perm = edition_permutation(&ts, &typeset, [0x5A; 32]).unwrap();
        (ts, typeset, perm)
    }

    #[test]
    fn frequency_order_is_descending_and_cut_reaches_target() {
        let (ts, typeset, perm) = book();
        let inverse = permuted_to_font(&perm);
        let fp = FieldParams::default();
        let inkful = |id: u16| {
            inverse
                .get(&id)
                .is_some_and(|&k| field_for(&ts, id, k, &fp).is_some())
        };
        let plan = plan_frequencies(&typeset, &perm, &inkful, PAGE0_COVERAGE);
        assert!(plan.total > 0 && !plan.ordered.is_empty());
        // Descending order.
        assert!(plan.ordered.windows(2).all(|w| w[0].1 >= w[1].1));
        // The cut reaches the target but the glyph before it did not.
        assert!(plan.coverage(plan.page0_len) >= PAGE0_COVERAGE);
        assert!(plan.page0_len <= plan.ordered.len());
        if plan.page0_len > 0 {
            assert!(plan.coverage(plan.page0_len - 1) < PAGE0_COVERAGE);
        }
    }

    #[test]
    fn single_page_when_the_book_fits() {
        let (ts, typeset, perm) = book();
        let atlas = build_book_atlas(
            &ts,
            &typeset,
            &perm,
            &FieldParams::default(),
            &AtlasParams::default(),
            PAGE0_COVERAGE,
            [0x5A; 32],
        )
        .unwrap();
        assert_eq!(atlas.pages.len(), 1, "the sample book fits one 2048² page");
        assert!(atlas.glyph_count() > 100);
        assert!((atlas.coverage.last().unwrap() - 1.0).abs() < 1e-9);
        // Every rendered glyph has a page and a slot on it.
        for (&id, &page) in &atlas.page_of {
            assert!(atlas.pages[page].glyphs.contains_key(&id));
        }
    }

    #[test]
    fn small_pages_force_frequency_ordered_paging() {
        let (ts, typeset, perm) = book();
        // A deliberately small page forces the tail onto later pages.
        let params = AtlasParams {
            max_height: 256,
            ..AtlasParams::default()
        };
        let atlas = build_book_atlas(
            &ts,
            &typeset,
            &perm,
            &FieldParams::default(),
            &params,
            PAGE0_COVERAGE,
            [0x5A; 32],
        )
        .unwrap();
        assert!(atlas.pages.len() >= 2, "small pages must split the book");
        // Coverage is non-decreasing and ends at 1.0.
        assert!(atlas.coverage.windows(2).all(|w| w[0] <= w[1]));
        assert!((atlas.coverage.last().unwrap() - 1.0).abs() < 1e-9);
        // Page 0 carries the most frequent glyphs: its mean frequency beats
        // the last page's.
        let freq: BTreeMap<u16, u64> = atlas.plan.ordered.iter().copied().collect();
        let mean = |page: usize| -> f64 {
            let ids: Vec<u64> = atlas
                .page_of
                .iter()
                .filter(|&(_, &p)| p == page)
                .map(|(id, _)| freq[id])
                .collect();
            ids.iter().sum::<u64>() as f64 / ids.len() as f64
        };
        assert!(mean(0) > mean(atlas.pages.len() - 1));
    }
}
