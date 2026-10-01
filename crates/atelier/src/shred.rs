//! Glyph shredding (ingest stage ⑤): split each glyph's multi-channel signed
//! distance field into k ≥ 2 fragments that live in unrelated atlas slots, so
//! no atlas slot is a recognizable character.
//!
//! # Construction
//!
//! Let `s_c(p)` be channel `c` of the glyph's MSDF at texel-space point `p`,
//! in normalized form (`0.5` on the edge, `> 0.5` inside, range `ρ` texels:
//! `s = d/ρ + ½`). Pick k seeded sites `q_1…q_k` inside the glyph's bounds.
//! They define a Voronoi partition of the plane into convex cells
//! `C_k = { p : |p − q_k| ≤ |p − q_j| ∀j }`.
//!
//! The exact signed distance from `p` to the boundary of `C_k` (positive
//! inside) is the minimum over its bisectors:
//!
//! ```text
//! δ_k(p) = min_{j≠k} ( |p − q_j|² − |p − q_k|² ) / ( 2 |q_j − q_k| )
//! ```
//!
//! Dilate each cell by an overlap `ω` texels and encode it like the glyph:
//! `r_k(p) = clamp( (δ_k(p) + ω)/ρ + ½, 0, 1 )`. Fragment `k` stores, per
//! channel: `f_{k,c}(p) = min( s_c(p), r_k(p) )`.
//!
//! # Why the GPU's MAX blend reassembles the glyph exactly
//!
//! 1. *Median commutes with a common min.* `x ↦ min(x, r)` is monotone, and
//!    the median of three values commutes with any monotone map applied to
//!    all three: `median(min(a,r), min(b,r), min(c,r)) = min(median(a,b,c), r)`.
//!    So each fragment's decoded distance is `min(sd, r_k)`.
//! 2. *Every point is saturated by its own cell.* Any `p` lies in some cell,
//!    so `δ_k(p) ≥ 0` for that `k`. With `ω ≥ ρ/2`, `r_k(p) = 1 ≥ s_c(p)`.
//! 3. *MAX distributes over min.* `max_k min(sd, r_k) = min(sd, max_k r_k) = sd`.
//!    Pass 1 renders each fragment's (monotone) distance encoding and blends
//!    with `MAX`, so the coverage target equals the unshredded glyph's.
//! 4. *Bilinear filtering.* The GPU interpolates the 4 texels around a sample.
//!    They are within `√2` texels of the sample, so `ω ≥ ρ/2 + √2` keeps all
//!    four saturated in the sample's own cell. [`min_overlap`] uses `ρ/2 + 2`.
//!
//! The union is therefore exact, not approximate. Additive or alpha-over
//! blending double-counts the overlap band where a cut crosses the outline:
//! `1 − (1 − a)(1 − b) > max(a, b)` for partial coverage. That would draw a
//! dark seam (see the WebGL test in `packages/lumen/test`).
//!
//! Spec and security rationale: `docs/spikes/s1-glyph-shredding.md`.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Error, PartialEq)]
pub enum ShredError {
    #[error("need at least 2 sites, got {0}")]
    TooFewSites(usize),
    #[error("sites {0} and {1} coincide")]
    CoincidentSites(usize, usize),
    #[error("overlap {got} below the exactness bound {min} (ρ/2 + 2 texels)")]
    OverlapTooSmall { got: f32, min: f32 },
    #[error("could not place {0} sites with the requested separation")]
    Placement(usize),
    #[error("distance image size mismatch")]
    SizeMismatch,
}

/// Minimum cell dilation (texels) for an exact, filter-safe union.
#[must_use]
pub fn min_overlap(px_range: f32) -> f32 {
    px_range / 2.0 + 2.0
}

/// A multi-channel distance image in normalized MSDF form (row-major).
#[derive(Debug, Clone, PartialEq)]
pub struct DistanceImage {
    pub width: usize,
    pub height: usize,
    /// Distance range ρ in texels: normalized value = d/ρ + 0.5.
    pub px_range: f32,
    pub texels: Vec<[f32; 3]>,
}

