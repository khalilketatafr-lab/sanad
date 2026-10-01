//! Knuth–Plass total-fit line breaking with kashida-first justification.
//!
//! Classic algorithm (Knuth & Plass, *Breaking Paragraphs into Lines*, 1981):
//! every legal breakpoint is evaluated against a set of active nodes. A line
//! from active node `a` to breakpoint `b` has an adjustment ratio and a
//! badness, and the chosen breaks minimize total demerits over the whole
//! paragraph. Browsers break greedily line by line; this optimizes the
//! paragraph as a whole, which is what removes "rivers".
//!
//! # Arabic: kashida before glue, never letter-spacing
//!
//! A line's shortfall `Δ = target − natural` is absorbed in strict priority:
//!
//! 1. **Kashida** (tatweel elongation, at most one per word): if `Δ ≤ K`
//!    (total kashida capacity), only kashida stretch, with kashida ratio
//!    `κ = Δ/K` and glue untouched (`r = 0`, word spacing stays natural).
//!    Badness is `100·κ³·w_k`, with `w_k < 1`, so a kashida-justified line
//!    is preferred over a glue-stretched one.
//! 2. **Glue**: only once kashida is exhausted (`κ = 1`):
//!    `r = (Δ − K)/Y`, badness `100·r³ + 100·w_k`.
//!
//! Shrinking uses glue only (kashida cannot shrink). Boxes have no stretch
//! or shrink by construction (see [`crate::item`]), so letter-spacing cannot
//! happen, and tests assert intra-word distances are exactly the advances.

use crate::item::{INFINITE_PENALTY, Item, ItemKind};

#[derive(Debug, Clone, Copy)]
pub struct BreakParams {
    /// Max glue ratio accepted in a pass (TeX tolerance ~ badness 100·ρ³).
    pub tolerance: f32,
    pub line_penalty: f64,
    pub flagged_demerits: f64,
    pub fitness_demerits: f64,
    /// Badness weight of kashida elongation relative to glue (< 1 prefers kashida).
    pub kashida_weight: f64,
}

impl Default for BreakParams {
    fn default() -> Self {
        Self {
            tolerance: 2.0,
            line_penalty: 10.0,
            flagged_demerits: 3000.0,
            fitness_demerits: 100.0,
            kashida_weight: 0.5,
        }
    }
}

/// One chosen line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// First item of the line (after discarding glue/penalties at the break).
    pub start: usize,
    /// The breakpoint item (exclusive for glue, included if a hyphen penalty).
    pub end: usize,
    /// Glue adjustment ratio: > 0 stretch, < 0 shrink, 0 natural.
    pub glue_ratio: f32,
    /// Fraction of each kashida's maximum elongation applied (0 ≤ κ ≤ 1).
    pub kashida_ratio: f32,
    /// Target width of this line.
    pub width: f32,
    /// True when the line ends at a hyphenation penalty (insert a hyphen).
    pub hyphenated: bool,
    /// Last line of the paragraph (set ragged, not justified).
    pub last: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Every line within `tolerance`.
    Optimal,
    /// Needed the relaxed second pass.
    Relaxed,
    /// Some line could not be set within tolerance: overfull (a word wider
    /// than the measure) or underfull with nothing to stretch (a lone long
    /// word). Callers may retry with hyphenation or a wider measure.
    Emergency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub lines: Vec<Line>,
    pub quality: Quality,
    pub demerits: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Sums {
    width: f64,
    stretch: f64,
    shrink: f64,
    kashida: f64,
    fil: u32,
}

#[derive(Debug, Clone, Copy)]
struct Node {
    position: usize,
    line: usize,
    fitness: u8,
    totals: Sums,
    demerits: f64,
    prev: Option<usize>,
    glue_ratio: f32,
    kashida_ratio: f32,
}

struct Measure {
    glue_ratio: f32,
    kashida_ratio: f32,
    badness: f64,
}

