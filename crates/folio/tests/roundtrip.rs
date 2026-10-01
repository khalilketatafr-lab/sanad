//! End-to-end container tests: build payload → compress → seal → parse →
//! open → decompress → verify → read. Plus tamper and identity checks.
#![cfg(all(feature = "seal", feature = "compress"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use flatbuffers::FlatBufferBuilder;
use sanad_folio::codec::{ChunkFlags, ChunkIdentity, ChunkKind, HEADER_LEN, SealedChunk};
use sanad_folio::schema::{
    Block, BlockArgs, BlockKind, Body, Direction, FlowChunk, FlowChunkArgs, GlyphFlags, Payload,
    PayloadArgs, Run, RunArgs, finish_payload_buffer,
};
use sanad_folio::seal::{SealError, TitleMasterKey, derive_chunk_key, open_chunk, seal_chunk};
use sanad_folio::{PayloadError, decompress, verify_payload};

const EDITION: [u8; 16] = [
    0x01, 0x8f, 0x3c, 0x5a, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0xca, 0x7a,
];

fn flags(f: GlyphFlags) -> u8 {
    f.bits()
}

/// One RTL paragraph: 6 glyphs, a word break with glue, one kashida point.
fn build_flow_payload() -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let cs = flags(GlyphFlags::ClusterStart);
    let ws = flags(GlyphFlags::ClusterStart | GlyphFlags::WordStart);
    let gids = fbb.create_vector(&[812u16, 77, 3051, 9, 412, 1999]);
    let advances = fbb.create_vector(&[480i16, 512, 230, 260, 498, 505]);
    let glyph_flags = fbb.create_vector(&[
        ws | flags(GlyphFlags::SentenceStart),
        cs | flags(GlyphFlags::KashidaOk),
        cs,
        cs | flags(GlyphFlags::Glue | GlyphFlags::BreakOk),
        ws,
        cs,
    ]);
    let bidi = fbb.create_vector(&[1u8, 1, 1, 1, 1, 1]);
    let prio = fbb.create_vector(&[0u8, 2, 0, 0, 0, 0]);
    let kmax = fbb.create_vector(&[0u16, 900, 0, 0, 0, 0]);
    let run = Run::create(
        &mut fbb,
        &RunArgs {
            font: 0,
            style: 3,
            gids: Some(gids),
            advances: Some(advances),
            offsets: None,
            flags: Some(glyph_flags),
            bidi_levels: Some(bidi),
            kashida_priority: Some(prio),
            kashida_max: Some(kmax),
        },
    );
    let runs = fbb.create_vector(&[run]);
    let block = Block::create(
        &mut fbb,
        &BlockArgs {
            kind: BlockKind::Paragraph,
            level: 0,
            style: 3,
            dir: Direction::Rtl,
            runs: Some(runs),
            figure: None,
            keep_with_next: false,
        },
    );
    let blocks = fbb.create_vector(&[block]);
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
    fbb.finished_data().to_vec()
}

fn identity(index: u32, variant: u8) -> ChunkIdentity {
    ChunkIdentity {
        kind: ChunkKind::Flow,
        edition_id: EDITION,
        chunk_index: index,
        variant,
    }
}

fn sealed(tmk: &TitleMasterKey, id: &ChunkIdentity) -> Vec<u8> {
    let compressed = zstd::encode_all(build_flow_payload().as_slice(), 19).unwrap();
    let key = derive_chunk_key(tmk, &id.edition_id, id.variant, id.chunk_index).unwrap();
    seal_chunk(&key, id, ChunkFlags::ZSTD, &compressed).unwrap()
}

#[test]
fn full_round_trip_preserves_glyph_runs() {
    let tmk = TitleMasterKey::generate().unwrap();
    let id = identity(21, 1);
    let object = sealed(&tmk, &id);

    let chunk = SealedChunk::parse(&object).unwrap();
    assert!(
        chunk.header.flags.contains(ChunkFlags::HAS_VARIANT),
        "variant B must set HAS_VARIANT"
    );
    assert_eq!(chunk.sealed_body().len(), object.len() - HEADER_LEN);

    let key = derive_chunk_key(&tmk, &EDITION, 1, 21).unwrap();
    let plaintext = open_chunk(&key, &object, &id).unwrap();
    let decoded = decompress(&plaintext, chunk.header.flags).unwrap();
    let payload = verify_payload(&decoded, ChunkKind::Flow).unwrap();
    let flow = payload.body_as_flow_chunk().unwrap();
    let run = flow.blocks().get(0).runs().unwrap().get(0);
    assert_eq!(
        run.gids().iter().collect::<Vec<_>>(),
        [812, 77, 3051, 9, 412, 1999]
    );
    assert_eq!(flow.blocks().get(0).dir(), Direction::Rtl);
    assert_eq!(run.kashida_max().unwrap().get(1), 900);
}