impl DistanceImage {
    /// Samples a signed distance function (texel units, positive inside) at texel centers.
    pub fn from_sdf(
        width: usize,
        height: usize,
        px_range: f32,
        sdf: impl Fn(Point) -> [f32; 3],
    ) -> Self {
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let d = sdf(Point {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                });
                texels.push(d.map(|c| encode(c, px_range)));
            }
        }
        Self {
            width,
            height,
            px_range,
            texels,
        }
    }

    #[must_use]
    pub fn median_at(&self, i: usize) -> f32 {
        self.texels.get(i).map_or(0.0, |t| median3(*t))
    }

    /// Bilinear sample at texel-space point `p` (texel centers at +0.5), clamped to edge.
    #[must_use]
    #[allow(clippy::many_single_char_names)] // a b c d = the 4 footprint texels, as in the literature
    pub fn bilinear(&self, p: Point) -> [f32; 3] {
        let fx = (p.x - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (p.y - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.texels[y * self.width + x];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        core::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }

    /// RGBA8 for atlas upload (alpha 255). Quantization is monotone, so the
    /// exact-union property survives it.
    #[must_use]
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.texels.len() * 4);
        for t in &self.texels {
            for c in t {
                out.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            out.push(255);
        }
        out
    }
}

#[must_use]
pub fn encode(d_texels: f32, px_range: f32) -> f32 {
    (d_texels / px_range + 0.5).clamp(0.0, 1.0)
}

#[must_use]
pub fn median3(v: [f32; 3]) -> f32 {
    v[0].min(v[1]).max(v[0].max(v[1]).min(v[2]))
}

/// A Voronoi partition with cell dilation.
#[derive(Debug, Clone, PartialEq)]
pub struct VoronoiCut {
    sites: Vec<Point>,
    overlap: f32,
}

impl VoronoiCut {
    pub fn new(sites: Vec<Point>, overlap: f32, px_range: f32) -> Result<Self, ShredError> {
        if sites.len() < 2 {
            return Err(ShredError::TooFewSites(sites.len()));
        }
        for (i, a) in sites.iter().enumerate() {
            for (j, b) in sites.iter().enumerate().skip(i + 1) {
                if (a.x - b.x).hypot(a.y - b.y) < 1e-3 {
                    return Err(ShredError::CoincidentSites(i, j));
                }
            }
        }
        let min = min_overlap(px_range);
        if overlap < min {
            return Err(ShredError::OverlapTooSmall { got: overlap, min });
        }
        Ok(Self { sites, overlap })
    }

    /// Deterministic placement from a per-(edition, glyph) secret seed: cut
    /// patterns differ per edition, so knowledge of one edition's shredding
    /// says nothing about another's. Sites are uniform in `bounds` with a
    /// minimum pairwise separation, so every fragment carries a real share.
    pub fn seeded(
        seed: u64,
        bounds: Rect,
        k: usize,
        min_separation: f32,
        overlap: f32,
        px_range: f32,
    ) -> Result<Self, ShredError> {
        let mut rng = SplitMix64(seed);
        let mut sites: Vec<Point> = Vec::with_capacity(k);
        let mut attempts = 0;
        while sites.len() < k {
            attempts += 1;
            if attempts > 10_000 {
                return Err(ShredError::Placement(k));
            }
            let p = Point {
                x: bounds.x + rng.unit() * bounds.w,
                y: bounds.y + rng.unit() * bounds.h,
            };
            if sites
                .iter()
                .all(|q| (p.x - q.x).hypot(p.y - q.y) >= min_separation)
            {
                sites.push(p);
            }
        }
        Self::new(sites, overlap, px_range)
    }

    #[must_use]
    pub fn sites(&self) -> &[Point] {
        &self.sites
    }

