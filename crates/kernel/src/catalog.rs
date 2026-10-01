//! Edition catalog: which editions exist, their key material and policy.
//!
//! Phase 0 stub. Production loads editions from Postgres, where each Title
//! Master Key is stored only as a KMS-wrapped blob and unwrapped into memory
//! for minutes (blueprint 01 §3.3). The development catalog holds the canary
//! book under a *published test key*: the TMK of Folio's known-answer vector,
//! so chunks sealed by `cargo run -p sanad-folio --example seal_fixture` open
//! with development leases. `KernelConfig::from_env` never enables it when
//! `SANAD_ENV=production`.

use std::collections::HashMap;

use sanad_folio::seal::{ChunkKey, SealError, TitleMasterKey, derive_chunk_key};
use uuid::Uuid;

/// The canary book (`fixtures/books/canary-book`), the reader's test route.
pub const CANARY_EDITION: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ca7a);
/// Published test key. Never used outside the development catalog.
const CANARY_TEST_TMK: [u8; 32] = [0x42; 32];

#[derive(Debug)]
pub struct Edition {
    pub id: Uuid,
    /// Chunks per variant.
    pub chunks: u32,
    /// Free editions may be opened by anonymous (sampling) devices.
    pub free: bool,
    tmk: TitleMasterKey,
}

impl Edition {
    /// `CK[index, variant]` (Folio's HKDF schedule).
    pub fn chunk_key(&self, index: u32, variant: u8) -> Result<ChunkKey, SealError> {
        derive_chunk_key(&self.tmk, self.id.as_bytes(), variant, index)
    }
}

#[derive(Debug, Default)]
pub struct Catalog {
    editions: HashMap<Uuid, Edition>,
}

impl Catalog {
    /// Development: the canary book only (6 chunks, free, test key).
    #[must_use]
    pub fn development() -> Self {
        let canary = Edition {
            id: CANARY_EDITION,
            chunks: 6,
            free: true,
            tmk: TitleMasterKey::from_bytes(CANARY_TEST_TMK),
        };
        Self {
            editions: HashMap::from([(canary.id, canary)]),
        }
    }

    #[must_use]
    pub fn get(&self, id: &Uuid) -> Option<&Edition> {
        self.editions.get(id)
    }
}
