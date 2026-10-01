//! Golden-page composition: shaped paragraphs → permuted runs → Compositor
//! lines → Lumen Pass 1 glyph instances.
//!
//! This runs the client's layout code (the Compositor) natively, over the
//! same permuted data a reader would decrypt, and emits exactly the instance
//! buffer Lumen draws (`GLYPH_INSTANCE`, 40 bytes each). In production the
//! Compositor does this in WASM on the device; here it produces reference
//! pages for visual review and regression tests.

use sanad_compositor::item::{ItemParams, RunView};
use sanad_compositor::layout::{
    GlyphKind, Measure, ParagraphLayout, PositionedGlyph, layout_paragraph_with,
};
use sanad_compositor::linebreak::{BreakParams, Quality};
use thiserror::Error;

use crate::atlas::Atlas;
use crate::permute::{GlyphKey, Permutation};
use crate::shape::{FontId, ShapedParagraph, Typesetter};

/// Page geometry in device px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSpec {
    pub width: f32,
    pub height: f32,
    pub margin_x: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Line pitch as a multiple of the paragraph's largest em.
    pub leading: f32,
    /// Extra leading for paragraphs that contain Arabic (taller ascenders,
    /// descenders and stacked marks).
    pub leading_arabic: f32,
    /// First-line indent of every paragraph but the first, in ems.
    pub indent_em: f32,
}

/// One Pass 1 instance (`GLYPH_INSTANCE` in `packages/lumen/src/gl/shaders.ts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    /// Page-space quad x, y, w, h.
    pub rect: [f32; 4],
    /// Atlas slot x, y, w, h in texels.
    pub slot: [f32; 4],
    /// 0 ink · 1 ink2 · 2 accent · 3 highlight.
    pub role: u32,
    pub highlight: f32,
}

pub const INSTANCE_STRIDE: usize = 40;

impl Instance {
    /// Little-endian bytes in Lumen's instance layout.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; INSTANCE_STRIDE] {
        let mut out = [0u8; INSTANCE_STRIDE];
        for (i, v) in self.rect.iter().chain(&self.slot).enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out[32..36].copy_from_slice(&self.role.to_le_bytes());
        out[36..40].copy_from_slice(&self.highlight.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageStats {
    pub paragraphs: usize,
    pub lines: usize,
    pub glyphs: usize,
    pub fragments: usize,
    pub kashidas: usize,
    pub hyphens: usize,
    pub relaxed_or_worse: usize,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub instances: Vec<Instance>,
    pub stats: PageStats,
    /// Paragraphs (from the start of the input) that fit on the page.
    pub paragraphs_set: usize,
}

impl Page {
    #[must_use]
    pub fn instance_bytes(&self) -> Vec<u8> {
        self.instances.iter().flat_map(Instance::to_bytes).collect()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PageError {
    #[error("glyph {gid} of font {font} is not in the edition's permutation")]
    Unpermuted { font: u8, gid: u16 },
}

/// A run with edition (permuted) glyph ids; the rest is borrowed.
struct EditionRun<'p> {
    font: FontId,
    gids: Vec<u16>,
    hyphen: Option<(u16, i16)>,
    tatweel: Option<(u16, i16)>,
    src: &'p crate::shape::ShapedRun,
}

fn permuted(perm: &Permutation, font: FontId, gid: u16) -> Result<u16, PageError> {
    perm.get(GlyphKey { font, gid })
        .ok_or(PageError::Unpermuted { font: font.0, gid })
}

fn edition_runs<'p>(
    ts: &Typesetter<'_>,
    para: &'p ShapedParagraph,
    perm: &Permutation,
) -> Result<Vec<EditionRun<'p>>, PageError> {
    para.runs
        .iter()
        .map(|r| {
            let gids = r
                .gids
                .iter()
                .map(|&g| permuted(perm, r.font, g))
                .collect::<Result<Vec<_>, _>>()?;
            let sp = ts.specials(r.font);
            let special = |s: Option<(u16, i16)>| -> Result<Option<(u16, i16)>, PageError> {
                s.map(|(g, adv)| permuted(perm, r.font, g).map(|p| (p, adv)))
                    .transpose()
            };
            Ok(EditionRun {
                font: r.font,
                gids,
                hyphen: special(sp.hyphen)?,
                tatweel: special(sp.tatweel)?,
                src: r,
            })
        })
        .collect()
}

/// Every glyph a set of paragraphs can draw, including inserted hyphens and
/// tatweels: the keys to permute and rasterize.
pub fn used_glyphs<'a>(
    ts: &'a Typesetter<'_>,
    paragraphs: &'a [ShapedParagraph],
) -> impl Iterator<Item = GlyphKey> + 'a {
    paragraphs.iter().flat_map(move |p| {
        p.runs.iter().flat_map(move |r| {
            let sp = ts.specials(r.font);
            r.gids
                .iter()
                .copied()
                .chain(sp.hyphen.map(|h| h.0))
                .chain(sp.tatweel.map(|t| t.0))
                .map(move |gid| GlyphKey { font: r.font, gid })
        })
    })
}

