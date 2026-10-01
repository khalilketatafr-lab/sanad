//! Ingest stage ③: glyph permutation.
//!
//! Every (font, font glyph id) an edition uses gets a random, unique 16-bit
//! edition id. Chunks, atlases and the Compositor only ever see edition ids,
//! so a glyph id carries no information about the character it draws, and
//! ids differ between editions (no cross-edition codebook).
//!
//! Ids are drawn uniformly from `1..=65535` (0 is reserved), not packed
//! densely, so even an id's magnitude says nothing about the font's glyph
//! order. The seed is per edition and secret: production draws it from the
//! OS CSPRNG and it never leaves Atelier.

use std::collections::{BTreeMap, BTreeSet};

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use thiserror::Error;

use crate::shape::FontId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    pub font: FontId,
    pub gid: u16,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermuteError {
    #[error("{0} glyphs exceed the 65535 edition ids")]
    TooMany(usize),
}

#[derive(Debug, Clone)]
pub struct Permutation {
    forward: BTreeMap<GlyphKey, u16>,
    inverse: BTreeMap<u16, GlyphKey>,
}

/// Uniform integer in `0..n` by rejection sampling (no modulo bias).
fn below(rng: &mut ChaCha20Rng, n: u32) -> u32 {
    let zone = u32::MAX - (u32::MAX % n);
    loop {
        let v = rng.next_u32();
        if v < zone {
            return v % n;
        }
    }
}

impl Permutation {
    pub fn new(
        keys: impl IntoIterator<Item = GlyphKey>,
        seed: [u8; 32],
    ) -> Result<Self, PermuteError> {
        let keys: BTreeSet<GlyphKey> = keys.into_iter().collect();
        if keys.len() > usize::from(u16::MAX) {
            return Err(PermuteError::TooMany(keys.len()));
        }
        let mut rng = ChaCha20Rng::from_seed(seed);
        // Partial Fisher–Yates over the id space: the first n slots become a
        // uniformly random n-subset in uniformly random order.
        let mut ids: Vec<u16> = (1..=u16::MAX).collect();
        let mut forward = BTreeMap::new();
        let mut inverse = BTreeMap::new();
        for (i, key) in keys.into_iter().enumerate() {
            let j = i + below(&mut rng, (ids.len() - i) as u32) as usize;
            ids.swap(i, j);
            forward.insert(key, ids[i]);
            inverse.insert(ids[i], key);
        }
        Ok(Self { forward, inverse })
    }

    #[must_use]
    pub fn get(&self, key: GlyphKey) -> Option<u16> {
        self.forward.get(&key).copied()
    }

    #[must_use]
    pub fn key(&self, id: u16) -> Option<GlyphKey> {
        self.inverse.get(&id).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Edition ids in ascending order with their keys.
    pub fn iter(&self) -> impl Iterator<Item = (u16, GlyphKey)> + '_ {
        self.inverse.iter().map(|(&id, &k)| (id, k))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn keys(n: u16) -> impl Iterator<Item = GlyphKey> {
        (0..n).map(|gid| GlyphKey {
            font: FontId((gid % 2) as u8),
            gid,
        })
    }

    #[test]
    fn bijective_nonzero_and_seed_dependent() {
        let a = Permutation::new(keys(500), [7; 32]).unwrap();
        let b = Permutation::new(keys(500), [8; 32]).unwrap();
        assert_eq!(a.len(), 500);
        let ids: BTreeSet<u16> = keys(500).map(|k| a.get(k).unwrap()).collect();
        assert_eq!(ids.len(), 500, "unique");
        assert!(!ids.contains(&0), "0 is reserved");
        for k in keys(500) {
            assert_eq!(a.key(a.get(k).unwrap()), Some(k));
        }
        let same = keys(500).filter(|&k| a.get(k) == b.get(k)).count();
        assert!(
            same < 5,
            "editions must not share a codebook ({same} equal ids)"
        );
        // Deterministic for a given seed.
        let a2 = Permutation::new(keys(500), [7; 32]).unwrap();
        assert!(keys(500).all(|k| a.get(k) == a2.get(k)));
    }

    #[test]
    fn ids_do_not_preserve_glyph_order() {
        let p = Permutation::new(keys(200), [1; 32]).unwrap();
        let ordered = keys(200)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| p.get(w[0]) < p.get(w[1]))
            .count();
        // A random order has ~half its adjacent pairs ascending.
        assert!(
            (60..140).contains(&ordered),
            "{ordered}/199 ascending pairs"
        );
    }
}
