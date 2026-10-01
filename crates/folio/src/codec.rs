//! Folio chunk container: the 48-byte header and the sealed body framing.
//!
//! ```text
//! offset  size  field
//! ──────  ────  ───────────────────────────────────────────────────────────
//! 0       4     magic            "FOLI"
//! 4       1     format version   1
//! 5       1     kind             0 flow · 1 page · 2 atlas page · 3 tile
//! 6       2     flags (u16 LE)   bit0 zstd · bit1 has-variant · bit2 last-in-chapter
//! 8       16    edition_id       UUID bytes (RFC 9562 byte order)
//! 24      4     chunk_index      u32 LE
//! 28      1     variant          0–3 (Phase 1: 0 = A, 1 = B)
//! 29      3     reserved         MUST be zero
//! 32      12    nonce            AES-GCM IV, random per object
//! 44      4     plaintext_len    u32 LE, length of the AEAD plaintext (= ciphertext length)
//! 48      n     ciphertext       AES-256-GCM(CK[idx, variant], nonce, payload, AAD)
//! 48+n    16    tag
//!
//! AAD = bytes[0..32]: the header up to the nonce is authenticated, so a chunk
//! cannot be replayed under another edition, index, variant or kind. The nonce
//! is a GCM input and `plaintext_len` is checked against the actual framing,
//! so every header byte is bound.
//! ```
//!
//! The parser is total over arbitrary input: it never panics, never allocates
//! and never reads out of bounds (`cargo fuzz run chunk_header` keeps it that
//! way). It does no cryptography. It hands out exactly the slices that
//! `crypto.subtle.decrypt` (client) or [`crate::seal::open_chunk`] (native) need.

use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"FOLI";
pub const FORMAT_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 48;
pub const AAD_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
/// Upper bound on one chunk's AEAD plaintext. Real chunks are 16–48 KiB; this
/// bound only caps allocations for hostile input.
pub const MAX_PLAINTEXT_LEN: u32 = 4 * 1024 * 1024;
pub const MAX_VARIANTS: u8 = 4;

/// What the chunk body contains (header byte 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ChunkKind {
    /// Reflowable content: a `FlowChunk` payload.
    Flow = 0,
    /// Fixed-layout content: a `PageChunk` payload.
    Page = 1,
    /// One page of a shredded MSDF glyph atlas.
    AtlasPage = 2,
    /// One image-pyramid tile.
    Tile = 3,
}

impl ChunkKind {
    const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Flow),
            1 => Some(Self::Page),
            2 => Some(Self::AtlasPage),
            3 => Some(Self::Tile),
            _ => None,
        }
    }
}

/// Header flags (u16 LE at offset 6). Unknown bits are rejected, not ignored:
/// a future format must bump the version instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ChunkFlags(u16);

impl ChunkFlags {
    pub const ZSTD: Self = Self(1 << 0);
    pub const HAS_VARIANT: Self = Self(1 << 1);
    pub const LAST_IN_CHAPTER: Self = Self(1 << 2);
    const KNOWN: u16 = 0b111;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn from_bits(bits: u16) -> Result<Self, CodecError> {
        if bits & !Self::KNOWN != 0 {
            return Err(CodecError::UnknownFlags(bits));
        }
        Ok(Self(bits))
    }
}

