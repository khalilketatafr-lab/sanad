//! Writes a test atlas for Lumen's WebGL shredding test:
//! slot 0 = the whole demo glyph (comparison only; production atlases never
//! contain whole glyphs), slots 1..=3 = its shredded fragments.
//!
//!   cargo run -q -p sanad-atelier --example shred_fixture -- <out-dir>
//!
//! Emits `<out-dir>/atlas.rgba` (RGBA8) and `<out-dir>/atlas.json`.

use std::{env, fs, path::PathBuf, process::ExitCode};

use sanad_atelier::shred::{Rect, VoronoiCut, bake_fragment, demo_glyph, ink_share, min_overlap};

// Power-of-two slots on a power-of-two atlas: every slot samples at bit-identical
// fractional texel positions, so the test isolates shredding from texture-
// coordinate rounding (which varies with slot position on any atlas).
const SIZE: usize = 128;
const PX_RANGE: f32 = 6.0;
const FRAGMENTS: usize = 3;

fn main() -> ExitCode {
    let Some(out) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: shred_fixture <out-dir>");
        return ExitCode::FAILURE;
    };
    let glyph = demo_glyph(SIZE, PX_RANGE);
    let cut = match VoronoiCut::seeded(
        0x5A4E_AD00,
        Rect {
            x: 24.0,
            y: 24.0,
            w: 80.0,
            h: 80.0,
        },
        FRAGMENTS,
        32.0,
        min_overlap(PX_RANGE),
        PX_RANGE,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("shred_fixture: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut slots = vec![glyph.clone()];
    slots.extend((0..FRAGMENTS).map(|k| bake_fragment(&glyph, &cut, k)));

    // Pack slots side by side: atlas is (SIZE * slots) × SIZE.
    let width = SIZE * slots.len();
    let mut rgba = vec![0u8; width * SIZE * 4];
    for (s, img) in slots.iter().enumerate() {
        let px = img.to_rgba8();
        for y in 0..SIZE {
            let dst = (y * width + s * SIZE) * 4;
            rgba[dst..dst + SIZE * 4].copy_from_slice(&px[y * SIZE * 4..(y + 1) * SIZE * 4]);
        }
    }
    let shares: Vec<f32> = slots[1..].iter().map(|f| ink_share(&glyph, f)).collect();
    let meta = serde_json::json!({
        "width": width,
        "height": SIZE,
        "slotSize": SIZE,
        "pxRange": PX_RANGE,
        "slots": slots.len(),
        "fragmentInkShare": shares,
        "sites": cut.sites().iter().map(|p| [p.x, p.y]).collect::<Vec<_>>(),
    });
    if let Err(e) = fs::create_dir_all(&out)
        .and_then(|()| fs::write(out.join("atlas.rgba"), &rgba))
        .and_then(|()| fs::write(out.join("atlas.json"), meta.to_string()))
    {
        eprintln!("shred_fixture: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
