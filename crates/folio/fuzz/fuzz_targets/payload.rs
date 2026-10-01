//! Post-decryption payload path: bounded zstd decode then FlatBuffer
//! verification and structural validation, for every chunk kind. Must never
//! panic or allocate past MAX_PAYLOAD_LEN, whatever the bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sanad_folio::codec::{ChunkFlags, ChunkKind};
use sanad_folio::payload::{decompress, verify_payload, MAX_PAYLOAD_LEN};

fuzz_target!(|data: &[u8]| {
    let Some((&selector, body)) = data.split_first() else { return };
    let flags = if selector & 1 == 1 { ChunkFlags::ZSTD } else { ChunkFlags::empty() };
    let kind = if selector & 2 == 2 { ChunkKind::Page } else { ChunkKind::Flow };
    if let Ok(decoded) = decompress(body, flags) {
        assert!(decoded.len() <= MAX_PAYLOAD_LEN);
        let _ = verify_payload(&decoded, kind);
    }
});
