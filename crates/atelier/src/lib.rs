//! Atelier: Sanad's ingestion and typesetting pipeline.
//! See docs/blueprint/01-architecture.md §5.
//!
//! Phase 0 slice: shaping (②), glyph permutation (③), MSDF generation and
//! shredding into power-of-two atlases (④–⑤) and page composition through the
//! Compositor, enough to render golden pages in Lumen. Variants, image
//! classification and sealing into Folio chunks land with spike S3.
//!
//! Atelier runs server-side only. It is the one component that sees Unicode
//! text and real glyph ids; everything it emits for the client is permuted
//! and shredded (P1).

pub mod atlas;
pub mod catalog;
pub mod epub;
pub mod msdf;
pub mod page;
pub mod permute;
pub mod qa;
pub mod script;
pub mod shape;
pub mod shred;
