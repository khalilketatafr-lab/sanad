//! Roadmap §10.4: a 10-paragraph Latin and a 10-paragraph Arabic fixture
//! through the Compositor, natively.
//!
//!   cargo run --release -p sanad-atelier --example reflow_bench [-- [<out-dir>] [--rounds N]]
//!
//! A *reflow* lays out all 20 paragraphs at one measure: what the reader does
//! when the font size, spacing or orientation changes. It is timed over
//! several phone and tablet measures (`--rounds 0` skips timing). The fixture
//! is about 205 lines at a phone measure, roughly eight pages. G0's budget is
//! ≤ 16 ms for the visible spread (two pages) on tier B.
//!
//! With `<out-dir>`, writes `reflow.json`: the shaped, permuted paragraphs as
//! the WASM Compositor consumes them, plus a digest of the native layout at
//! every measure. `packages/lumen/test/compositor-wasm.test.ts` checks that
//! WASM produces bit-identical lines, and `packages/lumen/bench/reflow.ts`
//! times the WASM build in Chromium with CPU throttling.

use std::str::FromStr;
use std::time::Instant;
use std::{env, fs, path::PathBuf, process::ExitCode};

use rustybuzz::{Face, Language};
use sanad_atelier::page::used_glyphs;
use sanad_atelier::permute::{GlyphKey, Permutation};
use sanad_atelier::shape::{FontFace, ShapedParagraph, Typesetter};
use sanad_compositor::item::{ItemParams, RunView};
use sanad_compositor::layout::{GlyphKind, ParagraphLayout, layout_paragraph};
use sanad_compositor::linebreak::BreakParams;

const LATIN_FONT: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC_FONT: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");
const LATIN: &str = include_str!("../../../fixtures/typeset/latin.txt");
const ARABIC: &str = include_str!("../../../fixtures/typeset/arabic.txt");

const DPR: f32 = 2.0;
/// Text measures in CSS px: phones (portrait), small tablet, tablet.
const MEASURES: [f32; 5] = [288.0, 320.0, 360.0, 408.0, 560.0];
const DEFAULT_ROUNDS: usize = 200;

type Error = Box<dyn std::error::Error>;

/// One paragraph as the WASM spike entry point takes it: a single run.
struct Para {
    base_level: u8,
    scale: f32,
    gids: Vec<u16>,
    advances: Vec<i16>,
    offsets: Vec<[i16; 2]>,
    flags: Vec<u8>,
    levels: Vec<u8>,
    kashida_priority: Vec<u8>,
    kashida_max: Vec<u16>,
    hyphen: Option<(u16, i16)>,
    tatweel: Option<(u16, i16)>,
}

impl Para {
    fn view(&self) -> RunView<'_> {
        RunView {
            scale: self.scale,
            gids: &self.gids,
            advances: &self.advances,
            offsets: &self.offsets,
            flags: &self.flags,
            bidi_levels: &self.levels,
            kashida_priority: &self.kashida_priority,
            kashida_max: &self.kashida_max,
            hyphen: self.hyphen,
            tatweel: self.tatweel,
        }
    }

    fn layout(&self, width: f32) -> ParagraphLayout {
        layout_paragraph(
            &[self.view()],
            width,
            self.base_level,
            &ItemParams::default(),
            &BreakParams::default(),
        )
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "baseLevel": self.base_level,
            "scale": self.scale,
            "gids": self.gids,
            "advances": self.advances,
            "offsets": self.offsets.iter().flatten().collect::<Vec<_>>(),
            "flags": self.flags,
            "levels": self.levels,
            "kashidaPriority": self.kashida_priority,
            "kashidaMax": self.kashida_max,
            "hyphen": self.hyphen.map(|(g, a)| [i32::from(g), i32::from(a)]),
            "tatweel": self.tatweel.map(|(g, a)| [i32::from(g), i32::from(a)]),
        })
    }
}

fn typesetter() -> Result<Typesetter<'static>, Error> {
    let face = |data: &'static [u8]| Face::from_slice(data, 0).ok_or("unreadable font");
    Ok(Typesetter::new(
        FontFace {
            face: face(LATIN_FONT)?,
            size_px: 18.0 * DPR,
            script: rustybuzz::script::LATIN,
            language: Language::from_str("en")?,
        },
        FontFace {
            face: face(ARABIC_FONT)?,
            size_px: 21.0 * DPR,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar")?,
        },
    )?)
}

/// The fixture: the 10 longest Latin paragraphs of chapter I (dialogue lines
/// are too short to exercise line breaking) and the 10 Arabic paragraphs.
fn fixture(ts: &Typesetter<'_>) -> Result<Vec<ShapedParagraph>, Error> {
    let mut latin: Vec<&str> = LATIN
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    latin.sort_by_key(|p| std::cmp::Reverse(p.len()));
    latin.truncate(10);
    let arabic = ARABIC
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .take(10);
    let mut out = Vec::with_capacity(20);
    for p in latin.into_iter().chain(arabic) {
        out.push(ts.shape_paragraph(p)?);
    }
    Ok(out)
}