#[test]
fn any_header_or_body_modification_fails_authentication() {
    let tmk = TitleMasterKey::generate().unwrap();
    let id = identity(3, 0);
    let object = sealed(&tmk, &id);
    let key = derive_chunk_key(&tmk, &EDITION, 0, 3).unwrap();
    // Flip one bit in each authenticated region: a reserved-free AAD byte (edition id),
    // the nonce, the ciphertext and the tag.
    for pos in [10usize, 33, HEADER_LEN + 2, object.len() - 1] {
        let mut t = object.clone();
        t[pos] ^= 0x01;
        let err = open_chunk(
            &key,
            &t,
            &if pos == 10 {
                ChunkIdentity {
                    edition_id: {
                        let mut e = EDITION;
                        e[2] ^= 1;
                        e
                    },
                    ..id
                }
            } else {
                id
            },
        );
        assert!(
            matches!(err, Err(SealError::Authentication)),
            "tamper at {pos} not detected: {err:?}"
        );
    }
}

#[test]
fn wrong_key_index_or_variant_cannot_open() {
    let tmk = TitleMasterKey::generate().unwrap();
    let id = identity(7, 0);
    let object = sealed(&tmk, &id);
    let other_index = derive_chunk_key(&tmk, &EDITION, 0, 8).unwrap();
    assert!(matches!(
        open_chunk(&other_index, &object, &id),
        Err(SealError::Authentication)
    ));
    let other_variant = derive_chunk_key(&tmk, &EDITION, 1, 7).unwrap();
    assert!(matches!(
        open_chunk(&other_variant, &object, &id),
        Err(SealError::Authentication)
    ));
    let other_title =
        derive_chunk_key(&TitleMasterKey::generate().unwrap(), &EDITION, 0, 7).unwrap();
    assert!(matches!(
        open_chunk(&other_title, &object, &id),
        Err(SealError::Authentication)
    ));
    // Identity mismatch is caught before any crypto.
    let key = derive_chunk_key(&tmk, &EDITION, 0, 7).unwrap();
    assert!(matches!(
        open_chunk(&key, &object, &identity(9, 0)),
        Err(SealError::Codec(_))
    ));
}

#[test]
fn payload_kind_must_match_header_kind() {
    let buf = build_flow_payload();
    assert!(matches!(
        verify_payload(&buf, ChunkKind::Page),
        Err(PayloadError::KindMismatch { .. })
    ));
    assert!(verify_payload(&buf[..buf.len() - 3], ChunkKind::Flow).is_err());
}

#[test]
fn decompression_is_bounded() {
    // 32 MiB of zeros compresses to a few KiB: a classic bomb.
    let bomb = zstd::encode_all(std::io::repeat(0).take_bytes(32 << 20).as_slice(), 3).unwrap();
    assert!(bomb.len() < 64 * 1024);
    assert!(matches!(
        decompress(&bomb, ChunkFlags::ZSTD),
        Err(PayloadError::TooLarge)
    ));
}

trait TakeBytes {
    fn take_bytes(self, n: usize) -> Vec<u8>;
}
impl TakeBytes for std::io::Repeat {
    fn take_bytes(self, n: usize) -> Vec<u8> {
        use std::io::Read;
        let mut v = Vec::with_capacity(n);
        self.take(n as u64).read_to_end(&mut v).unwrap();
        v
    }
}

/// Pins the derivation (salt/info layout) so the Kernel, Atelier and any
/// future implementation agree byte for byte.
#[test]
fn chunk_key_derivation_known_answer() {
    let tmk = TitleMasterKey::from_bytes([0x42; 32]);
    let key = derive_chunk_key(&tmk, &EDITION, 1, 21).unwrap();
    // Independently computed: HKDF-SHA256(ikm=42*32, salt=EDITION‖01, info="folio/chunk/v1"‖15000000, L=32).
    assert_eq!(hex::encode(key.expose_for_wrapping()), KNOWN_CHUNK_KEY);
}
const KNOWN_CHUNK_KEY: &str = include_str!("vectors/chunk_key_v1.hex").trim_ascii();

/// Sanity check of the HKDF call pattern against RFC 5869, Appendix A.1.
#[test]
fn hkdf_matches_rfc5869_case_1() {
    struct Len(usize);
    impl KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let ikm = [0x0b; 22];
    let salt: Vec<u8> = (0x00..=0x0c).collect();
    let info: Vec<u8> = (0xf0..=0xf9).collect();
    let mut okm = [0u8; 42];
    Salt::new(HKDF_SHA256, &salt)
        .extract(&ikm)
        .expand(&[&info], Len(42))
        .unwrap()
        .fill(&mut okm)
        .unwrap();
    assert_eq!(
        hex::encode(okm),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
}
