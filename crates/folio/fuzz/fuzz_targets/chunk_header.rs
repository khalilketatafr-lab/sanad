//! The container parser must be total: arbitrary bytes from the CDN (or an
//! attacker) may produce an error, never a panic, OOB read or hang. When a
//! header parses, encode(parse(h)) must reproduce the exact bytes, and the
//! exposed WebCrypto slices must tile the input exactly.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sanad_folio::codec::{ChunkHeader, SealedChunk, AAD_LEN, HEADER_LEN, TAG_LEN};

fuzz_target!(|data: &[u8]| {
    if let Some(header) = data.first_chunk::<HEADER_LEN>() {
        if let Ok(parsed) = ChunkHeader::parse(header) {
            assert_eq!(&parsed.encode(), header, "header round trip");
        }
    }
    if let Ok(chunk) = SealedChunk::parse(data) {
        assert_eq!(chunk.aad(), &data[..AAD_LEN]);
        assert_eq!(chunk.sealed_body().len(), data.len() - HEADER_LEN);
        assert_eq!(chunk.ciphertext().len() + chunk.tag().len(), chunk.sealed_body().len());
        assert_eq!(chunk.tag().len(), TAG_LEN);
        assert_eq!(chunk.ciphertext().len(), chunk.header.plaintext_len as usize);
        assert_eq!(&data[chunk.sealed_range()], chunk.sealed_body());
    }
});
