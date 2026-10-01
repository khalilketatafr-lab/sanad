//! Seals test chunks of the canary edition under the development catalog's
//! published test key (TMK = 0x42 × 32, Folio's known-answer key), so the
//! Kernel's development leases open them:
//!
//!   cargo run -q -p sanad-folio --features seal --example seal_fixture -- <out-dir>
//!
//! Writes `chunk-<index>-v0.folio` for chunks 0..6 and `manifest.json`
//! (SHA-256 of each plaintext payload). Payloads are opaque pseudo-random
//! bytes: like real Folio payloads they contain no text (P1).

use std::{env, fs, path::PathBuf, process::ExitCode};

use aws_lc_rs::digest::{SHA256, digest};
use sanad_folio::codec::{ChunkFlags, ChunkIdentity, ChunkKind};
use sanad_folio::seal::{TitleMasterKey, derive_chunk_key, seal_chunk};

/// `00000000-0000-7000-8000-00000000ca7a`, as in the Kernel's catalog.
const CANARY_EDITION: [u8; 16] = [0, 0, 0, 0, 0, 0, 0x70, 0, 0x80, 0, 0, 0, 0, 0, 0xca, 0x7a];
const CHUNKS: u32 = 6;
const PAYLOAD_LEN: usize = 24 * 1024;

fn payload(index: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(PAYLOAD_LEN);
    let mut block = digest(&SHA256, format!("canary/{index}").as_bytes());
    while out.len() < PAYLOAD_LEN {
        out.extend_from_slice(block.as_ref());
        block = digest(&SHA256, block.as_ref());
    }
    out.truncate(PAYLOAD_LEN);
    out
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: seal_fixture <out-dir>")?;
    fs::create_dir_all(&out)?;
    let tmk = TitleMasterKey::from_bytes([0x42; 32]);
    let mut manifest = Vec::new();
    for index in 0..CHUNKS {
        let id = ChunkIdentity {
            kind: ChunkKind::Flow,
            edition_id: CANARY_EDITION,
            chunk_index: index,
            variant: 0,
        };
        let key = derive_chunk_key(&tmk, &CANARY_EDITION, 0, index)?;
        let body = payload(index);
        let sealed = seal_chunk(&key, &id, ChunkFlags::empty(), &body)?;
        let file = format!("chunk-{index}-v0.folio");
        fs::write(out.join(&file), sealed)?;
        manifest.push(format!(
            r#"{{"chunk":{index},"variant":0,"file":"{file}","sha256":"{}"}}"#,
            hex(digest(&SHA256, &body).as_ref())
        ));
    }
    fs::write(
        out.join("manifest.json"),
        format!("[{}]\n", manifest.join(",")),
    )?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("seal_fixture: {e}");
            ExitCode::FAILURE
        }
    }
}
