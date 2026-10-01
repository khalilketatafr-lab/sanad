//! Ingest stage ④: glyph outlines → multi-channel signed distance fields.
//!
//! Each glyph is rasterized into its own power-of-two field (see
//! [`crate::atlas`] for why sizes and slots are powers of two) at a fixed
//! number of texels per em, with `ρ` texels of distance range, in the
//! normalized form the shredder and Lumen's Pass 1 share:
//! `s = d/ρ + ½` (`d` in texels, positive inside).
//!
//! Generation is fdsm (a Rust port of Chlumský's msdfgen): simple edge
//! coloring, MSDF, msdfgen's artifact error correction, then sign correction
//! by the nonzero fill rule (font outlines' winding is not trusted).
//!
//! **Far-field clearing.** Only the *median* of an MSDF texel is a distance.
//! Far from the outline the individual channels hold pseudo-distances that
//! can be anything, e.g. `(1, 0, 0)`. Bilinear filtering between two such
//! texels, such as at a quad edge where the sampler also reads the
//! neighboring atlas slot, can produce a median above ½ out of nothing, which
//! draws a stray sliver of ink. Every texel whose median is saturated outside
//! (≤ −ρ/2) is therefore set to `(0, 0, 0)`. That changes no distance the
//! renderer uses, since its AA ramp ends ±2 screen px from the edge. Because
//! the padding is ≥ ρ/2 + 1 texels, every slot border is then zero in every
//! channel, so slots cannot bleed into each other.

use fdsm::bezier::scanline::FillRule;
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_msdf};
use fdsm::generate::generate_msdf;
use fdsm::render::correct_sign_msdf;
use fdsm::shape::Shape;
use fdsm::transform::Transform;
use image::{ImageBuffer, Rgb};
use nalgebra::{Affine2, Matrix3};
use ttf_parser::{Face, GlyphId};

use crate::shred::{DistanceImage, Rect, median3};

/// Field resolution and range, shared by every glyph of an atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldParams {
    /// Texels per em.
    pub em_texels: f32,
    /// Distance range ρ in texels.
    pub px_range: f32,
    /// Smallest and largest slot edge (powers of two).
    pub min_slot: u32,
    pub max_slot: u32,
}

impl Default for FieldParams {
    fn default() -> Self {
        Self {
            em_texels: 48.0,
            px_range: 6.0,
            min_slot: 16,
            max_slot: 256,
        }
    }
}

/// One glyph's distance field and how to place it.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphField {
    /// Power-of-two sized field.
    pub image: DistanceImage,
    /// Texel coordinates (x right, y DOWN) of the glyph origin: the pen
    /// position on the baseline.
    pub origin: (f32, f32),
    /// Texels per font unit (`em_texels / units_per_em`).
    pub texels_per_unit: f32,
    /// Ink bounds in texels: where shredding sites may go.
    pub ink: Rect,
}

fn slot_edge(texels: f32, p: &FieldParams) -> Option<u32> {
    let needed = texels.ceil().max(1.0) as u32;
    let edge = needed.next_power_of_two().max(p.min_slot);
    (edge <= p.max_slot).then_some(edge)
}