    /// Exact signed distance (texels) from `p` to the boundary of cell `k`,
    /// positive inside. For points outside the cell it is a lower bound,
    /// which is conservative for our purpose.
    #[must_use]
    pub fn cell_distance(&self, k: usize, p: Point) -> f32 {
        let qk = self.sites[k];
        self.sites
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .map(|(_, qj)| {
                let dk = (p.x - qk.x).powi(2) + (p.y - qk.y).powi(2);
                let dj = (p.x - qj.x).powi(2) + (p.y - qj.y).powi(2);
                (dj - dk) / (2.0 * (qj.x - qk.x).hypot(qj.y - qk.y))
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// Normalized region field r_k(p) for cell k dilated by the overlap.
    #[must_use]
    pub fn region(&self, k: usize, p: Point, px_range: f32) -> f32 {
        encode(self.cell_distance(k, p) + self.overlap, px_range)
    }
}

/// Bakes fragment k: per channel `min(s_c, r_k)`.
#[must_use]
pub fn bake_fragment(glyph: &DistanceImage, cut: &VoronoiCut, k: usize) -> DistanceImage {
    let mut texels = Vec::with_capacity(glyph.texels.len());
    for y in 0..glyph.height {
        for x in 0..glyph.width {
            let p = Point {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            };
            let r = cut.region(k, p, glyph.px_range);
            let s = glyph.texels[y * glyph.width + x];
            texels.push(s.map(|c| c.min(r)));
        }
    }
    DistanceImage {
        width: glyph.width,
        height: glyph.height,
        px_range: glyph.px_range,
        texels,
    }
}

/// Tight texel bounds of a fragment's non-empty support (any channel > 0),
/// which is what gets packed into the atlas. `None` if empty.
#[must_use]
pub fn support_bounds(frag: &DistanceImage) -> Option<(usize, usize, usize, usize)> {
    let mut b: Option<(usize, usize, usize, usize)> = None;
    for y in 0..frag.height {
        for x in 0..frag.width {
            if frag.texels[y * frag.width + x].iter().any(|c| *c > 0.0) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

/// Share of the glyph's inked texels (median > 0.5) that fragment k carries.
/// Atelier rejects cuts where one fragment holds most of the glyph (it would
/// remain recognizable on its own).
#[must_use]
pub fn ink_share(glyph: &DistanceImage, frag: &DistanceImage) -> f32 {
    let mut total = 0usize;
    let mut mine = 0usize;
    for i in 0..glyph.texels.len() {
        if glyph.median_at(i) > 0.5 {
            total += 1;
            if frag.median_at(i) > 0.5 {
                mine += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        mine as f32 / total as f32
    }
}

/// SplitMix64: tiny, well-distributed, deterministic. The seed is secret
/// (derived per edition and glyph); the generator need not be cryptographic.
#[derive(Debug, Clone)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A ring with a stem: a stand-in for a bowl-and-stem letter (b, d, p, q).
#[must_use]
pub fn demo_glyph(size: usize, px_range: f32) -> DistanceImage {
    let s = size as f32;
    let (cx, cy, radius, half) = (s * 0.55, s * 0.6, s * 0.24, s * 0.07);
    let (stem_x0, stem_x1, stem_y0, stem_y1) = (s * 0.18, s * 0.31, s * 0.12, s * 0.86);
    DistanceImage::from_sdf(size, size, px_range, |p| {
        let ring = half - ((p.x - cx).hypot(p.y - cy) - radius).abs();
        let dx = (stem_x0 - p.x).max(p.x - stem_x1);
        let dy = (stem_y0 - p.y).max(p.y - stem_y1);
        let outside = dx.max(0.0).hypot(dy.max(0.0));
        let stem = if outside > 0.0 { -outside } else { -dx.max(dy) };
        let d = ring.max(stem);
        [d, d, d]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn union_of_fragments(frags: &[DistanceImage], p: Point) -> f32 {
        frags
            .iter()
            .map(|f| median3(f.bilinear(p)))
            .fold(0.0, f32::max)
    }

    #[test]
    fn rejects_unsafe_parameters() {
        let p = |x, y| Point { x, y };
        assert_eq!(
            VoronoiCut::new(vec![p(1.0, 1.0)], 10.0, 6.0),
            Err(ShredError::TooFewSites(1))
        );
        assert!(matches!(
            VoronoiCut::new(vec![p(1.0, 1.0), p(2.0, 2.0)], 1.0, 6.0),
            Err(ShredError::OverlapTooSmall { .. })
        ));
        assert_eq!(
            VoronoiCut::new(vec![p(1.0, 1.0), p(1.0, 1.0)], 10.0, 6.0),
            Err(ShredError::CoincidentSites(0, 1))
        );
    }

    #[test]
    fn demo_glyph_shreds_into_partial_fragments() {
        let px_range = 6.0;
        let glyph = demo_glyph(96, px_range);
        let cut = VoronoiCut::seeded(
            0x5A4E_AD00,
            Rect {
                x: 16.0,
                y: 16.0,
                w: 64.0,
                h: 64.0,
            },
            3,
            24.0,
            min_overlap(px_range),
            px_range,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let frags: Vec<_> = (0..3).map(|k| bake_fragment(&glyph, &cut, k)).collect();
        let shares: Vec<f32> = frags.iter().map(|f| ink_share(&glyph, f)).collect();
        // Every fragment carries something, and none carries (nearly) the whole glyph.
        assert!(
            shares.iter().all(|s| *s > 0.05 && *s < 0.8),
            "shares {shares:?}"
        );
        // Exact union at every texel center.
        for i in 0..glyph.texels.len() {
            let u = frags.iter().map(|f| f.median_at(i)).fold(0.0, f32::max);
            assert_eq!(u.to_bits(), glyph.median_at(i).to_bits(), "texel {i}");
        }
    }

    proptest! {
        /// The core claim: for arbitrary MSDF channels (not just true SDFs), arbitrary
        /// site placements and arbitrary sub-texel sample points, the MAX-union of the
        /// bilinearly filtered fragments equals the bilinearly filtered glyph.
        #[test]
        fn max_union_of_fragments_is_exact(
            channels in proptest::collection::vec(proptest::array::uniform3(0.0f32..1.0), 24 * 24),
            sites in proptest::collection::vec((0.0f32..24.0, 0.0f32..24.0), 2..5),
            px_range in 2.0f32..8.0,
            samples in proptest::collection::vec((0.0f32..24.0, 0.0f32..24.0), 64),
        ) {
            let glyph = DistanceImage { width: 24, height: 24, px_range, texels: channels };
            let pts: Vec<Point> = sites.iter().map(|&(x, y)| Point { x, y }).collect();
            let Ok(cut) = VoronoiCut::new(pts, min_overlap(px_range), px_range) else {
                return Ok(()); // coincident random sites: not a valid cut
            };
            let frags: Vec<_> = (0..cut.sites().len()).map(|k| bake_fragment(&glyph, &cut, k)).collect();
            for (x, y) in samples {
                let p = Point { x, y };
                let expected = median3(glyph.bilinear(p));
                let got = union_of_fragments(&frags, p);
                prop_assert!((got - expected).abs() <= 1e-6, "p=({x},{y}) expected {expected} got {got}");
            }
        }

        /// The median/min commutation that makes per-channel baking valid for MSDF.
        /// Exact float equality is the claim being tested.
        #[test]
        #[allow(clippy::float_cmp)]
        fn median_commutes_with_common_min(a in 0.0f32..1.0, b in 0.0f32..1.0, c in 0.0f32..1.0, r in 0.0f32..1.0) {
            prop_assert_eq!(median3([a.min(r), b.min(r), c.min(r)]), median3([a, b, c]).min(r));
        }
    }
}
