//! Ingest stages ⑥–⑦ (blueprint 01 §5): shaped, permuted content and its
//! atlas become **sealed Folio chunks** laid out for object storage, with an
//! Ed25519-signed manifest.
//!
//! ```text
//! TypesetChapter ─ flow_payload ─▶ FlatBuffers FlowChunk ─ zstd-19 ─▶ plaintext
//!                                                                       │
//!      derive_chunk_key(TMK, edition, variant, index) ─▶ CK ─ seal_chunk (AES-256-GCM)
//!                                                                       ▼
//!   e/{H(edition)}/v{n}/c/{opaque}.folio   a/{page}.atl   manifest.json(.sig)
//! ```
//!
//! Two variants per chunk (A, B): variant B perturbs inter-word glue advances
//! by [`GLUE_PERTURB`] (~0.6%), so a leaked rendering can be traced to the
//! lease that served it (Ex Libris, blueprint 05). Payloads carry only
//! permuted glyph ids — never Unicode (P1).

use std::collections::BTreeMap;
use std::path::Path;

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use flatbuffers::FlatBufferBuilder;
use sanad_compositor::item::flags as gflags;
use sanad_folio::codec::{ChunkFlags, ChunkIdentity, ChunkKind};
use sanad_folio::schema::{
    Block, BlockArgs, BlockKind as FbBlockKind, Body, Direction, FlowChunk, FlowChunkArgs,
    GlyphOffset, Payload, PayloadArgs, Run, RunArgs, finish_payload_buffer,
};
use sanad_folio::seal::{ChunkKey, SealError, TitleMasterKey, derive_chunk_key, seal_chunk};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::book_atlas::BookAtlas;
use crate::epub::BlockKind;
use crate::ingest::{TypesetBlock, TypesetChapter};
use crate::permute::{GlyphKey, Permutation};
use crate::shape::{ShapedRun, Typesetter};