fn prefix_sums(items: &[Item]) -> Vec<Sums> {
    let mut out = Vec::with_capacity(items.len() + 1);
    let mut s = Sums::default();
    out.push(s);
    for it in items {
        match it.kind {
            ItemKind::Box => s.width += f64::from(it.width),
            ItemKind::Glue => {
                s.width += f64::from(it.width);
                s.stretch += f64::from(it.stretch);
                s.shrink += f64::from(it.shrink);
                s.fil += u32::from(it.fil);
            }
            ItemKind::Kashida => s.kashida += f64::from(it.stretch),
            ItemKind::Penalty => {}
        }
        out.push(s);
    }
    out
}

fn is_legal_break(items: &[Item], b: usize) -> bool {
    let Some(it) = items.get(b) else { return false };
    match it.kind {
        ItemKind::Glue => b > 0 && matches!(items[b - 1].kind, ItemKind::Box),
        ItemKind::Penalty => it.penalty < INFINITE_PENALTY,
        ItemKind::Box | ItemKind::Kashida => false,
    }
}

/// Totals as they stand at the start of the line following a break at `b`
/// (glue and penalties after a break are discarded, up to the next box).
fn totals_after(items: &[Item], sums: &[Sums], b: usize) -> Sums {
    let mut t = sums[b];
    for (j, it) in items.iter().enumerate().skip(b) {
        match it.kind {
            ItemKind::Box => break,
            ItemKind::Glue => {
                t.width += f64::from(it.width);
                t.stretch += f64::from(it.stretch);
                t.shrink += f64::from(it.shrink);
                t.fil += u32::from(it.fil);
            }
            ItemKind::Kashida => t.kashida += f64::from(it.stretch),
            ItemKind::Penalty if j > b && it.is_forced_break() => break,
            ItemKind::Penalty => {}
        }
    }
    t
}

#[allow(clippy::many_single_char_names)] // k, y, z, r: the paper's notation
fn measure(
    items: &[Item],
    sums: &[Sums],
    a: &Node,
    b: usize,
    target: f64,
    kashida_weight: f64,
) -> Measure {
    let end = sums[b];
    let penalty_width = match items[b].kind {
        ItemKind::Penalty => f64::from(items[b].width),
        _ => 0.0,
    };
    let natural = end.width - a.totals.width + penalty_width;
    let fil = end.fil - a.totals.fil;
    if natural < target {
        if fil > 0 {
            return Measure {
                glue_ratio: 0.0,
                kashida_ratio: 0.0,
                badness: 0.0,
            };
        }
        let delta = target - natural;
        let k = end.kashida - a.totals.kashida;
        let y = end.stretch - a.totals.stretch;
        if k > 0.0 && delta <= k {
            let kr = delta / k;
            return Measure {
                glue_ratio: 0.0,
                kashida_ratio: kr as f32,
                badness: 100.0 * kr.powi(3) * kashida_weight,
            };
        }
        let rest = delta - k.max(0.0);
        let kashida_ratio = if k > 0.0 { 1.0 } else { 0.0 };
        let kashida_cost = if k > 0.0 { 100.0 * kashida_weight } else { 0.0 };
        if y <= 0.0 {
            return Measure {
                glue_ratio: f32::INFINITY,
                kashida_ratio,
                badness: f64::INFINITY,
            };
        }
        let r = rest / y;
        Measure {
            glue_ratio: r as f32,
            kashida_ratio,
            badness: 100.0 * r.powi(3) + kashida_cost,
        }
    } else if natural > target {
        let z = end.shrink - a.totals.shrink;
        if z <= 0.0 {
            return Measure {
                glue_ratio: f32::NEG_INFINITY,
                kashida_ratio: 0.0,
                badness: f64::INFINITY,
            };
        }
        let r = -(natural - target) / z;
        Measure {
            glue_ratio: r as f32,
            kashida_ratio: 0.0,
            badness: 100.0 * r.abs().powi(3),
        }
    } else {
        Measure {
            glue_ratio: 0.0,
            kashida_ratio: 0.0,
            badness: 0.0,
        }
    }
}

