#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use proptest::prelude::*;
use sanad_compositor::item::{GlyphRef, ItemKind, ItemParams, RunView, build_items, flags as F};
use sanad_compositor::layout::{GlyphKind, layout_paragraph};
use sanad_compositor::linebreak::{BreakParams, Placed, Quality, break_paragraph, justify};

const SCALE: f32 = 0.02; // 1000 upem at 20 px
const GLYPH: i16 = 500; // 10 px
const SPACE: i16 = 250; // 5 px
const TATWEEL: (u16, i16) = (4242, 300);
const HYPHEN: (u16, i16) = (99, 330);

/// SoA buffers for one run, built word by word.
#[derive(Default)]
struct Run {
    gids: Vec<u16>,
    adv: Vec<i16>,
    flags: Vec<u8>,
    levels: Vec<u8>,
    kprio: Vec<u8>,
    kmax: Vec<u16>,
}

impl Run {
    fn word(&mut self, len: usize, level: u8, kashida: &[(usize, u8)], hyphen_after: &[usize]) {
        if !self.gids.is_empty() {
            self.gids.push(3);
            self.adv.push(SPACE);
            self.flags.push(F::CLUSTER_START | F::GLUE | F::BREAK_OK);
            self.levels.push(level);
            self.kprio.push(0);
            self.kmax.push(0);
        }
        for i in 0..len {
            let mut f = F::CLUSTER_START;
            if i == 0 {
                f |= F::WORD_START;
            }
            let k = kashida.iter().find(|(at, _)| *at == i);
            if k.is_some() {
                f |= F::KASHIDA_OK;
            }
            if hyphen_after.contains(&i) {
                f |= F::HYPHEN_POINT;
            }
            self.gids.push(1000 + (self.gids.len() as u16 % 50));
            self.adv.push(GLYPH);
            self.flags.push(f);
            self.levels.push(level);
            self.kprio.push(k.map_or(0, |(_, p)| *p));
            self.kmax.push(if k.is_some() { 800 } else { 0 }); // 16 px max elongation
        }
    }

    fn view(&self) -> RunView<'_> {
        RunView {
            scale: SCALE,
            gids: &self.gids,
            advances: &self.adv,
            flags: &self.flags,
            bidi_levels: &self.levels,
            kashida_priority: &self.kprio,
            kashida_max: &self.kmax,
            hyphen: Some(HYPHEN),
            tatweel: Some(TATWEEL),
        }
    }
}

fn latin(lengths: &[usize]) -> Run {
    let mut r = Run::default();
    for &l in lengths {
        r.word(l, 0, &[], &[]);
    }
    r
}

fn lengths(seed: u64, n: usize) -> Vec<usize> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            2 + (s >> 61) as usize // 2..=9 glyphs (20–90 px): prose-like at a 260–300 px measure
        })
        .collect()
}