/// Variant B scales inter-word glue advances by this factor (~0.6%).
pub const GLUE_PERTURB: f32 = 1.006;
/// zstd level for chunk payloads (blueprint: level 19).
pub const ZSTD_LEVEL: i32 = 19;
/// Variants sealed per chunk: A (0) and B (1).
pub const VARIANTS: u8 = 2;
pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("shape error: {0}")]
    Shape(#[from] crate::shape::ShapeError),
    #[error("glyph {gid} of font {font} is not in the edition permutation")]
    Unpermuted { font: u8, gid: u16 },
    #[error("zstd: {0}")]
    Zstd(String),
    #[error("seal: {0}")]
    Seal(#[from] SealError),
    #[error("signing key rejected")]
    Key,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("serialize manifest: {0}")]
    Json(#[from] serde_json::Error),
}

fn fb_kind(kind: &BlockKind) -> (FbBlockKind, u8) {
    match kind {
        BlockKind::Heading { level } => (FbBlockKind::Heading, *level),
        BlockKind::ListItem { .. } => (FbBlockKind::ListItem, 0),
        BlockKind::Blockquote | BlockKind::Note | BlockKind::Caption => (FbBlockKind::Quote, 0),
        _ => (FbBlockKind::Paragraph, 0),
    }
}

fn permuted(perm: &Permutation, run: &ShapedRun) -> Result<Vec<u16>, PublishError> {
    run.gids
        .iter()
        .map(|&gid| {
            perm.get(GlyphKey {
                font: run.font,
                gid,
            })
            .ok_or(PublishError::Unpermuted {
                font: run.font.0,
                gid,
            })
        })
        .collect()
}

/// Variant B advances: glue glyphs scaled by [`GLUE_PERTURB`], others left
/// exact (boxes never stretch, blueprint D1).
fn advances_for(run: &ShapedRun, variant_b: bool) -> Vec<i16> {
    if !variant_b {
        return run.advances.clone();
    }
    run.advances
        .iter()
        .zip(&run.flags)
        .map(|(&adv, &flag)| {
            if flag & gflags::GLUE != 0 {
                (f32::from(adv) * GLUE_PERTURB).round() as i16
            } else {
                adv
            }
        })
        .collect()
}

/// Builds the zstd-compressed FlatBuffers `FlowChunk` payload for one chapter.
pub fn flow_payload(
    chapter: &TypesetChapter,
    perm: &Permutation,
    variant_b: bool,
) -> Result<Vec<u8>, PublishError> {
    let mut fbb = FlatBufferBuilder::new();
    let mut block_offsets = Vec::with_capacity(chapter.blocks.len());
    for TypesetBlock { role, shaped, .. } in &chapter.blocks {
        let mut run_offsets = Vec::with_capacity(shaped.runs.len());
        for run in &shaped.runs {
            let gids = permuted(perm, run)?;
            let offsets: Vec<GlyphOffset> = run
                .offsets
                .iter()
                .map(|&[x, y]| GlyphOffset::new(x, y))
                .collect();
            let gids = fbb.create_vector(&gids);
            let advances = fbb.create_vector(&advances_for(run, variant_b));
            let offsets = fbb.create_vector(&offsets);
            let fl = fbb.create_vector(&run.flags);
            let levels = fbb.create_vector(&run.levels);
            let kprio = fbb.create_vector(&run.kashida_priority);
            let kmax = fbb.create_vector(&run.kashida_max);
            run_offsets.push(Run::create(
                &mut fbb,
                &RunArgs {
                    font: run.font.0,
                    style: 0,
                    gids: Some(gids),
                    advances: Some(advances),
                    offsets: Some(offsets),
                    flags: Some(fl),
                    bidi_levels: Some(levels),
                    kashida_priority: Some(kprio),
                    kashida_max: Some(kmax),
                },
            ));
        }
        let runs = fbb.create_vector(&run_offsets);
        let (kind, level) = fb_kind(role);
        let dir = if shaped.base_level % 2 == 1 {
            Direction::Rtl
        } else {
            Direction::Ltr
        };
        block_offsets.push(Block::create(
            &mut fbb,
            &BlockArgs {
                kind,
                level,
                style: 0,
                dir,
                runs: Some(runs),
                figure: None,
                keep_with_next: matches!(role, BlockKind::Heading { .. }),
            },
        ));
    }
    let blocks = fbb.create_vector(&block_offsets);
    let flow = FlowChunk::create(
        &mut fbb,
        &FlowChunkArgs {
            blocks: Some(blocks),
        },
    );
    let payload = Payload::create(
        &mut fbb,
        &PayloadArgs {
            body_type: Body::FlowChunk,
            body: Some(flow.as_union_value()),
        },
    );
    finish_payload_buffer(&mut fbb, payload);
    zstd::bulk::compress(fbb.finished_data(), ZSTD_LEVEL)
        .map_err(|e| PublishError::Zstd(e.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(2 * bytes.len()), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(digest(&SHA256, bytes).as_ref())
}

/// Opaque, deterministic object name for one sealed chunk variant.
fn opaque_name(edition: &[u8; 16], index: u32, variant: u8) -> String {
    let mut m = Vec::with_capacity(21);
    m.extend_from_slice(edition);
    m.extend_from_slice(&index.to_be_bytes());
    m.push(variant);
    format!("{}.folio", &sha256_hex(digest(&SHA256, &m).as_ref())[..32])
}

// ── Manifest (signed) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ChunkEntry {
    pub chapter: usize,
    pub path: String,
    pub chunk_index: u32,
    pub variant: u8,
    pub flags: u16,
    /// SHA-256 of the compressed plaintext payload (integrity before sealing).
    pub plaintext_sha256: String,
    pub sealed_len: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotEntry {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlyphEntry {
    pub id: u16,
    pub origin: [f32; 2],
    pub texels_per_unit: f32,
    pub slots: Vec<SlotEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AtlasPageEntry {
    pub page: usize,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub coverage: f64,
    pub glyphs: Vec<GlyphEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub schema: u32,
    pub edition_id: String,
    pub version: u32,
    pub kind: &'static str,
    pub variants: u8,
    pub fonts: Vec<&'static str>,
    pub px_range: f32,
    pub chunks: Vec<ChunkEntry>,
    pub atlas: Vec<AtlasPageEntry>,
}

/// A sealed edition: every object keyed by its storage-relative path, plus
/// the parsed manifest. [`write_dir`](SealedEdition::write_dir) materializes it.
#[derive(Debug)]
pub struct SealedEdition {
    pub prefix: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub manifest: Manifest,
    pub public_key: Vec<u8>,
}

impl SealedEdition {
    /// Writes every object under `root`, creating directories.
    pub fn write_dir(&self, root: &Path) -> Result<(), PublishError> {
        for (path, bytes) in &self.files {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(full, bytes)?;
        }
        Ok(())
    }

    /// The manifest's storage path.
    #[must_use]
    pub fn manifest_path(&self) -> String {
        format!("{}/manifest.json", self.prefix)
    }
}

/// Seals a whole edition: one Flow chunk per chapter, two variants each,
/// plus the atlas pages, under the object-storage prefix
/// `e/{H(edition)}/v{version}`. The manifest is Ed25519-signed with
/// `signing_seed`; the public key is stored alongside.
#[allow(clippy::too_many_arguments)]
pub fn seal_edition(
    _ts: &Typesetter<'_>,
    book: &[TypesetChapter],
    perm: &Permutation,
    atlas: &BookAtlas,
    tmk: &TitleMasterKey,
    edition_id: Uuid,
    version: u32,
    signing_seed: &[u8; 32],
) -> Result<SealedEdition, PublishError> {
    let id16 = *edition_id.as_bytes();
    let edition_hash = &sha256_hex(&id16)[..32];
    let prefix = format!("e/{edition_hash}/v{version}");
    let mut files = BTreeMap::new();
    let mut chunks = Vec::new();

    for (chapter_index, chapter) in book.iter().enumerate() {
        let index = u32::try_from(chapter_index).unwrap_or(u32::MAX);
        for variant in 0..VARIANTS {
            let plaintext = flow_payload(chapter, perm, variant == 1)?;
            let id = ChunkIdentity {
                kind: ChunkKind::Flow,
                edition_id: id16,
                chunk_index: index,
                variant,
            };
            let key: ChunkKey = derive_chunk_key(tmk, &id16, variant, index)?;
            let flags = ChunkFlags::ZSTD.union(ChunkFlags::LAST_IN_CHAPTER);
            let sealed = seal_chunk(&key, &id, flags, &plaintext)?;
            let name = opaque_name(&id16, index, variant);
            let path = format!("{prefix}/c/{name}");
            chunks.push(ChunkEntry {
                chapter: chapter_index,
                path: path.clone(),
                chunk_index: index,
                variant,
                flags: flags.bits(),
                plaintext_sha256: sha256_hex(&plaintext),
                sealed_len: sealed.len(),
            });
            files.insert(path, sealed);
        }
    }

    // Atlas pages: raw RGBA bytes; geometry and glyph slots in the manifest.
    let mut atlas_entries = Vec::with_capacity(atlas.pages.len());
    for (page_index, page) in atlas.pages.iter().enumerate() {
        let path = format!("{prefix}/a/{page_index}.atl");
        let glyphs = page
            .glyphs
            .iter()
            .map(|(&id, g)| GlyphEntry {
                id,
                origin: [g.origin.0, g.origin.1],
                texels_per_unit: g.texels_per_unit,
                slots: g
                    .slots
                    .iter()
                    .map(|s| SlotEntry {
                        x: s.x,
                        y: s.y,
                        w: s.w,
                        h: s.h,
                    })
                    .collect(),
            })
            .collect();
        atlas_entries.push(AtlasPageEntry {
            page: page_index,
            path: path.clone(),
            width: page.width,
            height: page.height,
            coverage: atlas.coverage.get(page_index).copied().unwrap_or(1.0),
            glyphs,
        });
        files.insert(path, page.rgba.clone());
    }

    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        edition_id: edition_id.to_string(),
        version,
        kind: "flow",
        variants: VARIANTS,
        fonts: vec!["latin", "arabic"],
        px_range: atlas.pages.first().map_or(6.0, |p| p.px_range),
        chunks,
        atlas: atlas_entries,
    };

    // Canonical manifest bytes, Ed25519-signed.
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    let kp = Ed25519KeyPair::from_seed_unchecked(signing_seed).map_err(|_| PublishError::Key)?;
    let signature = kp.sign(&manifest_json);
    let public_key = kp.public_key().as_ref().to_vec();
    files.insert(format!("{prefix}/manifest.json"), manifest_json);
    files.insert(
        format!("{prefix}/manifest.sig"),
        signature.as_ref().to_vec(),
    );
    files.insert(format!("{prefix}/manifest.pub"), public_key.clone());

    Ok(SealedEdition {
        prefix,
        files,
        manifest,
        public_key,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::str::FromStr;

    use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
    use rustybuzz::{Face, Language};
    use sanad_folio::codec::ChunkKind;
    use sanad_folio::decompress;
    use sanad_folio::schema::{Body, root_as_payload};
    use sanad_folio::seal::open_chunk;

    use super::*;
    use crate::epub::{self, Limits};
    use crate::ingest::{edition_permutation, typeset_book};
    use crate::shape::FontFace;

    const SAMPLE: &[u8] = include_bytes!("../../../fixtures/epub/sanad-sample.epub");
    const LATIN: &[u8] = include_bytes!("../../../fixtures/fonts/literata/Literata-VF.ttf");
    const ARABIC: &[u8] =
        include_bytes!("../../../fixtures/fonts/noto-naskh-arabic/NotoNaskhArabic-VF.ttf");

    fn fixtures() -> (
        Typesetter<'static>,
        Vec<TypesetChapter>,
        Permutation,
        BookAtlas,
    ) {
        let ts = Typesetter::new(
            FontFace {
                face: Face::from_slice(LATIN, 0).unwrap(),
                size_px: 36.0,
                script: rustybuzz::script::LATIN,
                language: Language::from_str("en").unwrap(),
            },
            FontFace {
                face: Face::from_slice(ARABIC, 0).unwrap(),
                size_px: 42.0,
                script: rustybuzz::script::ARABIC,
                language: Language::from_str("ar").unwrap(),
            },
        )
        .unwrap();
        let book = epub::intake(SAMPLE, &Limits::default()).unwrap();
        let typeset = typeset_book(&ts, &book.chapters).unwrap();
        let perm = edition_permutation(&ts, &typeset, [0x5A; 32]).unwrap();
        let atlas = crate::book_atlas::build_book_atlas(
            &ts,
            &typeset,
            &perm,
            &crate::msdf::FieldParams::default(),
            &crate::atlas::AtlasParams::default(),
            crate::book_atlas::PAGE0_COVERAGE,
            [0x5A; 32],
        )
        .unwrap();
        (ts, typeset, perm, atlas)
    }

    const TMK: [u8; 32] = [0x42; 32];
    const EDITION: &str = "00000000-0000-7000-8000-00000000ca7a";

    #[test]
    fn sealed_chunk_opens_and_is_a_valid_flow_payload() {
        let (ts, book, perm, atlas) = fixtures();
        let tmk = TitleMasterKey::from_bytes(TMK);
        let edition = Uuid::parse_str(EDITION).unwrap();
        let sealed = seal_edition(&ts, &book, &perm, &atlas, &tmk, edition, 1, &[7; 32]).unwrap();

        // Open chunk 0, variant 0, exactly as a device with the lease would.
        let entry = sealed
            .manifest
            .chunks
            .iter()
            .find(|c| c.chunk_index == 0 && c.variant == 0)
            .unwrap();
        let bytes = &sealed.files[&entry.path];
        let id16 = *edition.as_bytes();
        let key = derive_chunk_key(&tmk, &id16, 0, 0).unwrap();
        let want = ChunkIdentity {
            kind: ChunkKind::Flow,
            edition_id: id16,
            chunk_index: 0,
            variant: 0,
        };
        let plaintext = open_chunk(&key, bytes, &want).unwrap();
        assert_eq!(sha256_hex(&plaintext), entry.plaintext_sha256);

        let fbuf = decompress(&plaintext, ChunkFlags::ZSTD).unwrap();
        let payload = root_as_payload(&fbuf).unwrap();
        assert_eq!(payload.body_type(), Body::FlowChunk);
        let flow = payload.body_as_flow_chunk().unwrap();
        // Chapter 1 has 12 text-bearing blocks; every block has ≥1 run with
        // permuted (non-Unicode) glyph ids.
        assert_eq!(flow.blocks().len(), 12);
        let first = flow.blocks().get(0);
        assert!(!first.runs().unwrap().is_empty());
    }

    #[test]
    fn variants_differ_only_in_glue_and_the_wrong_key_fails() {
        let (ts, book, perm, atlas) = fixtures();
        let tmk = TitleMasterKey::from_bytes(TMK);
        let edition = Uuid::parse_str(EDITION).unwrap();
        let sealed = seal_edition(&ts, &book, &perm, &atlas, &tmk, edition, 1, &[7; 32]).unwrap();
        let id16 = *edition.as_bytes();

        let a = flow_payload(&book[0], &perm, false).unwrap();
        let b = flow_payload(&book[0], &perm, true).unwrap();
        assert_ne!(a, b, "variant B perturbs glue");

        // Variant B opens with its own key; variant A's key must reject it.
        let vb = sealed
            .manifest
            .chunks
            .iter()
            .find(|c| c.chunk_index == 0 && c.variant == 1)
            .unwrap();
        let bytes = &sealed.files[&vb.path];
        let want_b = ChunkIdentity {
            kind: ChunkKind::Flow,
            edition_id: id16,
            chunk_index: 0,
            variant: 1,
        };
        assert!(
            open_chunk(
                &derive_chunk_key(&tmk, &id16, 1, 0).unwrap(),
                bytes,
                &want_b
            )
            .is_ok()
        );
        assert!(
            open_chunk(
                &derive_chunk_key(&tmk, &id16, 0, 0).unwrap(),
                bytes,
                &want_b
            )
            .is_err()
        );
    }

    #[test]
    fn manifest_signature_verifies_and_layout_is_correct() {
        let (ts, book, perm, atlas) = fixtures();
        let tmk = TitleMasterKey::from_bytes(TMK);
        let edition = Uuid::parse_str(EDITION).unwrap();
        let sealed = seal_edition(&ts, &book, &perm, &atlas, &tmk, edition, 1, &[7; 32]).unwrap();

        let json = &sealed.files[&sealed.manifest_path()];
        let sig = &sealed.files[&format!("{}/manifest.sig", sealed.prefix)];
        assert!(
            UnparsedPublicKey::new(&ED25519, &sealed.public_key)
                .verify(json, sig)
                .is_ok(),
            "manifest signature verifies"
        );

        // Object-storage layout: e/{hash}/v1/{c,a,manifest}.
        assert!(sealed.prefix.starts_with("e/") && sealed.prefix.ends_with("/v1"));
        // 2 chapters × 2 variants = 4 chunks; ≥1 atlas page.
        assert_eq!(sealed.manifest.chunks.len(), 4);
        assert!(!sealed.manifest.atlas.is_empty());
        assert!(sealed.files.keys().all(|k| k.starts_with("e/")));
        // Every chunk and atlas file is present.
        for c in &sealed.manifest.chunks {
            assert!(sealed.files.contains_key(&c.path));
        }
        for a in &sealed.manifest.atlas {
            assert!(sealed.files.contains_key(&a.path));
        }
    }
}