impl EditionRun<'_> {
    fn view(&self) -> RunView<'_> {
        RunView {
            scale: self.src.scale,
            gids: &self.gids,
            advances: &self.src.advances,
            offsets: &self.src.offsets,
            flags: &self.src.flags,
            bidi_levels: &self.src.levels,
            kashida_priority: &self.src.kashida_priority,
            kashida_max: &self.src.kashida_max,
            hyphen: self.hyphen,
            tatweel: self.tatweel,
        }
    }
}

/// One instance per fragment of a positioned glyph, at pen `(left + g.x,
/// baseline)`. Glyphs without ink (spaces) have no atlas entry.
fn emit_glyph(
    out: &mut Page,
    atlas: &Atlas,
    runs: &[EditionRun<'_>],
    g: &PositionedGlyph,
    left: f32,
    baseline: f32,
) {
    let Some(entry) = atlas.glyphs.get(&g.gid) else {
        return;
    };
    // Device px per atlas texel.
    let ppt = runs[usize::from(g.run)].src.scale / entry.texels_per_unit;
    match g.kind {
        GlyphKind::Kashida => out.stats.kashidas += 1,
        GlyphKind::Hyphen => out.stats.hyphens += 1,
        GlyphKind::Glyph => {}
    }
    out.stats.glyphs += 1;
    for s in &entry.slots {
        out.instances.push(Instance {
            rect: [
                left + g.x - entry.origin.0 * ppt * g.scale_x,
                baseline - g.y - entry.origin.1 * ppt,
                s.w as f32 * ppt * g.scale_x,
                s.h as f32 * ppt,
            ],
            slot: [s.x as f32, s.y as f32, s.w as f32, s.h as f32],
            role: 0,
            highlight: 0.0,
        });
        out.stats.fragments += 1;
    }
}

/// Sets as many paragraphs as fit on one page.
pub fn compose_page(
    ts: &Typesetter<'_>,
    paragraphs: &[ShapedParagraph],
    perm: &Permutation,
    atlas: &Atlas,
    spec: &PageSpec,
) -> Result<Page, PageError> {
    let measure = spec.width - 2.0 * spec.margin_x;
    let bottom = spec.height - spec.margin_bottom;
    let mut out = Page {
        instances: Vec::new(),
        stats: PageStats::default(),
        paragraphs_set: 0,
    };
    let mut cursor = spec.margin_top;

    for (pi, para) in paragraphs.iter().enumerate() {
        let runs = edition_runs(ts, para, perm)?;
        let views: Vec<RunView<'_>> = runs.iter().map(EditionRun::view).collect();
        let em = runs
            .iter()
            .map(|r| ts.font(r.font).size_px)
            .fold(0.0f32, f32::max);
        let has_arabic = runs.iter().any(|r| r.font == ts.arabic_font());
        let pitch = em
            * if has_arabic {
                spec.leading_arabic
            } else {
                spec.leading
            };
        let base_font = if para.base_level % 2 == 1 {
            ts.arabic_font()
        } else {
            ts.latin_font()
        };
        let base_em = ts.font(base_font).size_px;
        let indent = if pi == 0 {
            0.0
        } else {
            spec.indent_em * base_em
        };
        let layout: ParagraphLayout = layout_paragraph_with(
            &views,
            Measure {
                width: measure,
                first_indent: indent,
            },
            para.base_level,
            &ItemParams::default(),
            &BreakParams::default(),
        );
        // Baselines sit ~¾ down each line box.
        let first_baseline = cursor + pitch * 0.75;
        let last_line_bottom = cursor + pitch * layout.lines.len() as f32;
        if last_line_bottom > bottom {
            break; // golden pages set whole paragraphs only
        }
        for (li, line) in layout.lines.iter().enumerate() {
            let baseline = first_baseline + pitch * li as f32;
            for g in &line.glyphs {
                emit_glyph(&mut out, atlas, &runs, g, spec.margin_x, baseline);
            }
        }
        out.stats.paragraphs += 1;
        out.stats.lines += layout.lines.len();
        if layout.quality != Quality::Optimal {
            out.stats.relaxed_or_worse += 1;
        }
        out.paragraphs_set = pi + 1;
        cursor = last_line_bottom;
    }
    Ok(out)
}