/// The distance field of `gid`, or `None` for glyphs without ink (spaces)
/// or larger than the biggest slot.
#[must_use]
pub fn glyph_field(
    face: &Face<'_>,
    gid: u16,
    params: &FieldParams,
    coloring_seed: u64,
) -> Option<GlyphField> {
    let p = params;
    let id = GlyphId(gid);
    let bbox = face.glyph_bounding_box(id)?;
    let mut shape = fdsm_ttf_parser::load_shape_from_face(face, id)?;
    let tpu = p.em_texels / f32::from(face.units_per_em());
    // Padding: the field saturates ρ/2 texels from the outline; one more texel
    // keeps the bilinear footprint at the slot border fully "outside".
    let pad = (p.px_range / 2.0).ceil() + 1.0;
    let ink_w = f32::from(bbox.x_max - bbox.x_min) * tpu;
    let ink_h = f32::from(bbox.y_max - bbox.y_min) * tpu;
    let slot_w = slot_edge(ink_w + 2.0 * pad, p)?;
    let slot_h = slot_edge(ink_h + 2.0 * pad, p)?;
    // Center the ink in the slot (integer texel offset keeps origins exact).
    let left = ((slot_w as f32 - ink_w) / 2.0).floor();
    let top = ((slot_h as f32 - ink_h) / 2.0).floor();
    let origin = (
        left - f32::from(bbox.x_min) * tpu,
        top + f32::from(bbox.y_max) * tpu,
    );

    // Font units (y up) → texels (y down).
    let t = f64::from(tpu);
    let m = Matrix3::new(
        t,
        0.0,
        f64::from(origin.0), //
        0.0,
        -t,
        f64::from(origin.1), //
        0.0,
        0.0,
        1.0,
    );
    shape.transform(&Affine2::from_matrix_unchecked(m));
    let colored = Shape::edge_coloring_simple(shape, 0.03, coloring_seed);
    let prepared = colored.prepare();
    let range = f64::from(p.px_range);
    let mut img: ImageBuffer<Rgb<f32>, Vec<f32>> = ImageBuffer::new(slot_w, slot_h);
    generate_msdf(&prepared, range, &mut img);
    correct_error_msdf(
        &mut img,
        &colored,
        &prepared,
        range,
        &ErrorCorrectionConfig::default(),
    );
    correct_sign_msdf(&mut img, &prepared, FillRule::Nonzero);

    let texels = img
        .pixels()
        .map(|px| if median3(px.0) <= 0.0 { [0.0; 3] } else { px.0 })
        .collect();
    Some(GlyphField {
        image: DistanceImage {
            width: slot_w as usize,
            height: slot_h as usize,
            px_range: p.px_range,
            texels,
        },
        origin,
        texels_per_unit: tpu,
        ink: Rect {
            x: left,
            y: top,
            w: ink_w,
            h: ink_h,
        },
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]
    use super::*;
    use crate::shred::median3;

    const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");

    fn inside(f: &GlyphField, x: f32, y: f32) -> bool {
        let p = crate::shred::Point { x, y };
        median3(f.image.bilinear(p)) > 0.5
    }

    #[test]
    fn letter_o_has_a_hole_and_pow2_slots() {
        let face = Face::parse(LATIN, 0).unwrap();
        let gid = face.glyph_index('O').unwrap().0;
        let f = glyph_field(&face, gid, &FieldParams::default(), 1).unwrap();
        assert!(f.image.width.is_power_of_two() && f.image.height.is_power_of_two());
        let (cx, cy) = (f.ink.x + f.ink.w / 2.0, f.ink.y + f.ink.h / 2.0);
        assert!(!inside(&f, cx, cy), "counter of O is outside");
        assert!(inside(&f, f.ink.x + 1.5, cy), "left stroke is inside");
        assert!(!inside(&f, 0.5, 0.5), "slot corner is outside");
        // Baseline: the O sits on it, so its bottom ink edge is near origin.y.
        assert!((f.ink.y + f.ink.h - f.origin.1).abs() < 2.0);
    }

    #[test]
    fn far_field_and_slot_border_are_zero_in_every_channel() {
        let face = Face::parse(LATIN, 0).unwrap();
        for c in "gOW&@".chars() {
            let gid = face.glyph_index(c).unwrap().0;
            let field = glyph_field(&face, gid, &FieldParams::default(), 3).unwrap();
            let (width, height) = (field.image.width, field.image.height);
            for (i, texel) in field.image.texels.iter().enumerate() {
                let (col, row) = (i % width, i / width);
                let border = col == 0 || row == 0 || col == width - 1 || row == height - 1;
                if border || median3(*texel) <= 0.0 {
                    assert_eq!(*texel, [0.0; 3], "{c}: texel ({col},{row}) = {texel:?}");
                }
            }
        }
    }

    #[test]
    fn space_has_no_field() {
        let face = Face::parse(LATIN, 0).unwrap();
        let gid = face.glyph_index(' ').unwrap().0;
        assert!(glyph_field(&face, gid, &FieldParams::default(), 1).is_none());
    }
}
