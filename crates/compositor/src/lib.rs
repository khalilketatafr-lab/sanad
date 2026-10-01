//! # Compositor: layout without text
//!
//! One crate, two targets: native (server-side highlight geometry, quote
//! cards, golden tests) and `wasm32` (the reader's Vault Worker). Layout is
//! byte-identical on both.
//!
//! Input is shaped glyph runs with per-glyph flags (Folio `Run`). There is no
//! Unicode anywhere. Output is positioned permuted glyph ids and per-cluster
//! hit boxes.
//!
//! - [`item`]: runs → Knuth–Plass items (boxes never stretch: no letter-spacing)
//! - [`linebreak`]: total-fit breaking; kashida absorbs slack before glue
//! - [`bidi`]: UAX #9 rule L2 per line, at cluster granularity
//! - [`layout`]: justified, reordered, positioned lines and hit testing
//! - [`ring`]: SharedArrayBuffer pointer-sample ring (lock-free, multi-consumer)
//!
//! Spec: `docs/blueprint/03-reader-engine.md` §3.

pub mod bidi;
pub mod item;
pub mod layout;
pub mod linebreak;
pub mod ring;

#[cfg(target_arch = "wasm32")]
mod wasm;
