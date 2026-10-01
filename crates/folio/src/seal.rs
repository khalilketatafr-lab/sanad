//! Native sealing (Atelier, Kernel, tests): HKDF chunk-key derivation and
//! AES-256-GCM seal/open over the container framing. Feature `seal`.
//!
//! Key hierarchy (docs/blueprint/01-architecture.md §3.3):
//!
//! ```text
//! CK[idx, variant] = HKDF-SHA256(
//!     ikm  = TMK,
//!     salt = edition_id (16 bytes) ‖ variant (1 byte),
//!     info = "folio/chunk/v1" ‖ idx (u32 LE),
//!     L    = 32)
//! ```

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::error::Unspecified;
use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::codec::{
    ChunkFlags, ChunkHeader, ChunkIdentity, CodecError, FORMAT_VERSION, HEADER_LEN,
    MAX_PLAINTEXT_LEN, NONCE_LEN, SealedChunk, TAG_LEN,
};

pub const CHUNK_KEY_INFO: &[u8] = b"folio/chunk/v1";
pub const KEY_LEN: usize = 32;

/// Title Master Key: 256-bit, one per edition version. Lives in plaintext only
/// inside the Kernel/Atelier process, after a KMS unwrap. Zeroized on drop,
/// never `Debug`-printed.
pub struct TitleMasterKey(Zeroizing<[u8; KEY_LEN]>);

impl TitleMasterKey {
    #[must_use]
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn generate() -> Result<Self, SealError> {
        let mut k = Zeroizing::new([0u8; KEY_LEN]);
        SystemRandom::new()
            .fill(k.as_mut())
            .map_err(|_| SealError::Rng)?;
        Ok(Self(k))
    }
}

impl core::fmt::Debug for TitleMasterKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("TitleMasterKey(<redacted>)")
    }
}

/// Per-chunk AES-256-GCM key. Raw bytes are exposed only so the Kernel can
/// wrap them (AES-KW) into a lease; they never leave the Kernel unwrapped.
pub struct ChunkKey(Zeroizing<[u8; KEY_LEN]>);

impl ChunkKey {
    #[must_use]
    pub fn expose_for_wrapping(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl core::fmt::Debug for ChunkKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ChunkKey(<redacted>)")
    }
}

#[derive(Debug, Error)]
pub enum SealError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("payload of {0} bytes exceeds the chunk limit")]
    TooLarge(usize),
    #[error("system RNG failure")]
    Rng,
    #[error("key derivation failed")]
    Kdf,
    #[error("authentication failed: wrong key, or the chunk was modified")]
    Authentication,
}

impl From<Unspecified> for SealError {
    fn from(_: Unspecified) -> Self {
        Self::Authentication
    }
}

pub fn derive_chunk_key(
    tmk: &TitleMasterKey,
    edition_id: &[u8; 16],
    variant: u8,
    index: u32,
) -> Result<ChunkKey, SealError> {
    let mut salt = [0u8; 17];
    salt[..16].copy_from_slice(edition_id);
    salt[16] = variant;
    let idx = index.to_le_bytes();
    let info: [&[u8]; 2] = [CHUNK_KEY_INFO, &idx];
    let prk = Salt::new(HKDF_SHA256, &salt).extract(tmk.0.as_ref());
    let okm = prk.expand(&info, HKDF_SHA256).map_err(|_| SealError::Kdf)?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    okm.fill(key.as_mut()).map_err(|_| SealError::Kdf)?;
    Ok(ChunkKey(key))
}

fn aead_key(key: &ChunkKey) -> Result<LessSafeKey, SealError> {
    Ok(LessSafeKey::new(UnboundKey::new(
        &AES_256_GCM,
        key.0.as_ref(),
    )?))
}

/// Seals `payload` (already compressed if `flags` has ZSTD) into a complete
/// chunk object: header ‖ ciphertext ‖ tag. The nonce is fresh randomness.
/// Keys are per chunk and objects are written once, so a random 96-bit nonce
/// sits far below GCM's collision bounds.
pub fn seal_chunk(
    key: &ChunkKey,
    id: &ChunkIdentity,
    flags: ChunkFlags,
    payload: &[u8],
) -> Result<Vec<u8>, SealError> {
    let plaintext_len = u32::try_from(payload.len())
        .ok()
        .filter(|l| *l <= MAX_PLAINTEXT_LEN)
        .ok_or(SealError::TooLarge(payload.len()))?;
    let flags = if id.variant != 0 {
        flags | ChunkFlags::HAS_VARIANT
    } else {
        flags
    };
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| SealError::Rng)?;
    let header = ChunkHeader {
        version: FORMAT_VERSION,
        kind: id.kind,
        flags,
        edition_id: id.edition_id,
        chunk_index: id.chunk_index,
        variant: id.variant,
        nonce,
        plaintext_len,
    };
    let header_bytes = header.encode();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(payload);
    let aad = Aad::from(&header_bytes[..crate::codec::AAD_LEN]);
    let tag = aead_key(key)?.seal_in_place_separate_tag(
        Nonce::assume_unique_for_key(nonce),
        aad,
        &mut out[HEADER_LEN..],
    )?;
    out.extend_from_slice(tag.as_ref());
    Ok(out)
}

/// Parses, checks identity against the lease, authenticates and decrypts.
/// Returns the AEAD plaintext (still zstd-compressed if flagged).
pub fn open_chunk(
    key: &ChunkKey,
    bytes: &[u8],
    want: &ChunkIdentity,
) -> Result<Zeroizing<Vec<u8>>, SealError> {
    let chunk = SealedChunk::parse(bytes)?;
    chunk.header.expect(want)?;
    let mut buf = Zeroizing::new(chunk.sealed_body().to_vec());
    let aad = Aad::from(chunk.aad());
    let plain_len = aead_key(key)?
        .open_in_place(
            Nonce::assume_unique_for_key(*chunk.nonce()),
            aad,
            buf.as_mut_slice(),
        )
        .map_err(|_| SealError::Authentication)?
        .len();
    buf.truncate(plain_len);
    Ok(buf)
}
