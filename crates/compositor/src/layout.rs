//! Paragraph layout: items → Knuth–Plass lines → justified, bidi-reordered,
//! positioned glyphs, plus the per-cluster boxes used for hit testing.

use crate::bidi::visual_order;
use crate::item::{GlyphRef, Item, ItemParams, RunView, build_items, flags};
use crate::linebreak::{BreakParams, Paragraph, Placed, Quality, break_paragraph, justify};

/// What a positioned glyph is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphKind {
    Glyph,
    /// Tatweel scaled horizontally by `scale_x` (kashida elongation).
    Kashida,
    Hyphen,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub kind: GlyphKind,
    pub run: u16,
    /// Glyph index in its run (the joining glyph for kashida; the break glyph for hyphen).
    pub index: u32,
    /// Permuted glyph id to draw.
    pub gid: u16,
    /// Pen position (line-relative device px).
    pub x: f32,
    /// Horizontal scale (1 except for kashida).
    pub scale_x: f32,
}

/// Visual extent of one cluster: what a tap on `[x0, x1)` selects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterBox {
    pub x0: f32,
    pub x1: f32,
    /// Paragraph-relative cluster ordinal (the third anchor component).
    pub cluster: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineLayout {
    pub glyphs: Vec<PositionedGlyph>,
    pub clusters: Vec<ClusterBox>,
    /// Sum of placed widths (equals the target width on justified lines).
    pub advance: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParagraphLayout {
    pub lines: Vec<LineLayout>,
    pub quality: Quality,
}

/// A unit in L2 reordering: a whole cluster, a space, a kashida, or a hyphen.
struct Unit {
    level: u8,
    width: f32,
    glyphs: Vec<PositionedGlyph>,
    cluster: Option<u32>,
}

fn level_of(runs: &[RunView<'_>], g: GlyphRef) -> u8 {
    runs.get(usize::from(g.run))
        .and_then(|r| r.bidi_levels.get(g.index as usize))
        .copied()
        .unwrap_or(0)
}

/// Paragraph-relative cluster ordinal for every glyph (the anchor's third component).
fn cluster_ordinals(runs: &[RunView<'_>]) -> Vec<Vec<u32>> {
    let mut out = Vec::with_capacity(runs.len());
    let mut ordinal: u32 = 0;
    let mut seen_any = false;
    for run in runs {
        let mut v = Vec::with_capacity(run.flags.len());
        for f in run.flags {
            if f & flags::CLUSTER_START != 0 {
                if seen_any {
                    ordinal += 1;
                }
                seen_any = true;
            }
            v.push(ordinal);
        }
        out.push(v);
    }
    out
}

fn special_glyph(kind: GlyphKind, g: GlyphRef, gid: u16, scale_x: f32) -> PositionedGlyph {
    PositionedGlyph {
        kind,
        run: g.run,
        index: g.index,
        gid,
        x: 0.0,
        scale_x,
    }
}

/// Groups a justified line (logical order) into L2 units: whole clusters,
/// spaces, kashidas and hyphens.
fn line_units(
    runs: &[RunView<'_>],
    items: &[Item],
    placed: &[Placed],
    clusters: &[Vec<u32>],
) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    for p in placed {
        match *p {
            Placed::Glyph { item, width } => {
                let g = items[item].glyph;
                let run = &runs[usize::from(g.run)];
                let i = g.index as usize;
                let starts = run
                    .flags
                    .get(i)
                    .is_some_and(|f| f & flags::CLUSTER_START != 0);
                let glyph = special_glyph(
                    GlyphKind::Glyph,
                    g,
                    run.gids.get(i).copied().unwrap_or(0),
                    1.0,
                );
                match units.last_mut() {
                    Some(u) if !starts && u.cluster.is_some() => {
                        u.width += width;
                        u.glyphs.push(glyph);
                    }
                    _ => units.push(Unit {
                        level: level_of(runs, g),
                        width,
                        glyphs: vec![glyph],
                        cluster: clusters
                            .get(usize::from(g.run))
                            .and_then(|v| v.get(i))
                            .copied(),
                    }),
                }
            }
            Placed::Space { item, width } => {
                units.push(Unit {
                    level: level_of(runs, items[item].glyph),
                    width,
                    glyphs: Vec::new(),
                    cluster: None,
                });
            }
            Placed::Kashida { item, width } => {
                let g = items[item].glyph;
                let run = &runs[usize::from(g.run)];
                let (tatweel, adv) = run.tatweel.unwrap_or((0, 1));
                let scale_x = width / (f32::from(adv.max(1)) * run.scale);
                let glyph = special_glyph(GlyphKind::Kashida, g, tatweel, scale_x);
                units.push(Unit {
                    level: level_of(runs, g),
                    width,
                    glyphs: vec![glyph],
                    cluster: None,
                });
            }
            Placed::Hyphen { item, width } => {
                let g = items[item].glyph;
                let gid = runs[usize::from(g.run)].hyphen.map_or(0, |(gid, _)| gid);
                let glyph = special_glyph(GlyphKind::Hyphen, g, gid, 1.0);
                units.push(Unit {
                    level: level_of(runs, g),
                    width,
                    glyphs: vec![glyph],
                    cluster: None,
                });
            }
        }
    }
    units
}

/// Applies L2 and assigns x positions, left to right.
fn place_units(
    runs: &[RunView<'_>],
    units: &[Unit],
    start_x: f32,
) -> (Vec<PositionedGlyph>, Vec<ClusterBox>) {
    let levels: Vec<u8> = units.iter().map(|u| u.level).collect();
    let mut x = start_x;
    let mut glyphs = Vec::new();
    let mut clusters = Vec::new();
    for &ui in &visual_order(&levels) {
        let u = &units[ui];
        let mut pen = x;
        for g in &u.glyphs {
            glyphs.push(PositionedGlyph { x: pen, ..*g });
            if g.kind == GlyphKind::Glyph {
                let run = &runs[usize::from(g.run)];
                pen +=
                    f32::from(run.advances.get(g.index as usize).copied().unwrap_or(0)) * run.scale;
            }
        }
        if let Some(cluster) = u.cluster {
            clusters.push(ClusterBox {
                x0: x,
                x1: x + u.width,
                cluster,
            });
        }
        x += u.width;
    }
    (glyphs, clusters)
}

/// Lays out one paragraph at constant measure `width`. `base_level` is the
/// paragraph direction (0 LTR, 1 RTL); RTL last lines align right.
#[must_use]
pub fn layout_paragraph(
    runs: &[RunView<'_>],
    width: f32,
    base_level: u8,
    item_params: &ItemParams,
    break_params: &BreakParams,
) -> ParagraphLayout {
    let items = build_items(runs, item_params);
    let para: Paragraph = break_paragraph(&items, &|_| width, break_params);
    let clusters = cluster_ordinals(runs);
    let lines = para
        .lines
        .iter()
        .map(|line| {
            let units = line_units(runs, &items, &justify(&items, line), &clusters);
            let advance: f32 = units.iter().map(|u| u.width).sum();
            // Ragged last line: align to the paragraph's start edge.
            let start_x = if line.last && base_level % 2 == 1 {
                (width - advance).max(0.0)
            } else {
                0.0
            };
            let (glyphs, clusters) = place_units(runs, &units, start_x);
            LineLayout {
                glyphs,
                clusters,
                advance,
            }
        })
        .collect();
    ParagraphLayout {
        lines,
        quality: para.quality,
    }
}

impl ParagraphLayout {
    /// Maps a point (line-relative x, line index) to the cluster under it,
    /// snapping to the nearest cluster when the point falls in a space.
    #[must_use]
    pub fn hit_test(&self, line: usize, x: f32) -> Option<u32> {
        let clusters = &self.lines.get(line)?.clusters;
        if let Some(c) = clusters.iter().find(|c| x >= c.x0 && x < c.x1) {
            return Some(c.cluster);
        }
        clusters
            .iter()
            .min_by(|a, b| {
                let da = (x - f32::midpoint(a.x0, a.x1)).abs();
                let db = (x - f32::midpoint(b.x0, b.x1)).abs();
                da.total_cmp(&db)
            })
            .map(|c| c.cluster)
    }
}
