//! # Folio: Sanad's encrypted, chunked content container
//!
//! Layers, outermost first:
//!
//! 1. [`codec`]: the 48-byte chunk header and sealed-body framing. It parses
//!    arbitrary input without panicking and does no cryptography.
//! 2. AEAD: AES-256-GCM. In the browser this is `crypto.subtle.decrypt` with a
//!    non-extractable key, fed the slices [`codec::SealedChunk`] exposes.
//!    Natively it is [`seal`] (feature `seal`).
//! 3. [`payload`]: bounded zstd decode, then a verified, zero-copy FlatBuffer
//!    view ([`schema`]) with structural validation.
//!
//! P1: the schema has no string fields. Payloads carry permuted glyph IDs,
//! never Unicode (see `schemas/folio.fbs` and `tests/schema_p1.rs`).
//!
//! Spec: `docs/blueprint/01-architecture.md` §4.

pub mod codec;
pub mod payload;
#[cfg(feature = "seal")]
pub mod seal;

#[allow(
    warnings,
    unsafe_code,
    unreachable_pub,
    missing_debug_implementations,
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#[rustfmt::skip]
mod folio_generated;

/// Generated FlatBuffers bindings for `schemas/folio.fbs` (flatc, pinned in
/// `tools/flatc.version`). Regenerate with `tools/gen-folio.sh`.
pub mod schema {
    pub use crate::folio_generated::sanad::folio::v_1::*;
}

pub use codec::{ChunkFlags, ChunkHeader, ChunkIdentity, ChunkKind, CodecError, SealedChunk};
pub use payload::{PayloadError, decompress, verify_payload};