fn fitness(glue_ratio: f32) -> u8 {
    if glue_ratio < -0.5 {
        0 // tight
    } else if glue_ratio <= 0.5 {
        1 // decent
    } else if glue_ratio <= 1.0 {
        2 // loose
    } else {
        3 // very loose
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pass {
    Strict(f32),
    Emergency,
}

fn run_pass(
    items: &[Item],
    sums: &[Sums],
    widths: &dyn Fn(usize) -> f32,
    params: &BreakParams,
    pass: Pass,
) -> Option<(Vec<Node>, usize)> {
    let mut nodes = vec![Node {
        position: 0,
        line: 0,
        fitness: 1,
        totals: Sums::default(),
        demerits: 0.0,
        prev: None,
        glue_ratio: 0.0,
        kashida_ratio: 0.0,
    }];
    let mut active: Vec<usize> = vec![0];

    for b in 0..items.len() {
        if !is_legal_break(items, b) {
            continue;
        }
        let item = items[b];
        // Best candidate per fitness class: (demerits, from node, measure).
        let mut best: [Option<(f64, usize, f32, f32)>; 4] = [None; 4];
        let mut keep = Vec::with_capacity(active.len());
        for &ai in &active {
            let a = nodes[ai];
            let target = f64::from(widths(a.line));
            let m = measure(items, sums, &a, b, target, params.kashida_weight);
            let overfull = m.glue_ratio < -1.0;
            if !(overfull || item.is_forced_break()) {
                keep.push(ai);
            }
            let feasible = match pass {
                Pass::Strict(tol) => !overfull && m.glue_ratio <= tol,
                // Emergency: accept anything, but make overfull/underfull lines very costly.
                Pass::Emergency => true,
            };
            if !feasible {
                continue;
            }
            let badness = if m.badness.is_finite() {
                m.badness.min(1.0e6)
            } else {
                1.0e6
            };
            let badness = if overfull { 1.0e7 } else { badness };
            let lp = params.line_penalty + badness;
            let p = f64::from(item.penalty);
            let mut d = if matches!(item.kind, ItemKind::Penalty) && p >= 0.0 {
                lp * lp + p * p
            } else if matches!(item.kind, ItemKind::Penalty) && p > -f64::from(INFINITE_PENALTY) {
                lp * lp - p * p
            } else {
                lp * lp
            };
            if item.flagged && items[a.position].flagged && a.position != 0 {
                d += params.flagged_demerits;
            }
            let fit = fitness(m.glue_ratio);
            if fit.abs_diff(a.fitness) > 1 {
                d += params.fitness_demerits;
            }
            let total = a.demerits + d;
            let slot = &mut best[usize::from(fit)];
            if slot.is_none_or(|(t, ..)| total < t) {
                *slot = Some((total, ai, m.glue_ratio, m.kashida_ratio));
            }
        }
        active = keep;
        let min = best
            .iter()
            .flatten()
            .map(|c| c.0)
            .fold(f64::INFINITY, f64::min);
        if min.is_finite() {
            let after = totals_after(items, sums, b);
            for (fit, cand) in best.iter().enumerate() {
                let Some((total, from, gr, kr)) = *cand else {
                    continue;
                };
                if total <= min + params.fitness_demerits {
                    nodes.push(Node {
                        position: b,
                        line: nodes[from].line + 1,
                        fitness: fit as u8,
                        totals: after,
                        demerits: total,
                        prev: Some(from),
                        glue_ratio: gr,
                        kashida_ratio: kr,
                    });
                    active.push(nodes.len() - 1);
                }
            }
        }
        if active.is_empty() {
            return None;
        }
    }
    let last = items.len().checked_sub(1)?;
    let best = active
        .iter()
        .copied()
        .filter(|&i| nodes[i].position == last)
        .min_by(|&x, &y| nodes[x].demerits.total_cmp(&nodes[y].demerits))?;
    Some((nodes, best))
}

/// Breaks a paragraph into lines. `widths(line)` gives each line's target
/// width (e.g. for a drop cap or a float, or a constant measure).
///
/// Always returns a result: strict pass → relaxed pass (×5 tolerance) →
/// emergency pass that accepts overfull lines (reported via [`Quality`]).
#[must_use]
pub fn break_paragraph(
    items: &[Item],
    widths: &dyn Fn(usize) -> f32,
    params: &BreakParams,
) -> Paragraph {
    let sums = prefix_sums(items);
    let attempts = [
        (Pass::Strict(params.tolerance), Quality::Optimal),
        (Pass::Strict(params.tolerance * 5.0), Quality::Relaxed),
        (Pass::Emergency, Quality::Emergency),
    ];
    for (pass, quality) in attempts {
        if let Some((nodes, best)) = run_pass(items, &sums, widths, params, pass) {
            let mut chain = Vec::new();
            let mut cur = Some(best);
            while let Some(i) = cur {
                if nodes[i].prev.is_some() {
                    chain.push(i);
                }
                cur = nodes[i].prev;
            }
            chain.reverse();
            let mut lines = Vec::with_capacity(chain.len());
            for (n, &i) in chain.iter().enumerate() {
                let node = nodes[i];
                let from = node.prev.map_or(0, |p| nodes[p].position);
                let start = if n == 0 {
                    0
                } else {
                    first_box_after(items, from)
                };
                let it = items[node.position];
                lines.push(Line {
                    start,
                    end: node.position,
                    glue_ratio: if node.glue_ratio.is_finite() {
                        node.glue_ratio
                    } else {
                        0.0
                    },
                    kashida_ratio: node.kashida_ratio,
                    width: widths(node.line - 1),
                    hyphenated: matches!(it.kind, ItemKind::Penalty) && it.flagged,
                    last: n + 1 == chain.len(),
                });
            }
            let demerits = nodes[best].demerits;
            let quality =
                if quality == Quality::Emergency || lines.iter().any(|l| l.glue_ratio < -1.0) {
                    Quality::Emergency
                } else {
                    quality
                };
            return Paragraph {
                lines,
                quality,
                demerits,
            };
        }
    }
    Paragraph {
        lines: Vec::new(),
        quality: Quality::Emergency,
        demerits: f64::INFINITY,
    }
}

fn first_box_after(items: &[Item], b: usize) -> usize {
    items
        .iter()
        .enumerate()
        .skip(b + 1)
        .find(|(_, it)| matches!(it.kind, ItemKind::Box))
        .map_or(items.len(), |(j, _)| j)
}

/// A positioned element of a line, in logical order, x relative to the line
/// start (not yet bidi-reordered).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placed {
    /// A glyph at its natural advance (never letter-spaced).
    Glyph { item: usize, width: f32 },
    /// Inter-word space after justification.
    Space { item: usize, width: f32 },
    /// Tatweel elongation after a joining glyph. Rendered as the tatweel
    /// glyph scaled horizontally to `width`.
    Kashida { item: usize, width: f32 },
    /// Hyphen inserted at a hyphenated line end.
    Hyphen { item: usize, width: f32 },
}

impl Placed {
    #[must_use]
    pub const fn width(&self) -> f32 {
        match *self {
            Self::Glyph { width, .. }
            | Self::Space { width, .. }
            | Self::Kashida { width, .. }
            | Self::Hyphen { width, .. } => width,
        }
    }
}

/// Distributes a line's adjustment: kashida first (κ × max each), then glue.
/// Boxes keep their natural width. Last lines are set at natural spacing.
#[must_use]
pub fn justify(items: &[Item], line: &Line) -> Vec<Placed> {
    let (gr, kr) = if line.last {
        (0.0f32.min(line.glue_ratio), 0.0)
    } else {
        (line.glue_ratio, line.kashida_ratio)
    };
    let mut out = Vec::new();
    for (j, it) in items.iter().enumerate().take(line.end).skip(line.start) {
        match it.kind {
            ItemKind::Box => out.push(Placed::Glyph {
                item: j,
                width: it.width,
            }),
            ItemKind::Glue if !it.fil => {
                let w = if gr >= 0.0 {
                    it.width + gr * it.stretch
                } else {
                    it.width + gr * it.shrink
                };
                out.push(Placed::Space { item: j, width: w });
            }
            ItemKind::Kashida if kr > 0.0 => out.push(Placed::Kashida {
                item: j,
                width: kr * it.stretch,
            }),
            _ => {}
        }
    }
    if line.hyphenated {
        out.push(Placed::Hyphen {
            item: line.end,
            width: items[line.end].width,
        });
    }
    out
}
