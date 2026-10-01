# Spike S1: glyph shredding and the MAX-union

**Status:** proven numerically (Rust property tests) and on a real WebGL2
rasterizer (`packages/lumen/test/webgl.test.ts`).

**Code:** `crates/atelier/src/shred.rs` (bake) · `packages/lumen/src/gl/shaders.ts`
(Pass 1 and Pass 3) · `packages/lumen/src/gl/renderer.ts`.

---

## 1. Goal

No atlas slot may be a recognizable character. Each glyph's multi-channel
signed distance field (MSDF) is split into k ≥ 2 fragments stored in unrelated
slots. The renderer reassembles them on the GPU with **zero visible
difference**, at any zoom, in any theme.

## 2. Notation

- Normalized MSDF value per channel `c`: `s_c(p) = d_c(p)/ρ + ½`, where `d` is
  the signed distance in atlas texels (positive inside) and `ρ` is the
  distance range (`pxRange`).
- The decoded distance is the median of the three channels:
  `sd(p) = median(s_r, s_g, s_b)`.
- Glyph coverage is a monotone function of `sd`. Pass 3 applies it after
  scaling to screen pixels.

## 3. Cutting: a dilated Voronoi partition

Sites `q_1 … q_k` are placed deterministically from a secret per-(edition,
glyph) seed, inside the glyph's bounds, with a minimum pairwise separation.
They induce convex Voronoi cells `C_k`. The exact signed distance from `p` to
the boundary of `C_k` (positive inside) is the minimum over its bisectors:

```text
δ_k(p) = min_{j≠k}  ( |p − q_j|² − |p − q_k|² ) / ( 2 |q_j − q_k| )
```

Each cell is dilated by an overlap `ω` and encoded like the glyph:

```text
r_k(p) = clamp( (δ_k(p) + ω)/ρ + ½ , 0, 1 )
```

Fragment `k` is baked per channel as `f_{k,c}(p) = min( s_c(p), r_k(p) )` and
packed into its own slot, cropped to its non-empty support.

## 4. Why the GPU's MAX blend reassembles the glyph exactly

1. **The median commutes with a common min.** `x ↦ min(x, r)` is monotone,
   and a monotone map applied to all three values commutes with the median:
   `median(min(a,r), min(b,r), min(c,r)) = min(median(a,b,c), r)`. So
   fragment `k` decodes to `min(sd, r_k)`. Per-channel baking is valid for
   MSDF, not only for single-channel SDFs.
2. **Every point is saturated by its own cell.** Each `p` lies in some cell,
   so `δ_k(p) ≥ 0` there. With `ω ≥ ρ/2`, `r_k(p) = 1 ≥ sd(p)`, so that
   fragment equals the glyph at `p`.
3. **MAX distributes over min.**
   `max_k min(sd, r_k) = min(sd, max_k r_k) = sd`. Pass 1 writes a monotone
   encoding of each fragment's distance and blends with `MAX`, so the target
   holds exactly the unshredded glyph's distance.
4. **Bilinear filtering.** The GPU samples 4 texels within `√2` texels of the
   sample point, so `ω ≥ ρ/2 + √2` keeps all four saturated in the sample's
   own cell. Atelier enforces `ω ≥ ρ/2 + 2` (`min_overlap`) and rejects
   anything smaller.

The result is an identity, not an approximation.

### Why not alpha or additive blending?

Cells overlap by design (`ω > 0`). Where a cut crosses the outline, two
fragments both have partial coverage `a`, `b`. Source-over gives
`1 − (1 − a)(1 − b) > max(a, b)`, and additive gives `a + b`. Both draw a
darker seam exactly where the cut meets an edge, which is the most visible
place.

## 5. Why Pass 1 stores distance, not coverage

Pass 1 writes `enc = clamp(sd_px / 4 + ½, 0, 1)`: the screen-space signed
distance over a ±2 px window, at 1/64 px resolution in RGBA8. Coverage is
monotone in distance, so MAX over distances is the same union. Keeping
distance lets Pass 3 move the edge:

