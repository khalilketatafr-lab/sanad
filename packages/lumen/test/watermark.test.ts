/**
 * Ex Libris session watermark (src/vault/watermark.ts): the attribution math
 * that makes a leaked screenshot dangerous. A leak is simulated as the true
 * session's keyed carrier signs, then degraded (bit flips = noise, zeros =
 * cropped/unreadable carriers); the forensic side re-derives suspects' signs
 * and accuses the match, with a false-accusation probability. Proves a clean
 * and a noisy leak attribute correctly, an innocent session is not accused, a
 * wrong key implicates no one, a two-session collusion still implicates a
 * colluder, and pooling pages turns inconclusive pages into a confident call.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DEFAULT_WATERMARK,
  accuse,
  accuseMultiPage,
  carrierMods,
  correlate,
  markSignature,
} from "../src/vault/watermark.ts";

const K_WM = new Uint8Array(32).fill(0x9e);
const WRONG_KEY = new Uint8Array(32).fill(0x11);

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rng = mulberry32(0xca7a);
const randomId = (): bigint => (BigInt(Math.floor(rng() * 2 ** 32)) << 32n) | BigInt(Math.floor(rng() * 2 ** 32));

/** A field of innocent suspects plus the real ones, shuffled in. */
function suspects(count: number, ...real: bigint[]): bigint[] {
  const set = new Set(real);
  while (set.size < count) set.add(randomId());
  return [...set];
}

/**
 * Degrades a true signature into an observed leak. A carrier is erased with
 * probability `erase` (cropped / unmeasurable); otherwise its sign is flipped
 * with probability `flip` (measurement noise). The two are independent, so the
 * surviving carriers match with probability `1 - flip`.
 */
function leak(signature: Int8Array, flip: number, erase: number): Int8Array {
  const out = Int8Array.from(signature);
  for (let i = 0; i < out.length; i++) {
    if (rng() < erase) out[i] = 0;
    else if (rng() < flip) out[i] = -out[i]!;
  }
  return out;
}

test("a clean leak attributes to the exact session with a tiny false-accusation probability", async () => {
  const A = randomId();
  const observed = await markSignature(K_WM, A, 7);
  const { accused, ranked } = await accuse(K_WM, 7, observed, suspects(200, A));
  assert.ok(accused, "an accusation was made");
  assert.equal(accused.sessionId, A);
  assert.equal(accused.matches, accused.compared, "every carrier matched");
  assert.ok(accused.pValue < 1e-12, `p=${accused.pValue}`);
  // An innocent suspect sits near chance (~half the carriers match).
  const innocent = ranked.find((r) => r.sessionId !== A)!;
  assert.ok(innocent.matches / innocent.compared < 0.7, "innocent near chance");
});

test("a noisy single-screenshot leak (10% flips, 15% erasures) still attributes correctly", async () => {
  const A = randomId();
  const sig = await markSignature(K_WM, A, 3);
  const observed = leak(sig, 0.1, 0.15);
  const { accused } = await accuse(K_WM, 3, observed, suspects(200, A));
  assert.ok(accused && accused.sessionId === A, "attributed despite noise");
  assert.ok(accused.pValue <= 1e-6, `p=${accused.pValue}`);
  assert.ok(accused.compared < sig.length, "some carriers were erased");
});

test("an innocent session is never accused when the real leaker is absent", async () => {
  const leaker = randomId();
  const observed = leak(await markSignature(K_WM, leaker, 1), 0.2, 0.1);
  // The candidate pool does NOT include the real leaker.
  const { accused, ranked } = await accuse(K_WM, 1, observed, suspects(200));
  assert.equal(accused, null, "no false accusation against innocents");
  // Best innocent is near chance, nowhere near significance after correction.
  assert.ok(ranked[0]!.matches / ranked[0]!.compared < 0.72);
});

test("the wrong watermark key implicates no one", async () => {
  const A = randomId();
  const observed = await markSignature(K_WM, A, 5);
  const { accused } = await accuse(WRONG_KEY, 5, observed, suspects(200, A));
  assert.equal(accused, null, "a different K_wm cannot attribute the leak");
});

test("a two-session collusion is traced to a colluder across an edition, not to an innocent", async () => {
  const A = randomId();
  const C = randomId();
  // Marking-assumption pirate, per page: keep carriers where the colluders
  // agree; where they differ, output either (they cannot know the safe way).
  const leaves = [];
  for (let page = 0; page < 6; page++) {
    const sA = await markSignature(K_WM, A, page);
    const sC = await markSignature(K_WM, C, page);
    leaves.push({ page, observed: Int8Array.from(sA, (v, i) => (v === sC[i] ? v : rng() < 0.5 ? v : sC[i]!)) });
  }
  const { accused, ranked } = await accuseMultiPage(K_WM, leaves, suspects(200, A, C));
  assert.ok(accused, "the coalition is traced");
  assert.ok(accused.sessionId === A || accused.sessionId === C, "an actual colluder is accused");
  assert.ok(accused.pValue <= 1e-6, `p=${accused.pValue}`);
  const innocentBest = ranked.find((r) => r.sessionId !== A && r.sessionId !== C)!;
  assert.ok(accused.matches / accused.compared > innocentBest.matches / innocentBest.compared, "colluder beats innocents");
});

test("pooling pages turns inconclusive single pages into a confident attribution", async () => {
  const A = randomId();
  const pool = suspects(150, A);
  // One heavily degraded page is not enough on its own…
  const onePage = leak(await markSignature(K_WM, A, 0), 0.38, 0.4);
  const single = await accuse(K_WM, 0, onePage, pool);
  // …but the same quality across many pages pools into significance.
  const leaves = [];
  for (let page = 0; page < 14; page++) {
    leaves.push({ page, observed: leak(await markSignature(K_WM, A, page), 0.38, 0.4) });
  }
  const many = await accuseMultiPage(K_WM, leaves, pool);
  assert.ok(many.accused && many.accused.sessionId === A, "pooled attribution succeeds");
  assert.ok(many.accused.pValue <= single.ranked[0]!.pValue, "more pages never weaken the case");
  assert.ok(many.accused.pValue <= 1e-6, `pooled p=${many.accused.pValue}`);
});

test("signature is deterministic, keyed, and carrier mods cover both channels", async () => {
  const A = 0x0123456789abcdefn;
  const a1 = await markSignature(K_WM, A, 2);
  const a2 = await markSignature(K_WM, A, 2);
  assert.deepEqual([...a1], [...a2], "same inputs → same signs");
  const other = await markSignature(K_WM, A, 3);
  assert.notDeepEqual([...a1], [...other], "a different page → different signs");
  assert.equal(a1.length, DEFAULT_WATERMARK.carriersPerPage * 2);
  for (const v of a1) assert.ok(v === 1 || v === -1, "signs are ±1");

  const mods = carrierMods(a1);
  assert.equal(mods.length, DEFAULT_WATERMARK.carriersPerPage * 2);
  assert.ok(mods.some((m) => m.channel === "glue") && mods.some((m) => m.channel === "baseline"));

  // A perfect self-correlation is maximal and significant.
  const self = correlate(a1, a1);
  assert.equal(self.matches, self.compared);
  assert.ok(self.pValue < 1e-9);
});
