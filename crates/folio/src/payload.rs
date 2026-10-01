//! Payload decoding: zstd → verified FlatBuffer → structural validation.
//!
//! Runs after AEAD open. The input is therefore authenticated, but it is still
//! validated as if hostile: a compromised ingest worker or a bug must never
//! turn into an out-of-bounds read, a decompression bomb or a layout crash in
//! every reader's browser.

use std::borrow::Cow;
use std::io::Read;

use flatbuffers::{InvalidFlatbuffer, VerifierOptions};
use thiserror::Error;

use crate::codec::{ChunkFlags, ChunkKind};
use crate::schema::{
    Block, BlockKind, Body, FlowChunk, GlyphFlags, PageChunk, Payload, Run,
    payload_buffer_has_identifier, root_as_payload_with_opts,
};

/// Decompressed payload cap. Flow chunks are ~16–48 KiB compressed and
/// expand ~3–5×, so this leaves two orders of magnitude of headroom.
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
/// Root `uoffset` (4) + file identifier (4).
const MIN_PAYLOAD_LEN: usize = 8;
/// UAX #9: maximum explicit embedding depth is 125.
pub const MAX_BIDI_LEVEL: u8 = 125;
/// PDF user-space limit (ISO 32000: 14 400 units).
pub const MAX_PAGE_EXTENT: f32 = 14_400.0;

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("zstd: {0}")]
    Zstd(String),
    #[error("decompressed payload exceeds {MAX_PAYLOAD_LEN} bytes")]
    TooLarge,
    #[error("payload is not a Folio v1 buffer (missing FOL1 identifier)")]
    MissingIdentifier,
    #[error("flatbuffer verification failed: {0}")]
    Flatbuffer(#[from] InvalidFlatbuffer),
    #[error("chunk kind {header:?} cannot carry a {body} payload")]
    KindMismatch {
        header: ChunkKind,
        body: &'static str,
    },
    #[error("block {block}: {detail}")]
    Block { block: usize, detail: &'static str },
    #[error("block {block} run {run}: {detail}")]
    Run {
        block: usize,
        run: usize,
        detail: &'static str,
    },
    #[error("page {page}: {detail}")]
    Page { page: usize, detail: &'static str },
}

/// Removes the compression layer, bounded by [`MAX_PAYLOAD_LEN`].
pub fn decompress(plaintext: &[u8], flags: ChunkFlags) -> Result<Cow<'_, [u8]>, PayloadError> {
    if !flags.contains(ChunkFlags::ZSTD) {
        if plaintext.len() > MAX_PAYLOAD_LEN {
            return Err(PayloadError::TooLarge);
        }
        return Ok(Cow::Borrowed(plaintext));
    }
    let mut src = plaintext;
    let decoder = ruzstd::decoding::StreamingDecoder::new(&mut src)
        .map_err(|e| PayloadError::Zstd(e.to_string()))?;
    let mut out = Vec::with_capacity(plaintext.len().saturating_mul(4).min(MAX_PAYLOAD_LEN));
    // Read one byte past the cap so an over-long stream is detected, not truncated.
    decoder
        .take(MAX_PAYLOAD_LEN as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| PayloadError::Zstd(e.to_string()))?;
    if out.len() > MAX_PAYLOAD_LEN {
        return Err(PayloadError::TooLarge);
    }
    Ok(Cow::Owned(out))
}

fn verifier_options() -> VerifierOptions {
    VerifierOptions {
        max_depth: 16,
        max_tables: 1 << 20,
        max_apparent_size: 1 << 26,
        ignore_missing_null_terminator: false,
    }
}

/// Verifies `buf` (decompressed) as a Folio payload whose body matches the
/// container's `kind`, then validates structure. Zero-copy: the returned
/// view borrows `buf`.
pub fn verify_payload(buf: &[u8], kind: ChunkKind) -> Result<Payload<'_>, PayloadError> {
    // `buffer_has_identifier` asserts len ≥ 8 (root offset + identifier) and
    // would panic on shorter input. Found by `cargo fuzz run payload`.
    if buf.len() < MIN_PAYLOAD_LEN || !payload_buffer_has_identifier(buf) {
        return Err(PayloadError::MissingIdentifier);
    }
    let payload = root_as_payload_with_opts(&verifier_options(), buf)?;
    match (kind, payload.body_type()) {
        (ChunkKind::Flow, Body::FlowChunk) => {
            let flow = payload
                .body_as_flow_chunk()
                .ok_or(PayloadError::KindMismatch {
                    header: kind,
                    body: "missing",
                })?;
            validate_flow(flow)?;
        }
        (ChunkKind::Page, Body::PageChunk) => {
            let page = payload
                .body_as_page_chunk()
                .ok_or(PayloadError::KindMismatch {
                    header: kind,
                    body: "missing",
                })?;
            validate_pages(page)?;
        }
        (_, Body::FlowChunk) => {
            return Err(PayloadError::KindMismatch {
                header: kind,
                body: "FlowChunk",
            });
        }
        (_, Body::PageChunk) => {
            return Err(PayloadError::KindMismatch {
                header: kind,
                body: "PageChunk",
            });
        }
        (_, _) => {
            return Err(PayloadError::KindMismatch {
                header: kind,
                body: "unknown",
            });
        }
    }
    Ok(payload)
}