#[test]
fn justified_lines_fill_the_measure_exactly() {
    let run = latin(&lengths(7, 60));
    let p = layout_paragraph(
        &[run.view()],
        300.0,
        0,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    assert_ne!(p.quality, Quality::Emergency);
    assert!(p.lines.len() > 5);
    for (i, line) in p.lines.iter().enumerate() {
        if i + 1 < p.lines.len() {
            assert!(
                (line.advance - 300.0).abs() < 1e-3,
                "line {i} advance {}",
                line.advance
            );
        } else {
            assert!(line.advance <= 300.0 + 1e-3, "last line overfull");
        }
    }
}

#[test]
fn boxes_never_stretch_no_letter_spacing() {
    let run = latin(&lengths(11, 60));
    let p = layout_paragraph(
        &[run.view()],
        260.0,
        0,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    for line in &p.lines {
        let glyphs: Vec<_> = line
            .glyphs
            .iter()
            .filter(|g| g.kind == GlyphKind::Glyph)
            .collect();
        for pair in glyphs.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let same_word = b.index == a.index + 1; // consecutive glyphs, no space between
            if same_word {
                let gap = b.x - a.x;
                assert!(
                    (gap - 10.0).abs() < 1e-4,
                    "intra-word distance {gap} != advance 10"
                );
            }
        }
    }
    // Structurally: no Box item carries stretch or shrink.
    let items = build_items(&[run.view()], &ItemParams::default());
    assert!(
        items
            .iter()
            .filter(|i| i.kind == ItemKind::Box)
            .all(|i| i.stretch == 0.0 && i.shrink == 0.0)
    );
}

fn fitness_class(r: f32) -> u8 {
    if r < -0.5 {
        0
    } else if r <= 0.5 {
        1
    } else if r <= 1.0 {
        2
    } else {
        3
    }
}

/// Greedy first-fit with the same items, as browsers do, scored with the
/// same objective as Knuth–Plass (line demerits + fitness-class demerits).
/// Returns None when greedy produces an infeasible line (outside tolerance,
/// or overfull), in which case there is nothing to compare.
#[allow(clippy::many_single_char_names)]
fn greedy_demerits(run: &Run, width: f32, params: &BreakParams) -> Option<f64> {
    let items = build_items(
        &[run.view()],
        &ItemParams {
            hyphenate: false,
            ..ItemParams::default()
        },
    );
    let n = items.len();
    // f64 throughout, like the Knuth–Plass implementation, so equal break sets score equally.
    let measure = |s: usize, e: usize| -> (f64, f64, f64) {
        let (mut w, mut y, mut z) = (0.0, 0.0, 0.0);
        for it in &items[s..e] {
            if matches!(it.kind, ItemKind::Box | ItemKind::Glue) && !it.fil {
                w += f64::from(it.width);
            }
            if it.kind == ItemKind::Glue && !it.fil {
                y += f64::from(it.stretch);
                z += f64::from(it.shrink);
            }
        }
        (w, y, z)
    };
    let width = f64::from(width);
    // Legal breaks: glue after a box, and the paragraph's final forced break.
    let legal = |b: usize| {
        b == n - 1
            || (items[b].kind == ItemKind::Glue && b > 0 && items[b - 1].kind == ItemKind::Box)
    };
    let mut total = 0.0;
    let mut prev_fit = 1u8;
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut b = 0;
    while b < n {
        if legal(b) {
            let (w, _, z) = measure(start, b);
            if w - z <= width {
                if b == n - 1 {
                    // Last line: fil stretch makes an underfull last line free, but
                    // a slightly overfull one must still shrink, and pays for it.
                    let r = if w > width { -(w - width) / z } else { 0.0 };
                    total += (params.line_penalty + 100.0 * r.abs().powi(3)).powi(2);
                    if fitness_class(r as f32).abs_diff(prev_fit) > 1 {
                        total += params.fitness_demerits;
                    }
                    return Some(total);
                }
                last_fit = Some(b);
            } else {
                let brk = last_fit?; // a single overfull word: greedy has no feasible line
                let (w, y, z) = measure(start, brk);
                let r = if w < width {
                    (width - w) / y
                } else {
                    -(w - width) / z
                };
                if !(-1.0..=f64::from(params.tolerance)).contains(&r) {
                    return None;
                }
                total += (params.line_penalty + 100.0 * r.abs().powi(3)).powi(2);
                let fit = fitness_class(r as f32);
                if fit.abs_diff(prev_fit) > 1 {
                    total += params.fitness_demerits;
                }
                prev_fit = fit;
                start = brk + 1;
                last_fit = None;
                b = brk;
            }
        }
        b += 1;
    }
    None
}

proptest! {
    /// Total-fit optimality: whenever greedy's breaks are feasible, Knuth–Plass
    /// finds a solution with no more demerits.
    #[test]
    fn knuth_plass_never_worse_than_greedy(seed in 0u64..10_000, width in 180.0f32..420.0) {
        let run = latin(&lengths(seed, 40));
        let params = BreakParams::default();
        if let Some(greedy) = greedy_demerits(&run, width, &params) {
            let items = build_items(&[run.view()], &ItemParams { hyphenate: false, ..ItemParams::default() });
            let kp = break_paragraph(&items, &|_| width, &params);
            // Both sides compute in f64; only summation order differs.
            prop_assert!(kp.demerits <= greedy * (1.0 + 1e-9), "kp {} > greedy {}", kp.demerits, greedy);
        }
    }
}

fn arabic(words: usize, kashida_per_word: &[(usize, u8)]) -> Run {
    let mut r = Run::default();
    for _ in 0..words {
        r.word(6, 1, kashida_per_word, &[]);
    }
    r
}

#[test]
fn arabic_slack_goes_to_kashida_before_word_spaces() {
    // 6-glyph words = 60 px, space 5 px. Width 200 → 3 words (190 px) per line, slack 10 px,
    // kashida capacity 3 × 16 px = 48 px ≥ slack: spaces must stay natural.
    let run = arabic(12, &[(2, 1)]);
    let items = build_items(&[run.view()], &ItemParams::default());
    let para = break_paragraph(&items, &|_| 200.0, &BreakParams::default());
    let justified: Vec<_> = para.lines.iter().filter(|l| !l.last).collect();
    assert!(!justified.is_empty());
    for line in justified {
        assert_eq!(
            line.glue_ratio, 0.0,
            "word spacing must stay natural while kashida has capacity"
        );
        assert!(line.kashida_ratio > 0.0 && line.kashida_ratio <= 1.0);
        let placed = justify(&items, line);
        let spaces: f32 = placed
            .iter()
            .filter(|p| matches!(p, Placed::Space { .. }))
            .map(Placed::width)
            .sum();
        let kashida: f32 = placed
            .iter()
            .filter(|p| matches!(p, Placed::Kashida { .. }))
            .map(Placed::width)
            .sum();
        let natural_spaces = placed
            .iter()
            .filter(|p| matches!(p, Placed::Space { .. }))
            .count() as f32
            * 5.0;
        assert!(
            (spaces - natural_spaces).abs() < 1e-4,
            "spaces were stretched"
        );
        let total: f32 = placed.iter().map(Placed::width).sum();
        assert!((total - 200.0).abs() < 1e-3, "line not justified: {total}");
        assert!(kashida > 0.0);
    }
}

#[test]
fn glue_stretches_only_after_kashida_is_exhausted() {
    // Width 235: 3 words + 2 spaces = 190 px, slack 45 px; but only one kashida
    // opportunity per line (priority 1 on the first word only) → 16 px capacity.
    let mut run = Run::default();
    for w in 0..12 {
        let k: &[(usize, u8)] = if w % 3 == 0 { &[(2, 1)] } else { &[] };
        run.word(6, 1, k, &[]);
    }
    let items = build_items(&[run.view()], &ItemParams::default());
    let para = break_paragraph(&items, &|_| 235.0, &BreakParams::default());
    let line = para
        .lines
        .iter()
        .find(|l| !l.last && l.glue_ratio > 0.0)
        .expect("a glue-stretched line");
    assert_eq!(
        line.kashida_ratio, 1.0,
        "kashida must be fully used before glue stretches"
    );
}

#[test]
fn at_most_one_kashida_per_word_highest_priority_wins() {
    let mut run = Run::default();
    run.word(8, 1, &[(1, 3), (5, 1), (6, 2)], &[]);
    let items = build_items(&[run.view()], &ItemParams::default());
    let kashidas: Vec<_> = items
        .iter()
        .filter(|i| i.kind == ItemKind::Kashida)
        .collect();
    assert_eq!(kashidas.len(), 1);
    assert_eq!(
        kashidas[0].glyph,
        GlyphRef { run: 0, index: 5 },
        "priority 1 (glyph 5) must win"
    );
}

#[test]
fn rtl_lines_run_right_to_left_and_last_line_aligns_right() {
    let run = arabic(7, &[(2, 1)]);
    let p = layout_paragraph(
        &[run.view()],
        200.0,
        1,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    let first = &p.lines[0];
    let x_of = |idx: u32| {
        first
            .glyphs
            .iter()
            .find(|g| g.kind == GlyphKind::Glyph && g.index == idx)
            .map(|g| g.x)
            .unwrap()
    };
    // Logically first glyph sits to the right of the logically later ones.
    assert!(
        x_of(0) > x_of(5),
        "RTL: glyph 0 at {} should be right of glyph 5 at {}",
        x_of(0),
        x_of(5)
    );
    let last = p.lines.last().unwrap();
    let max_x = last.glyphs.iter().map(|g| g.x + 10.0).fold(0.0, f32::max);
    assert!(
        (max_x - 200.0).abs() < 1e-3,
        "RTL last line must end at the right edge, ends at {max_x}"
    );
}

#[test]
fn kashida_renders_as_scaled_tatweel() {
    let run = arabic(12, &[(2, 1)]);
    let p = layout_paragraph(
        &[run.view()],
        200.0,
        1,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    let k = p.lines[0]
        .glyphs
        .iter()
        .find(|g| g.kind == GlyphKind::Kashida)
        .expect("kashida glyph");
    assert_eq!(k.gid, TATWEEL.0);
    assert!(k.scale_x > 0.0);
}

#[test]
fn hit_test_returns_the_cluster_under_the_point() {
    let run = latin(&[3, 4, 5]);
    let p = layout_paragraph(
        &[run.view()],
        500.0,
        0,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    // Clusters: word0 = 0..3, space = 3, word1 = 4..8 … ; glyph 1 of word 1 starts at 30 + 5 + 10.
    assert_eq!(p.hit_test(0, 46.0), Some(5));
    assert_eq!(p.hit_test(0, 1.0), Some(0));
    assert_eq!(p.hit_test(9, 1.0), None);
}

#[test]
fn overlong_word_degrades_to_emergency_not_failure() {
    let run = latin(&[4, 60, 4]); // 600 px word, measure 200 px
    let p = layout_paragraph(
        &[run.view()],
        200.0,
        0,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    assert_eq!(p.quality, Quality::Emergency);
    assert!(!p.lines.is_empty());
}

#[test]
fn hyphenation_inserts_a_hyphen_glyph() {
    let mut run = Run::default();
    for _ in 0..8 {
        run.word(14, 0, &[], &[3, 6, 9]);
    }
    let p = layout_paragraph(
        &[run.view()],
        230.0,
        0,
        &ItemParams::default(),
        &BreakParams::default(),
    );
    let hyphens: Vec<_> = p
        .lines
        .iter()
        .flat_map(|l| l.glyphs.iter())
        .filter(|g| g.kind == GlyphKind::Hyphen)
        .collect();
    assert!(!hyphens.is_empty(), "expected at least one hyphenated line");
    assert!(hyphens.iter().all(|g| g.gid == HYPHEN.0));
}

#[test]
fn no_break_space_is_never_a_line_break() {
    let mut run = latin(&lengths(3, 30));
    // Turn every space into a no-break space except one in the middle.
    let spaces: Vec<usize> = run
        .flags
        .iter()
        .enumerate()
        .filter(|(_, f)| *f & F::GLUE != 0)
        .map(|(i, _)| i)
        .collect();
    let keep = spaces[spaces.len() / 2];
    for &i in &spaces {
        if i != keep {
            run.flags[i] &= !F::BREAK_OK;
        }
    }
    let items = build_items(&[run.view()], &ItemParams::default());
    let para = break_paragraph(&items, &|_| 400.0, &BreakParams::default());
    for line in para.lines.iter().filter(|l| !l.last) {
        let it = items[line.end];
        assert_eq!(it.glyph.index as usize, keep, "broke at a no-break space");
    }
}