impl core::ops::BitOr for ChunkFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkHeader {
    pub version: u8,
    pub kind: ChunkKind,
    pub flags: ChunkFlags,
    pub edition_id: [u8; 16],
    pub chunk_index: u32,
    pub variant: u8,
    pub nonce: [u8; NONCE_LEN],
    pub plaintext_len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CodecError {
    #[error("chunk truncated: {got} bytes, need at least {need}")]
    Truncated { got: usize, need: usize },
    #[error("bad magic {0:02x?}")]
    BadMagic([u8; 4]),
    #[error("unsupported format version {0}")]
    UnsupportedVersion(u8),
    #[error("unknown chunk kind {0}")]
    UnknownKind(u8),
    #[error("unknown header flags {0:#06x}")]
    UnknownFlags(u16),
    #[error("variant {0} out of range")]
    BadVariant(u8),
    #[error("variant flag and variant byte disagree")]
    VariantFlagMismatch,
    #[error("reserved header bytes are not zero")]
    ReservedNotZero,
    #[error("declared plaintext length {declared} exceeds limit {limit}")]
    TooLarge { declared: u32, limit: u32 },
    #[error("framing mismatch: header declares {declared} plaintext bytes, body carries {actual}")]
    LengthMismatch { declared: u32, actual: usize },
    #[error("chunk identity mismatch: {0} differs from what the lease requested")]
    IdentityMismatch(&'static str),
}

fn read_u16_le(b: &[u8; HEADER_LEN], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn read_u32_le(b: &[u8; HEADER_LEN], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl ChunkHeader {
    /// Parses and validates the fixed 48-byte header.
    pub fn parse(bytes: &[u8; HEADER_LEN]) -> Result<Self, CodecError> {
        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if magic != MAGIC {
            return Err(CodecError::BadMagic(magic));
        }
        let version = bytes[4];
        if version != FORMAT_VERSION {
            return Err(CodecError::UnsupportedVersion(version));
        }
        let kind = ChunkKind::from_byte(bytes[5]).ok_or(CodecError::UnknownKind(bytes[5]))?;
        let flags = ChunkFlags::from_bits(read_u16_le(bytes, 6))?;
        let variant = bytes[28];
        if variant >= MAX_VARIANTS {
            return Err(CodecError::BadVariant(variant));
        }
        // A non-zero variant only makes sense when the object is varianted.
        if variant != 0 && !flags.contains(ChunkFlags::HAS_VARIANT) {
            return Err(CodecError::VariantFlagMismatch);
        }
        if bytes[29..32] != [0, 0, 0] {
            return Err(CodecError::ReservedNotZero);
        }
        let plaintext_len = read_u32_le(bytes, 44);
        if plaintext_len > MAX_PLAINTEXT_LEN {
            return Err(CodecError::TooLarge {
                declared: plaintext_len,
                limit: MAX_PLAINTEXT_LEN,
            });
        }
        let mut edition_id = [0u8; 16];
        edition_id.copy_from_slice(&bytes[8..24]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[32..44]);
        Ok(Self {
            version,
            kind,
            flags,
            edition_id,
            chunk_index: read_u32_le(bytes, 24),
            variant,
            nonce,
            plaintext_len,
        })
    }

    /// Serializes the header. `encode(parse(b)) == b` for every valid `b`.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = self.version;
        out[5] = self.kind as u8;
        out[6..8].copy_from_slice(&self.flags.bits().to_le_bytes());
        out[8..24].copy_from_slice(&self.edition_id);
        out[24..28].copy_from_slice(&self.chunk_index.to_le_bytes());
        out[28] = self.variant;
        out[32..44].copy_from_slice(&self.nonce);
        out[44..48].copy_from_slice(&self.plaintext_len.to_le_bytes());
        out
    }

    /// Fails fast when a fetched object is not the one the lease asked for.
    /// The AEAD would reject a swapped chunk anyway (CK is per index and
    /// variant), but this gives a precise error before any crypto runs.
    pub fn expect(&self, want: &ChunkIdentity) -> Result<(), CodecError> {
        let checks: [(bool, &'static str); 4] = [
            (self.kind == want.kind, "kind"),
            (self.edition_id == want.edition_id, "edition_id"),
            (self.chunk_index == want.chunk_index, "chunk_index"),
            (self.variant == want.variant, "variant"),
        ];
        match checks.iter().find(|(ok, _)| !ok) {
            Some((_, field)) => Err(CodecError::IdentityMismatch(field)),
            None => Ok(()),
        }
    }
}

/// The identity a lease promises for an object: what the client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkIdentity {
    pub kind: ChunkKind,
    pub edition_id: [u8; 16],
    pub chunk_index: u32,
    pub variant: u8,
}

/// A parsed, framing-validated chunk that borrows the input buffer.
/// Holds no plaintext: the body is still sealed.
#[derive(Debug, Clone, Copy)]
pub struct SealedChunk<'a> {
    pub header: ChunkHeader,
    bytes: &'a [u8],
}

impl<'a> SealedChunk<'a> {
    /// Parses the header and checks that the body is exactly
    /// `plaintext_len + TAG_LEN` bytes. Total over arbitrary input.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, CodecError> {
        let need = HEADER_LEN + TAG_LEN;
        let Some(header_bytes) = bytes.first_chunk::<HEADER_LEN>() else {
            return Err(CodecError::Truncated {
                got: bytes.len(),
                need,
            });
        };
        let header = ChunkHeader::parse(header_bytes)?;
        if bytes.len() < need {
            return Err(CodecError::Truncated {
                got: bytes.len(),
                need,
            });
        }
        let body = bytes.len() - HEADER_LEN - TAG_LEN;
        if body != header.plaintext_len as usize {
            return Err(CodecError::LengthMismatch {
                declared: header.plaintext_len,
                actual: body,
            });
        }
        Ok(Self { header, bytes })
    }

    /// `additionalData` for AES-GCM: header bytes `[0, 32)`.
    #[must_use]
    pub fn aad(&self) -> &'a [u8] {
        &self.bytes[..AAD_LEN]
    }

    /// AES-GCM IV (header bytes `[32, 44)`).
    #[must_use]
    pub const fn nonce(&self) -> &[u8; NONCE_LEN] {
        &self.header.nonce
    }