fn validate_flow(flow: FlowChunk<'_>) -> Result<(), PayloadError> {
    for (b, block) in flow.blocks().iter().enumerate() {
        validate_block(b, block)?;
    }
    Ok(())
}

fn validate_block(b: usize, block: Block<'_>) -> Result<(), PayloadError> {
    let kind = block.kind();
    if kind.variant_name().is_none() {
        return Err(PayloadError::Block {
            block: b,
            detail: "unknown block kind",
        });
    }
    if kind == BlockKind::Heading && !(1..=6).contains(&block.level()) {
        return Err(PayloadError::Block {
            block: b,
            detail: "heading level outside 1–6",
        });
    }
    if kind == BlockKind::Figure && block.figure().is_none() {
        return Err(PayloadError::Block {
            block: b,
            detail: "figure block without figure",
        });
    }
    if block.dir().variant_name().is_none() {
        return Err(PayloadError::Block {
            block: b,
            detail: "unknown direction",
        });
    }
    let runs = block.runs().into_iter().flatten();
    let captions = block
        .figure()
        .and_then(|f| f.caption())
        .into_iter()
        .flatten();
    for (r, run) in runs.chain(captions).enumerate() {
        validate_run(run).map_err(|detail| PayloadError::Run {
            block: b,
            run: r,
            detail,
        })?;
    }
    Ok(())
}

/// Structure-of-arrays invariants the Compositor relies on to index without
/// bounds checks failing mid-layout.
fn validate_run(run: Run<'_>) -> Result<(), &'static str> {
    let n = run.gids().len();
    if n == 0 {
        return Err("empty run");
    }
    if run.advances().len() != n || run.flags().len() != n || run.bidi_levels().len() != n {
        return Err("advances/flags/bidi_levels length differs from gids");
    }
    let opt_len_ok = |len: Option<usize>| len.is_none_or(|l| l == 0 || l == n);
    if !opt_len_ok(run.offsets().map(|v| v.len())) {
        return Err("offsets length differs from gids");
    }
    let prio = run.kashida_priority();
    let max = run.kashida_max();
    if !opt_len_ok(prio.map(|v| v.len())) || !opt_len_ok(max.map(|v| v.len())) {
        return Err("kashida vectors length differs from gids");
    }
    if run.bidi_levels().iter().any(|l| l > MAX_BIDI_LEVEL) {
        return Err("bidi level above 125");
    }
    let flags = run.flags();
    match flags.iter().next() {
        Some(first) if first & GlyphFlags::ClusterStart.bits() != 0 => {}
        _ => return Err("run must start on a cluster boundary"),
    }
    let has_kashida = flags.iter().any(|f| f & GlyphFlags::KashidaOk.bits() != 0);
    let kashida_vectors = prio.is_some_and(|v| v.len() == n) && max.is_some_and(|v| v.len() == n);
    if has_kashida && !kashida_vectors {
        return Err("KashidaOk glyphs without kashida_priority/kashida_max");
    }
    Ok(())
}

fn finite_in(v: f32, lo: f32, hi: f32) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

fn validate_pages(chunk: PageChunk<'_>) -> Result<(), PayloadError> {
    for (p, page) in chunk.pages().iter().enumerate() {
        let err = |detail| PayloadError::Page { page: p, detail };
        if !finite_in(page.width(), 1.0, MAX_PAGE_EXTENT)
            || !finite_in(page.height(), 1.0, MAX_PAGE_EXTENT)
        {
            return Err(err("page size outside 1–14400 pt"));
        }
        let lim = MAX_PAGE_EXTENT;
        for g in page.glyphs().into_iter().flatten() {
            if !finite_in(g.x(), -lim, lim)
                || !finite_in(g.y(), -lim, lim)
                || !finite_in(g.size(), 0.0, lim)
            {
                return Err(err("glyph geometry not finite or out of range"));
            }
        }
        let rects = page
            .blocks()
            .into_iter()
            .flatten()
            .map(|b| *b.rect())
            .chain(page.images().into_iter().flatten().map(|i| *i.rect()));
        for r in rects {
            if ![r.x(), r.y()].iter().all(|v| finite_in(*v, -lim, lim))
                || ![r.w(), r.h()].iter().all(|v| finite_in(*v, 0.0, lim))
            {
                return Err(err("rect not finite or out of range"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (fuzz crash-5ba93c9d): inputs shorter than 8 bytes used to
    /// panic inside `flatbuffers::buffer_has_identifier`.
    #[test]
    fn short_buffers_are_rejected_without_panicking() {
        for len in 0..MIN_PAYLOAD_LEN {
            let buf = vec![0u8; len];
            assert!(matches!(
                verify_payload(&buf, ChunkKind::Flow),
                Err(PayloadError::MissingIdentifier)
            ));
        }
    }

    #[test]
    fn uncompressed_payload_size_is_bounded() {
        let big = vec![0u8; MAX_PAYLOAD_LEN + 1];
        assert!(matches!(
            decompress(&big, ChunkFlags::empty()),
            Err(PayloadError::TooLarge)
        ));
    }
}
