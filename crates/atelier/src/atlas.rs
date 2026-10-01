//! Ingest stage ⑤: shred every glyph field and pack the fragments into one
//! atlas page.
//!
//! - **Shredding:** each field is split into k Voronoi fragments (k = 3, or 2
//!   for small glyphs). Every fragment keeps the full field size, so the GPU's
//!   MAX blend reassembles the glyph exactly (see [`crate::shred`]).
//! - **Atomic glyphs:** exactness needs each cell dilated by ρ/2 + 2 texels.
//!   Glyphs smaller than that (periods, dots, harakat) come out with one
//!   fragment that already *is* the whole glyph, so shredding them is
//!   meaningless. They are stored as a single fragment. They carry no
//!   letterform, and the edition's glyph permutation already hides their
//!   identity.
//! - **Placement:** fragments are shuffled before packing, so a glyph's
//!   fragments land in unrelated slots and no slot holds a recognizable
//!   character.
//! - **Determinism:** slot sizes are powers of two, each slot sits at a
//!   multiple of its own size, and the page is a power of two. Every fragment
//!   of a glyph then samples the atlas at bit-identical fractional texel
//!   positions, which is what makes the reassembled union exact on real
//!   hardware (measured in `packages/lumen/test/webgl.test.ts`).

use std::cmp::Reverse;
use std::collections::BTreeMap;

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use thiserror::Error;

use crate::msdf::GlyphField;
use crate::shred::{DistanceImage, Point, VoronoiCut, bake_fragment, min_overlap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Where one glyph's fragments live and how to place them on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasGlyph {
    /// Texel coordinates of the glyph origin within each (equal-size) slot.
    pub origin: (f32, f32),
    pub texels_per_unit: f32,
    pub slots: Vec<Slot>,
}

#[derive(Debug, Clone)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub px_range: f32,
    /// RGBA8, row-major, top row first.
    pub rgba: Vec<u8>,
    /// Keyed by edition glyph id.
    pub glyphs: BTreeMap<u16, AtlasGlyph>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasParams {
    /// Page width (power of two). WebGL2 guarantees 2048.
    pub width: u32,
    pub max_height: u32,
    /// Ink area (texels²) below which a glyph gets 2 fragments instead of 3.
    pub small_ink_area: f32,
    /// `false` stores whole glyphs: only for building the reference that
    /// tests compare shredded rendering against. Never shipped.
    pub shred: bool,
}