fn single_run(ts: &Typesetter<'_>, p: &ShapedParagraph, perm: &Permutation) -> Result<Para, Error> {
    let [r] = p.runs.as_slice() else {
        return Err(format!(
            "fixture paragraph has {} runs; the spike entry point takes one",
            p.runs.len()
        )
        .into());
    };
    let id = |gid: u16| {
        perm.get(GlyphKey { font: r.font, gid })
            .ok_or_else(|| format!("glyph {gid} not permuted"))
    };
    let sp = ts.specials(r.font);
    let special = |s: Option<(u16, i16)>| -> Result<Option<(u16, i16)>, String> {
        s.map(|(g, a)| id(g).map(|p| (p, a))).transpose()
    };
    Ok(Para {
        base_level: p.base_level,
        scale: r.scale,
        gids: r.gids.iter().map(|&g| id(g)).collect::<Result<_, _>>()?,
        advances: r.advances.clone(),
        offsets: r.offsets.clone(),
        flags: r.flags.clone(),
        levels: r.levels.clone(),
        kashida_priority: r.kashida_priority.clone(),
        kashida_max: r.kashida_max.clone(),
        hyphen: special(sp.hyphen)?,
        tatweel: special(sp.tatweel)?,
    })
}

/// FNV-1a over the bits of every `[gid, x, y, scale_x, kind]` the layout
/// yields, line by line: the same values `Compositor.line_glyphs` returns.
fn digest(layouts: &[ParagraphLayout]) -> (u32, usize) {
    let mut h: u32 = 0x811C_9DC5;
    let mut lines = 0;
    for l in layouts {
        for line in &l.lines {
            lines += 1;
            for g in &line.glyphs {
                let kind = match g.kind {
                    GlyphKind::Glyph => 0.0f32,
                    GlyphKind::Kashida => 1.0,
                    GlyphKind::Hyphen => 2.0,
                };
                for v in [f32::from(g.gid), g.x, g.y, g.scale_x, kind] {
                    for b in v.to_bits().to_le_bytes() {
                        h ^= u32::from(b);
                        h = h.wrapping_mul(0x0100_0193);
                    }
                }
            }
        }
    }
    (h, lines)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn run() -> Result<(), Error> {
    let args: Vec<String> = env::args().skip(1).collect();
    let rounds = match args.iter().position(|a| a == "--rounds") {
        Some(i) => args.get(i + 1).ok_or("--rounds needs a value")?.parse()?,
        None => DEFAULT_ROUNDS,
    };
    let out = args
        .iter()
        .enumerate()
        .find(|&(i, a)| !a.starts_with("--") && (i == 0 || args[i - 1] != "--rounds"))
        .map(|(_, a)| PathBuf::from(a));
    let ts = typesetter()?;
    let shaped = fixture(&ts)?;
    let perm = Permutation::new(used_glyphs(&ts, &shaped), [0x5A; 32])?;
    let paras: Vec<Para> = shaped
        .iter()
        .map(|p| single_run(&ts, p, &perm))
        .collect::<Result<_, _>>()?;
    let glyphs: usize = paras.iter().map(|p| p.gids.len()).sum();

    // Reference digests per measure (also warms caches).
    let mut reference = Vec::new();
    for css in MEASURES {
        let layouts: Vec<ParagraphLayout> = paras.iter().map(|p| p.layout(css * DPR)).collect();
        let (hash, lines) = digest(&layouts);
        reference.push(serde_json::json!({ "width": css * DPR, "lines": lines, "digest": hash }));
    }

    let mut samples = Vec::with_capacity(rounds * MEASURES.len());
    for round in 0..rounds {
        for css in MEASURES {
            let t = Instant::now();
            let mut lines = 0usize;
            for p in &paras {
                lines += p.layout(css * DPR).lines.len();
            }
            samples.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box((lines, round));
        }
    }
    samples.sort_by(f64::total_cmp);
    if !samples.is_empty() {
        let lines: u64 = reference.iter().filter_map(|r| r["lines"].as_u64()).sum();
        let per_line = percentile(&samples, 0.5) * MEASURES.len() as f64 / lines as f64;
        println!(
            "native reflow of 20 paragraphs ({glyphs} glyphs): p50 {:.3} ms · p95 {:.3} ms · max {:.3} ms ({} runs)",
            percentile(&samples, 0.5),
            percentile(&samples, 0.95),
            samples.last().copied().unwrap_or(0.0),
            samples.len()
        );
        println!(
            "  per line (p50): {:.1} µs → a 50-line spread ≈ {:.2} ms",
            per_line * 1e3,
            per_line * 50.0
        );
    }
    for r in &reference {
        println!("  width {:>4} px: {} lines", r["width"], r["lines"]);
    }

    if let Some(out) = out {
        fs::create_dir_all(&out)?;
        let json = serde_json::json!({
            "paragraphs": paras.iter().map(Para::to_json).collect::<Vec<_>>(),
            "reference": reference,
        });
        fs::write(out.join("reflow.json"), serde_json::to_vec(&json)?)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("reflow_bench: {e}");
            ExitCode::FAILURE
        }
    }
}
