//! Writes the reader's cross-language fixture: one sealed Folio chunk from the
//! sample book, the dev chunk key that opens it, and the FlowChunk it decodes
//! to — so `packages/lumen` can prove its TypeScript decode path recovers
//! exactly what Atelier sealed.
//!
//!   cargo run -q -p sanad-atelier --example seal_reader_fixture
//!
//! Dev key only (the canary edition's published test TMK). Server-side tool.

use std::path::Path;
use std::str::FromStr;

use rustybuzz::{Face, Language};
use sanad_atelier::epub::{self, Limits};
use sanad_atelier::atlas::AtlasParams;
use sanad_atelier::book_atlas::{PAGE0_COVERAGE, build_book_atlas};
use sanad_atelier::ingest::{edition_permutation, typeset_book};
use sanad_atelier::msdf::FieldParams;
use sanad_atelier::publish::flow_payload;
use sanad_atelier::shape::{FontFace, Typesetter};
use sanad_folio::codec::{ChunkFlags, ChunkIdentity, ChunkKind};
use sanad_folio::schema::{Body, root_as_payload};
use sanad_folio::seal::{TitleMasterKey, derive_chunk_key, seal_chunk};
use serde_json::{Value, json};
use uuid::Uuid;

const SAMPLE: &[u8] = include_bytes!("../../../fixtures/epub/sanad-sample.epub");
const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
const ARABIC: &[u8] =
    include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");
const EDITION: &str = "00000000-0000-7000-8000-00000000ca7a";
const TMK: [u8; 32] = [0x42; 32];

type Error = Box<dyn std::error::Error>;

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// The FlowChunk payload decoded back to JSON (what the TS reader must match).
fn decode(fbuf: &[u8]) -> Result<Value, Error> {
    let payload = root_as_payload(fbuf)?;
    assert_eq!(payload.body_type(), Body::FlowChunk);
    let flow = payload.body_as_flow_chunk().ok_or("not a flow chunk")?;
    let blocks: Vec<Value> = flow
        .blocks()
        .iter()
        .map(|b| {
            let runs: Vec<Value> = b
                .runs()
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            json!({
                                "font": r.font(),
                                "gids": r.gids().iter().collect::<Vec<u16>>(),
                                "advances": r.advances().iter().collect::<Vec<i16>>(),
                                "flags": r.flags().iter().collect::<Vec<u8>>(),
                                "bidiLevels": r.bidi_levels().iter().collect::<Vec<u8>>(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({ "kind": b.kind().0, "dir": b.dir().0, "runs": runs })
        })
        .collect();
    Ok(json!({ "blocks": blocks }))
}

fn main() -> Result<(), Error> {
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
    let edition = Uuid::parse_str(EDITION)?;
    let id16 = *edition.as_bytes();
    let book = epub::intake(SAMPLE, &Limits::default())?;
    let typeset = typeset_book(&ts, &book.chapters)?;
    let perm = edition_permutation(&ts, &typeset, [0x5A; 32])?;

    // Chapter 0, variant A: the plaintext payload, compressed and sealed.
    let plaintext = flow_payload(&typeset[0], &perm, false)?;
    let fbuf = zstd::bulk::decompress(&plaintext, 16 << 20)?; // uncompressed FlatBuffer
    let id = ChunkIdentity {
        kind: ChunkKind::Flow,
        edition_id: id16,
        chunk_index: 0,
        variant: 0,
    };
    let tmk = TitleMasterKey::from_bytes(TMK);
    let key = derive_chunk_key(&tmk, &id16, 0, 0)?;
    let sealed = seal_chunk(
        &key,
        &id,
        ChunkFlags::ZSTD.union(ChunkFlags::LAST_IN_CHAPTER),
        &plaintext,
    )?;

    let dir = Path::new("fixtures/folio");
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("canary-chunk0-v0.folio"), &sealed)?;
    let expected = json!({
        "note": "Dev fixture: canary edition sealed with the published test TMK (0x42). Not for production.",
        "editionId": EDITION,
        "chunkIndex": 0,
        "variant": 0,
        "kind": "flow",
        "chunkKeyHex": hex(key.expose_for_wrapping()),
        "payload": decode(&fbuf)?,
    });
    std::fs::write(
        dir.join("canary-chunk0-v0.expected.json"),
        serde_json::to_vec_pretty(&expected)?,
    )?;

    // Atlas page 0 (PNG) and glyph table, so the reader can render the chunk.
    let atlas = build_book_atlas(
        &ts,
        &typeset,
        &perm,
        &FieldParams::default(),
        &AtlasParams::default(),
        PAGE0_COVERAGE,
        [0x5A; 32],
    )?;
    let page0 = &atlas.pages[0];
    image::save_buffer(
        dir.join("canary-atlas0.png"),
        &page0.rgba,
        page0.width,
        page0.height,
        image::ExtendedColorType::Rgba8,
    )?;
    let glyphs: serde_json::Map<String, Value> = page0
        .glyphs
        .iter()
        .map(|(&gid, g)| {
            (
                gid.to_string(),
                json!({
                    "o": [g.origin.0, g.origin.1],
                    "t": g.texels_per_unit,
                    "s": g.slots.iter().map(|s| [s.x, s.y, s.w, s.h]).collect::<Vec<_>>(),
                }),
            )
        })
        .collect();
    std::fs::write(
        dir.join("canary-glyphs.json"),
        serde_json::to_vec(&json!({
            "pxRange": page0.px_range,
            "width": page0.width,
            "height": page0.height,
            "glyphs": glyphs,
        }))?,
    )?;
    println!(
        "wrote fixtures/folio/canary-chunk0-v0.folio ({} bytes), .expected.json, canary-atlas0.png ({}×{}), canary-glyphs.json ({} glyphs)",
        sealed.len(),
        page0.width,
        page0.height,
        page0.glyphs.len(),
    );
    Ok(())
}
