//! Typesets the Phase 0 golden pages through the real pipeline (shape →
//! permute → MSDF → shred → atlas → Compositor) and writes exactly what Lumen
//! needs to draw them:
//!
//!   cargo run -q -p sanad-atelier --example golden_pages -- <out-dir>
//!
//! Emits `<out-dir>/atlas.rgba` (RGBA8), `<out-dir>/<page>.bin` (Pass 1
//! instances, 40 bytes each) and `<out-dir>/pages.json`. The pages are
//! rendered and compared by `packages/lumen/test/golden.test.ts`.
//!
//! It also emits a *reference* set (`atlas-whole.rgba`, `<page>-whole.bin`)
//! with unshredded glyphs at identical slot sizes. The test requires the
//! shredded page to render pixel-identical to it: seams are invisible on real
//! text, not just on a test glyph. The reference never ships.
//!
//! Seeds are fixed so the output is reproducible. Production editions draw
//! them from the OS CSPRNG.

use std::str::FromStr;
use std::{env, fs, path::PathBuf, process::ExitCode};

use rustybuzz::{Face, Language};
use sanad_atelier::atlas::{Atlas, AtlasParams, build_atlas};
use sanad_atelier::msdf::{FieldParams, glyph_field};
use sanad_atelier::page::{PageSpec, compose_page, used_glyphs};
use sanad_atelier::permute::Permutation;
use sanad_atelier::shape::{FontFace, ShapedParagraph, Typesetter};

const LATIN_FONT: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC_FONT: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

const PAGES: [(&str, &str); 3] = [
    ("latin", include_str!("../../../fixtures/typeset/latin.txt")),
    (
        "arabic",
        include_str!("../../../fixtures/typeset/arabic.txt"),
    ),
    ("mixed", include_str!("../../../fixtures/typeset/mixed.txt")),
];

/// A 480 × 720 CSS-px page at device pixel ratio 2.
const DPR: f32 = 2.0;
const SPEC: PageSpec = PageSpec {
    width: 480.0 * DPR,
    height: 720.0 * DPR,
    margin_x: 36.0 * DPR,
    margin_top: 44.0 * DPR,
    margin_bottom: 44.0 * DPR,
    leading: 1.5,
    leading_arabic: 1.75,
    indent_em: 1.5,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("golden_pages: {e}");
            ExitCode::FAILURE
        }
    }
}

type Error = Box<dyn std::error::Error>;

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

/// The shipped (shredded) atlas and the unshredded test reference.
fn atlases(ts: &Typesetter<'_>, perm: &Permutation) -> Result<(Atlas, Atlas), Error> {
    let field_params = FieldParams::default();
    let fields: Vec<_> = perm
        .iter()
        .filter_map(|(id, key)| {
            glyph_field(
                &ts.font(key.font).face,
                key.gid,
                &field_params,
                u64::from(id),
            )
            .map(|f| (id, f))
        })
        .collect();
    let reference = AtlasParams {
        shred: false,
        ..AtlasParams::default()
    };
    let whole = build_atlas(fields.clone(), &reference, [0xA7; 32])?;
    let shredded = build_atlas(fields, &AtlasParams::default(), [0xA7; 32])?;
    Ok((shredded, whole))
}

fn shape_pages(ts: &Typesetter<'_>) -> Result<Vec<(&'static str, Vec<ShapedParagraph>)>, Error> {
    PAGES
        .iter()
        .map(|&(name, text)| {
            let paras = text
                .split("\n\n")
                .filter(|p| !p.trim().is_empty())
                .map(|p| ts.shape_paragraph(p.trim()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((name, paras))
        })
        .collect()
}

fn run() -> Result<(), Error> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: golden_pages <out-dir>")?;
    fs::create_dir_all(&out)?;
    let ts = typesetter()?;
    let shaped = shape_pages(&ts)?;
    let all: Vec<ShapedParagraph> = shaped.iter().flat_map(|(_, p)| p.clone()).collect();
    let perm = Permutation::new(used_glyphs(&ts, &all), [0x5A; 32])?;
    let (atlas, whole) = atlases(&ts, &perm)?;
    fs::write(out.join("atlas.rgba"), &atlas.rgba)?;
    fs::write(out.join("atlas-whole.rgba"), &whole.rgba)?;
    let shredded = atlas.glyphs.values().filter(|g| g.slots.len() > 1).count();
    eprintln!(
        "atlas: {} glyphs ({} shredded, {} atomic) → {}×{}",
        atlas.glyphs.len(),
        shredded,
        atlas.glyphs.len() - shredded,
        atlas.width,
        atlas.height
    );

    let mut pages = Vec::new();
    for (name, paras) in &shaped {
        let page = compose_page(&ts, paras, &perm, &atlas, &SPEC)?;
        let reference = compose_page(&ts, paras, &perm, &whole, &SPEC)?;
        fs::write(out.join(format!("{name}.bin")), page.instance_bytes())?;
        fs::write(
            out.join(format!("{name}-whole.bin")),
            reference.instance_bytes(),
        )?;
        let s = &page.stats;
        eprintln!(
            "{name}: {}/{} paragraphs, {} lines, {} glyphs → {} fragments, {} kashidas, {} hyphens, {} non-optimal paragraphs",
            s.paragraphs,
            paras.len(),
            s.lines,
            s.glyphs,
            s.fragments,
            s.kashidas,
            s.hyphens,
            s.relaxed_or_worse
        );
        pages.push(serde_json::json!({
            "name": name,
            "file": format!("{name}.bin"),
            "count": page.instances.len(),
            "wholeFile": format!("{name}-whole.bin"),
            "wholeCount": reference.instances.len(),
            "stats": {
                "paragraphs": s.paragraphs,
                "lines": s.lines,
                "glyphs": s.glyphs,
                "fragments": s.fragments,
                "kashidas": s.kashidas,
                "hyphens": s.hyphens,
            },
        }));
    }
    let meta = serde_json::json!({
        "width": SPEC.width,
        "height": SPEC.height,
        "atlas": {
            "file": "atlas.rgba",
            "width": atlas.width,
            "height": atlas.height,
            "pxRange": atlas.px_range,
        },
        "atlasWhole": {
            "file": "atlas-whole.rgba",
            "width": whole.width,
            "height": whole.height,
        },
        "pages": pages,
    });
    fs::write(out.join("pages.json"), serde_json::to_vec_pretty(&meta)?)?;
    Ok(())
}