impl Default for AtlasParams {
    fn default() -> Self {
        Self {
            width: 2048,
            max_height: 2048,
            small_ink_area: 144.0,
            shred: true,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AtlasError {
    #[error("atlas page width {0} is not a power of two")]
    Width(u32),
    #[error("fragments need {needed} rows of texels; the page allows {max}")]
    Full { needed: u32, max: u32 },
    #[error("glyph {0}: could not place shredding sites")]
    Shred(u16),
}

/// Voronoi sites for one glyph: k well-separated sites inside its ink box,
/// relaxing separation and k for tiny glyphs.
fn cut_for(field: &GlyphField, seed: u64, p: &AtlasParams) -> Option<VoronoiCut> {
    let rho = field.image.px_range;
    let ink = field.ink;
    let k = if ink.w * ink.h >= p.small_ink_area {
        3
    } else {
        2
    };
    let sep = 0.35 * ink.w.min(ink.h).max(1.0);
    for (k, sep) in [(k, sep), (2, sep * 0.5), (2, 0.5)] {
        if let Ok(cut) = VoronoiCut::seeded(seed, ink, k, sep, min_overlap(rho), rho) {
            return Some(cut);
        }
    }
    // Degenerate ink (a hairline): split it across its long axis.
    let (cx, cy) = (ink.x + ink.w / 2.0, ink.y + ink.h / 2.0);
    let (a, b) = if ink.w >= ink.h {
        (Point { x: cx - 0.5, y: cy }, Point { x: cx + 0.5, y: cy })
    } else {
        (Point { x: cx, y: cy - 0.5 }, Point { x: cx, y: cy + 0.5 })
    };
    VoronoiCut::new(vec![a, b], min_overlap(rho), rho).ok()
}

struct Piece {
    gid: u16,
    image: DistanceImage,
}

pub fn build_atlas(
    fields: Vec<(u16, GlyphField)>,
    params: &AtlasParams,
    seed: [u8; 32],
) -> Result<Atlas, AtlasError> {
    let p = params;
    if !p.width.is_power_of_two() {
        return Err(AtlasError::Width(p.width));
    }
    let mut rng = ChaCha20Rng::from_seed(seed);
    let px_range = fields.first().map_or(6.0, |(_, f)| f.image.px_range);
    let mut glyphs = BTreeMap::new();
    let mut pieces: Vec<Piece> = Vec::new();
    for (gid, field) in fields {
        let cut = cut_for(&field, rng.next_u64(), p).ok_or(AtlasError::Shred(gid))?;
        let fragments: Vec<DistanceImage> = (0..cut.sites().len())
            .map(|k| bake_fragment(&field.image, &cut, k))
            .collect();
        let whole = field.image.to_rgba8();
        if !p.shred || fragments.iter().any(|f| f.to_rgba8() == whole) {
            pieces.push(Piece {
                gid,
                image: field.image.clone(),
            });
        } else {
            pieces.extend(fragments.into_iter().map(|image| Piece { gid, image }));
        }
        glyphs.insert(
            gid,
            AtlasGlyph {
                origin: field.origin,
                texels_per_unit: field.texels_per_unit,
                slots: Vec::new(),
            },
        );
    }

    // Shuffle, then a stable sort by size class: random order within a class.
    for i in (1..pieces.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        pieces.swap(i, j);
    }
    pieces.sort_by_key(|pc| (Reverse(pc.image.height), Reverse(pc.image.width)));

    // Shelf packing. Rows hold one height; heights and widths are
    // non-increasing powers of two, so every slot is aligned to its size.
    let mut placed: Vec<(Slot, usize)> = Vec::with_capacity(pieces.len());
    let (mut cursor_x, mut cursor_y, mut row_h) = (0u32, 0u32, 0u32);
    for (i, pc) in pieces.iter().enumerate() {
        let (w, h) = (pc.image.width as u32, pc.image.height as u32);
        if row_h == 0 {
            row_h = h;
        } else if h != row_h || cursor_x + w > p.width {
            cursor_y += row_h;
            cursor_x = 0;
            row_h = h;
        }
        placed.push((
            Slot {
                x: cursor_x,
                y: cursor_y,
                w,
                h,
            },
            i,
        ));
        cursor_x += w;
    }
    let used = cursor_y + row_h;
    let height = used.max(1).next_power_of_two();
    if height > p.max_height {
        return Err(AtlasError::Full {
            needed: used,
            max: p.max_height,
        });
    }

    let mut rgba = vec![0u8; (p.width * height * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    for (slot, i) in placed {
        let pc = &pieces[i];
        let src = pc.image.to_rgba8();
        let row = (slot.w * 4) as usize;
        for ty in 0..slot.h as usize {
            let dst = (((slot.y as usize + ty) * p.width as usize) + slot.x as usize) * 4;
            rgba[dst..dst + row].copy_from_slice(&src[ty * row..(ty + 1) * row]);
        }
        if let Some(g) = glyphs.get_mut(&pc.gid) {
            g.slots.push(slot);
        }
    }
    Ok(Atlas {
        width: p.width,
        height,
        px_range,
        rgba,
        glyphs,
    })
}

impl Atlas {
    /// RGBA8 texels of one slot.
    #[must_use]
    pub fn slot_texels(&self, s: Slot) -> Vec<[u8; 4]> {
        let mut out = Vec::with_capacity((s.w * s.h) as usize);
        for y in s.y..s.y + s.h {
            for x in s.x..s.x + s.w {
                let i = ((y * self.width + x) * 4) as usize;
                out.push([
                    self.rgba[i],
                    self.rgba[i + 1],
                    self.rgba[i + 2],
                    self.rgba[i + 3],
                ]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::msdf::{FieldParams, glyph_field};
    use ttf_parser::Face;

    const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
    const ARABIC: &[u8] =
        include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

    fn fields() -> Vec<(u16, GlyphField)> {
        let mut out = Vec::new();
        let mut id = 100u16;
        for (data, chars) in [(LATIN, "gOW.i"), (ARABIC, "عبكـ")] {
            let face = Face::parse(data, 0).unwrap();
            for c in chars.chars() {
                let gid = face.glyph_index(c).unwrap().0;
                out.push((
                    id,
                    glyph_field(&face, gid, &FieldParams::default(), 7).unwrap(),
                ));
                id += 1;
            }
        }
        out
    }

    #[test]
    fn fragments_reassemble_each_glyph_exactly_and_are_aligned() {
        let fields = fields();
        let originals: BTreeMap<u16, Vec<u8>> = fields
            .iter()
            .map(|(g, f)| (*g, f.image.to_rgba8()))
            .collect();
        let atlas = build_atlas(fields, &AtlasParams::default(), [3; 32]).unwrap();
        assert!(atlas.height.is_power_of_two());
        for (gid, g) in &atlas.glyphs {
            if g.slots.len() == 1 {
                // Atomic: within the overlap band of a single cut (the period
                // and the tatweel, a bare connecting stroke). Letters never are.
                let s = g.slots[0];
                let one: Vec<u8> = atlas.slot_texels(s).into_iter().flatten().collect();
                assert_eq!(&one, &originals[gid]);
                assert!(
                    matches!(*gid, 103 | 108),
                    "letter glyph {gid} was not shredded"
                );
                continue;
            }
            let first = g.slots[0];
            let mut union = vec![[0u8; 4]; (first.w * first.h) as usize];
            for s in &g.slots {
                assert_eq!((s.w, s.h), (first.w, first.h), "equal-size fragments");
                assert_eq!((s.x % s.w, s.y % s.h), (0, 0), "slot aligned to its size");
                for (u, t) in union.iter_mut().zip(atlas.slot_texels(*s)) {
                    for c in 0..3 {
                        u[c] = u[c].max(t[c]);
                    }
                    u[3] = 255;
                }
            }
            let flat: Vec<u8> = union.into_iter().flatten().collect();
            assert_eq!(&flat, &originals[gid], "MAX of fragments == glyph {gid}");
            // And no single fragment is the glyph.
            for s in &g.slots {
                let one: Vec<u8> = atlas.slot_texels(*s).into_iter().flatten().collect();
                assert_ne!(&one, &originals[gid], "a fragment is the whole glyph {gid}");
            }
        }
    }

    #[test]
    fn a_glyphs_fragments_are_not_neighbors_in_packing_order() {
        let atlas = build_atlas(fields(), &AtlasParams::default(), [9; 32]).unwrap();
        let adjacent = atlas
            .glyphs
            .values()
            .filter(|g| {
                g.slots
                    .windows(2)
                    .all(|w| w[1].x == w[0].x + w[0].w && w[1].y == w[0].y)
            })
            .count();
        assert!(
            adjacent < atlas.glyphs.len(),
            "shuffling separates fragments"
        );
    }
}