    /// `ciphertext ‖ tag`, the exact `data` argument WebCrypto's
    /// `subtle.decrypt({ name: "AES-GCM", iv, additionalData, tagLength: 128 }, key, data)`
    /// expects. Zero-copy: JS receives a subarray of the fetched buffer.
    #[must_use]
    pub fn sealed_body(&self) -> &'a [u8] {
        &self.bytes[HEADER_LEN..]
    }

    /// Byte range of [`Self::sealed_body`] within the input, for JS bindings
    /// that hand WebCrypto a subarray view without copying.
    #[must_use]
    pub const fn sealed_range(&self) -> core::ops::Range<usize> {
        HEADER_LEN..self.bytes.len()
    }

    #[must_use]
    pub fn ciphertext(&self) -> &'a [u8] {
        &self.bytes[HEADER_LEN..self.bytes.len() - TAG_LEN]
    }

    #[must_use]
    pub fn tag(&self) -> &'a [u8] {
        &self.bytes[self.bytes.len() - TAG_LEN..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChunkHeader {
        ChunkHeader {
            version: FORMAT_VERSION,
            kind: ChunkKind::Flow,
            flags: ChunkFlags::ZSTD | ChunkFlags::HAS_VARIANT,
            edition_id: [0xca; 16],
            chunk_index: 21,
            variant: 1,
            nonce: [7; NONCE_LEN],
            plaintext_len: 5,
        }
    }

    fn framed(h: &ChunkHeader) -> Vec<u8> {
        let mut v = h.encode().to_vec();
        v.extend(core::iter::repeat_n(
            0xee,
            h.plaintext_len as usize + TAG_LEN,
        ));
        v
    }

    #[test]
    fn header_round_trips_and_layout_is_exact() {
        let h = sample();
        let bytes = h.encode();
        assert_eq!(&bytes[0..4], b"FOLI");
        assert_eq!(bytes[5], 0);
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 0b011);
        assert_eq!(&bytes[24..28], &21u32.to_le_bytes());
        assert_eq!(bytes[28], 1);
        assert_eq!(&bytes[44..48], &5u32.to_le_bytes());
        assert_eq!(ChunkHeader::parse(&bytes), Ok(h));
    }

    #[test]
    fn sealed_chunk_exposes_webcrypto_slices() {
        let h = sample();
        let bytes = framed(&h);
        let c = SealedChunk::parse(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.aad(), &bytes[..32]);
        assert_eq!(c.nonce(), &[7; 12]);
        assert_eq!(c.sealed_body().len(), 5 + 16);
        assert_eq!(c.ciphertext().len(), 5);
        assert_eq!(c.tag().len(), 16);
        assert_eq!(c.sealed_range(), 48..bytes.len());
    }

    #[test]
    fn rejects_malformed_headers() {
        let good = sample().encode();
        let mut b = good;
        b[0] = b'X';
        assert!(matches!(
            ChunkHeader::parse(&b),
            Err(CodecError::BadMagic(_))
        ));
        b = good;
        b[4] = 2;
        assert_eq!(
            ChunkHeader::parse(&b),
            Err(CodecError::UnsupportedVersion(2))
        );
        b = good;
        b[5] = 9;
        assert_eq!(ChunkHeader::parse(&b), Err(CodecError::UnknownKind(9)));
        b = good;
        b[7] = 0x80;
        assert!(matches!(
            ChunkHeader::parse(&b),
            Err(CodecError::UnknownFlags(_))
        ));
        b = good;
        b[28] = 4;
        assert_eq!(ChunkHeader::parse(&b), Err(CodecError::BadVariant(4)));
        b = good;
        b[6] = ChunkFlags::ZSTD.bits() as u8; // variant 1 without HAS_VARIANT
        assert_eq!(ChunkHeader::parse(&b), Err(CodecError::VariantFlagMismatch));
        b = good;
        b[30] = 1;
        assert_eq!(ChunkHeader::parse(&b), Err(CodecError::ReservedNotZero));
        b = good;
        b[44..48].copy_from_slice(&(MAX_PLAINTEXT_LEN + 1).to_le_bytes());
        assert!(matches!(
            ChunkHeader::parse(&b),
            Err(CodecError::TooLarge { .. })
        ));
    }

    #[test]
    fn rejects_bad_framing() {
        let h = sample();
        let bytes = framed(&h);
        assert!(matches!(
            SealedChunk::parse(&bytes[..47]),
            Err(CodecError::Truncated { .. })
        ));
        assert!(matches!(
            SealedChunk::parse(&bytes[..bytes.len() - 1]),
            Err(CodecError::LengthMismatch { .. })
        ));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            SealedChunk::parse(&longer),
            Err(CodecError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn expect_checks_identity() {
        let h = sample();
        let want = ChunkIdentity {
            kind: ChunkKind::Flow,
            edition_id: [0xca; 16],
            chunk_index: 21,
            variant: 1,
        };
        assert_eq!(h.expect(&want), Ok(()));
        assert!(
            h.expect(&ChunkIdentity {
                chunk_index: 22,
                ..want
            })
            .is_err()
        );
        assert!(h.expect(&ChunkIdentity { variant: 0, ..want }).is_err());
    }
}