```text
coverage = clamp( (enc − ½)·4 + u_weightPx + ½ , 0, 1 ) ^ u_covGamma
```

This puts **optical weight compensation** (`u_weightPx`: lighter strokes for
light-on-dark text) in the composite pass. It is a per-theme uniform, so a
theme switch never re-renders Pass 1. Because the offset is in screen pixels,
it fades naturally at large zoom, where irradiation no longer matters.

## 6. Measurements

### Rust property tests (`cargo test -p sanad-atelier`)

- `max_union_of_fragments_is_exact`: random MSDF channels (not only true
  SDFs), 2–4 random sites, `ρ ∈ [2, 8)`, 64 random sub-texel sample points.
  Union equals glyph within 1e-6.
- **Mutation check:** lowering the bound to `ω = ρ/4` makes the property test
  and the exhaustive demo-glyph test fail with concrete counterexamples. The
  bound is load-bearing.

### Real WebGL2 (`pnpm --filter @sanad/lumen test`, Chromium + SwiftShader)

The atlas comes from the production shredder (`shred_fixture`): one whole
glyph (comparison only) plus 3 fragments. Ink shares are 31%, 51% and 31%.
No fragment is more than 80% of the glyph.

| Check | 2× magnification | 0.5× minification |
|---|---|---|
| MAX-union vs whole glyph, max channel diff | **0 / 255 (bit-identical)** | **0 / 255 (bit-identical)** |
| Additive union vs whole glyph | 222 / 255 (seams) | 222 / 255 (seams) |
| One fragment alone vs whole glyph | differs on 13 301 px | differs on 902 px |

Optical weight (ink mass at 0.5×): −0.4 px gives 563 587, 0 gives 584 873,
+0.4 px gives 607 294, which is about ±4% and monotone. Anti-glare: white
paper with `lumaCeil = 0.45` came out at Y = 0.4508.

### Atlas layout finding

The first GPU run used a 384-texel-wide atlas (96-texel slots). The union
then differed from the whole glyph by up to 11/255 on 35 px. Copying the
*identical* whole glyph into every slot reproduced the same differences, so
shredding was not the cause. The cause is texture-coordinate rounding:
`1/384` is not exactly representable, so each slot's sample positions round
differently. That happens on any atlas renderer.

With power-of-two pages and power-of-two-aligned slot origins (512-wide,
128-texel slots), the fractional sample positions are identical in every slot
and the difference drops to **0**.

**Production rule:** atlas pages are power-of-two, and slot origins sit on a
power-of-two grid (or fragments go into `TEXTURE_2D_ARRAY` layers at
identical UVs). This is not needed for visual quality: the differences are
sub-perceptual and production atlases never contain the whole glyph to compare
against. It is needed for **determinism**. The Ex Libris forensic decoder
re-renders reference pages and correlates against leaks, and slot-dependent
sampling noise would degrade that signal (blueprint 05 §4.2).

## 7. Security properties and limits

- **Generic atlas OCR fails.** A slot holds a partial shape: a bowl without a
  stem, a stem without a crossbar.
- **The cut pattern is secret and per edition.** The site seed derives from a
  per-edition secret, so a codebook built for one edition does not transfer.
- **Contour-level deduplication.** Separate contours (Arabic dots, i-dots,
  accents) are natural fragments and are deduplicated by a content hash of
  the quantized field, so one slot serves many glyphs. Voronoi cut fragments
  rarely deduplicate.
- **Limit, stated honestly.** The glyph-to-fragments composition table must
  reach the client inside encrypted chunks (read only in WASM memory). A T2
  attacker who extracts it can reassemble and OCR each composite glyph once
  per edition: hours to days of bespoke work, not a generic tool, and the
  resulting text still carries the A/B variant watermark (blueprint 00 §3).
