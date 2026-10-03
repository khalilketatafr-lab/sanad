//! Ingests an EPUB and prints its frequency-ordered atlas layout:
//!
//!   cargo run -q -p sanad-atelier --example atlas_plan -- [epub] [max_height]
//!
//! Defaults to the sample book. A small `max_height` (e.g. 256) forces the
//! long tail onto later pages so the paging is visible. Server-side only.

use std::str::FromStr;

use rustybuzz::{Face, Language};
use sanad_atelier::atlas::AtlasParams;
use sanad_atelier::book_atlas::{PAGE0_COVERAGE, build_book_atlas};
use sanad_atelier::epub::{self, Limits};
use sanad_atelier::ingest::{edition_permutation, typeset_book};
use sanad_atelier::msdf::FieldParams;
use sanad_atelier::shape::{FontFace, Typesetter};

const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");
const SAMPLE: &[u8] = include_bytes!("../../../fixtures/epub/sanad-sample.epub");

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let bytes = match args.next() {
        Some(path) if !path.is_empty() => std::fs::read(path)?,
        _ => SAMPLE.to_vec(),
    };
    let max_height: u32 = args.next().map_or(Ok(2048), |s| s.parse())?;

    let face = |d: &'static [u8]| Face::from_slice(d, 0).ok_or("unreadable font");
    let ts = Typesetter::new(
        FontFace {
            face: face(LATIN)?,
            size_px: 36.0,
            script: rustybuzz::script::LATIN,
            language: Language::from_str("en")?,
        },
        FontFace {
            face: face(ARABIC)?,
            size_px: 42.0,
            script: rustybuzz::script::ARABIC,
            language: Language::from_str("ar")?,
        },
    )?;

    let book = epub::intake(&bytes, &Limits::default())?;
    let typeset = typeset_book(&ts, &book.chapters)?;
    let perm = edition_permutation(&ts, &typeset, [0x5A; 32])?;
    let params = AtlasParams {
        max_height,
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
    )?;

    let plan = &atlas.plan;
    println!(
        "book: {} chapters, {} rendered glyphs",
        typeset.len(),
        plan.total
    );
    println!(
        "distinct inkful glyphs: {}   95% reached at glyph rank {} ({:.1}%)",
        plan.ordered.len(),
        plan.page0_len,
        plan.coverage(plan.page0_len) * 100.0
    );
    println!(
        "\natlas: {} page(s), width {}, max_height {}",
        atlas.pages.len(),
        params.width,
        max_height
    );
    println!(
        "  {:<5} {:<12} {:<8} {:<10} {:<10}",
        "page", "size", "glyphs", "fragments", "coverage"
    );
    println!(
        "  {:-<5} {:-<12} {:-<8} {:-<10} {:-<10}",
        "", "", "", "", ""
    );
    for (i, page) in atlas.pages.iter().enumerate() {
        let glyphs = atlas.page_of.values().filter(|&&p| p == i).count();
        let fragments: usize = page.glyphs.values().map(|g| g.slots.len()).sum();
        println!(
            "  {i:<5} {:<12} {glyphs:<8} {fragments:<10} {:<9.1}%",
            format!("{}×{}", page.width, page.height),
            atlas.coverage[i] * 100.0
        );
    }

    println!("\ntop glyphs by frequency (permuted id · occurrences · page):");
    for &(id, count) in plan.ordered.iter().take(10) {
        let page = atlas.page_of.get(&id).copied().unwrap_or(usize::MAX);
        println!("  π={id:<6} ×{count:<4} page {page}");
    }
    Ok(())
}
