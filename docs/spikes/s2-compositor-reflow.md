# Spike S2: Compositor on real text, and reflow speed

**Question (gate G0):** can the Compositor (Knuth–Plass, kashida-first
Arabic justification, UAX #9 L2) reflow print-grade Latin *and* Arabic fast
enough on a mid-range phone: visible spread ≤ 16 ms on tier B?

**Answer so far:** yes, with a large margin on emulated tier B. Real-device
numbers are still owed (see *Limits*).

## Fixture

`fixtures/typeset`, shaped by Atelier (rustybuzz, UAX #9, Knuth–Liang
hyphenation, kashida opportunities from Arabic joining), permuted, then laid
out by the Compositor:

- **Latin:** the 10 longest paragraphs of *Pride and Prejudice*, chapter I
  (Literata, 36 device px).
- **Arabic:** the 10 paragraphs of *Kalīla wa-Dimna*, "باب الحمامة المطوقة"
  (Noto Naskh Arabic, 42 device px).
- **Total:** 20 paragraphs and 12 014 glyphs. That is 149–290 lines across
  five measures (288–560 CSS px at DPR 2), roughly eight phone pages.

A *reflow* lays out all 20 paragraphs at one measure. That is what happens
when the reader changes the size, spacing or orientation.

## Results

Native (x86-64, release): `cargo run --release -p sanad-atelier --example reflow_bench`

| Reflow (20 paragraphs) | p50 | p95 | Per line | 50-line spread |
|---|---|---|---|---|
| Native | 2.0 ms | 3.0 ms | 8.7 µs | 0.43 ms |

WASM in headless Chromium, the shipped `@sanad/lumen-wasm` build:
`pnpm --filter @sanad/lumen bench:reflow`

| CPU | Full reflow p50 / p95 | Per line | 50-line spread p95 |
|---|---|---|---|
| 1× (dev machine) | 2.2 / 3.2 ms | 9.6 µs | 0.70 ms |
| **4× (≈ tier B, mid-range Android)** | 9.8 / 12.8 ms | 43 µs | **2.8 ms** |
| 6× (≈ low-end Android) | 14.3 / 19.7 ms | 63 µs | 4.3 ms |

WASM runs within ~10% of native. At 4× throttling, a two-page spread reflows
in under a fifth of the budget, and even the full eight-page fixture fits
in one frame.

**Parity:** `packages/lumen/test/compositor-wasm.test.ts` lays the fixture out
in WASM at all five measures. The glyph ids, positions, kashida scales and
kinds hash to exactly the native digest (FNV-1a over the f32 bits), so the
client and Atelier's golden pages lay out identically.

## Typographic observations (for the G0 review)

- **Arabic:** kashida-first justification spends elongation before word
  space. On the golden page (17 lines) it inserted 121 kashidas, at most one
  per word. The highest-priority joints are used: after Seen/Sad, and before
  final Teh Marbuta, Heh and Dal. Lines look even, but some lines elongate
  nearly every word. A typographer may want a per-line kashida cap or a
  higher `kashida_weight`. Both are `BreakParams` and need no code.
- **Latin:** at a 408-CSS-px measure, paragraph 2 sets loosely because
  Knuth–Plass declined the available hyphenation points ("neighbourhood").
  `hyphen_penalty` (50) and `tolerance` (2.0) are the tuning knobs.

## Limits

- CPU throttling scales main-thread speed. It does not model a phone's
  memory bandwidth, thermal throttling or big.LITTLE scheduling. G0 still
  requires timings on the reference low-end Android: run the same bench page
  on the device through remote debugging.
- Times include item building, all three Knuth–Plass passes when needed,
  justification and L2. They exclude chunk decryption and FlatBuffer access,
  which happen once per chunk, not per reflow.
