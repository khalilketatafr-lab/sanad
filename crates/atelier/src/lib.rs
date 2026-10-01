//! Atelier: Sanad's ingestion and typesetting pipeline.
//! See docs/blueprint/01-architecture.md §5.
//!
//! Phase 0 contains stage ⑤ (glyph shredding). Shaping, permutation, atlas
//! packing, variants and sealing land with spike S3 integration.

pub mod shred;
