/**
 * Ex Libris — session micro-typography watermark (blueprint 05 §4).
 *
 * Every reading session carries a 64-bit id. On each page, a keyed PRF
 * (HMAC-SHA256 under the watermark key K_wm) turns (sessionId, page) into a
 * deterministic vector of ±1 *carrier signs*. The client nudges ~40 carriers
 * per page by that sign — inter-word glue by ±0.6 % and the baseline by
 * ±0.08 em (05 §4.2) — far below the just-noticeable difference, since
 * justified text already varies inter-word space by 10–30 % line to line.
 *
 * Attribution is *detection*, not blind decoding (05 §4.3, §4.5): the carrier
 * signs depend on the session id, so a forensic analyst re-derives a *suspect's*
 * expected signs and correlates them with the signs measured from a leak. A
 * genuine match lines up on nearly every carrier; an innocent session matches
 * about half (the PRF is pseudorandom to anyone without that session's seed).
 * The number of matches is Binomial(n, ½) under the innocent null, so every
 * accusation ships with a **false-accusation probability** — the chance a
 * random session would match this well — exactly as §4.5 step ④ requires.
 *
 * This module is the shared codec and the `leak-drill` core (05 §9): the client
 * applies [`markSignature`]; a forensic tool measures carriers from a leak and
 * calls [`accuse`]. Applying the nudges inside the Compositor (glue during line
 * breaking, baseline at emit) is the follow-up that needs the wasm carrier
 * binding; the attribution math that makes a leak dangerous is here and proven.
 */

/** Carrier channels a carrier word can be nudged on (05 §4.2). */
export type Channel = "glue" | "baseline";

export interface WatermarkParams {
  /** Carriers modulated per page (≈ 40 in the blueprint). */
  readonly carriersPerPage: number;
  /** Inter-word glue delta, fraction of the line's natural space. */
  readonly glueDelta: number;
  /** Baseline shift on a carrier word start, in ems. */
  readonly baselineDelta: number;
}

export const DEFAULT_WATERMARK: WatermarkParams = {
  carriersPerPage: 48,
  glueDelta: 0.006,
  baselineDelta: 0.08,
};

/** A single carrier's modulation: which carrier, which channel, which way. */
export interface CarrierMod {
  readonly carrier: number;
  readonly channel: Channel;
  readonly sign: -1 | 1;
}

/** The result of correlating a leak against one suspect session. */
export interface Attribution {
  readonly sessionId: bigint;
  /** Carriers that agreed with the suspect's expected signs. */
  readonly matches: number;
  /** Carriers actually compared (erased carriers are skipped). */
  readonly compared: number;
  /** Standard normal score of the match count under the innocent null. */
  readonly z: number;
  /**
   * The chance a random, innocent session would match at least this well —
   * the false-accusation probability for this single comparison, before any
   * multiple-comparison correction.
   */
  readonly pValue: number;
}

const te = new TextEncoder();

function sessionBytes(sessionId: bigint, page: number): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(12);
  const view = new DataView(out.buffer);
  view.setBigUint64(0, BigInt.asUintN(64, sessionId), false);
  view.setUint32(8, page >>> 0, false);
  return out;
}

async function prfKey(kWm: Uint8Array): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", kWm.slice(), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
}

/**
 * The deterministic ±1 carrier signs for a session on a page. Index `2k` is
 * carrier `k`'s glue channel, `2k+1` its baseline channel, so a page yields
 * `2·carriersPerPage` signs. Pure PRF output: identical inputs give identical
 * signs, and a different session (or key, or page) gives pseudorandom signs.
 */
export async function markSignature(
  kWm: Uint8Array,
  sessionId: bigint,
  page: number,
  params: WatermarkParams = DEFAULT_WATERMARK,
): Promise<Int8Array> {
  const need = params.carriersPerPage * 2;
  const key = await prfKey(kWm);
  const base = sessionBytes(sessionId, page);
  const signs = new Int8Array(need);
  let produced = 0;
  for (let block = 0; produced < need; block++) {
    const msg = new Uint8Array(base.length + 4);
    msg.set(base, 0);
    new DataView(msg.buffer).setUint32(base.length, block, false);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, msg));
    for (let i = 0; i < mac.length && produced < need; i++) {
      for (let bit = 0; bit < 8 && produced < need; bit++) {
        signs[produced++] = ((mac[i]! >> bit) & 1) === 1 ? 1 : -1;
      }
    }
  }
  return signs;
}

/** Turns a signature into the per-carrier, per-channel modulations to apply. */
export function carrierMods(signature: Int8Array): CarrierMod[] {
  const mods: CarrierMod[] = [];
  for (let k = 0; k * 2 + 1 < signature.length; k++) {
    mods.push({ carrier: k, channel: "glue", sign: signature[k * 2] === 1 ? 1 : -1 });
    mods.push({ carrier: k, channel: "baseline", sign: signature[k * 2 + 1] === 1 ? 1 : -1 });
  }
  return mods;
}

/** √2·erfc argument helper: the standard-normal upper tail Q(z) = ½·erfc(z/√2). */
function normalUpperTail(z: number): number {
  // Abramowitz & Stegun 7.1.26 for erfc, good to ~1e-7.
  const x = z / Math.SQRT2;
  const t = 1 / (1 + 0.3275911 * Math.abs(x));
  const y =
    t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
  const erfcAbs = y * Math.exp(-x * x);
  const erfc = x >= 0 ? erfcAbs : 2 - erfcAbs;
  return 0.5 * erfc;
}

/**
 * Correlates measured carrier signs against a suspect session's expected signs.
 * `observed[i]` is +1/-1 for a carrier read from the leak, or 0 if that carrier
 * could not be measured (cropped, too noisy). Returns the match count and the
 * false-accusation probability under the innocent null (matches ~ Binomial(n,½),
 * normal-approximated with a continuity correction).
 */
export function correlate(observed: Int8Array, expected: Int8Array): Omit<Attribution, "sessionId"> {
  const n = Math.min(observed.length, expected.length);
  let compared = 0;
  let matches = 0;
  for (let i = 0; i < n; i++) {
    if (observed[i] === 0) continue;
    compared++;
    if (observed[i] === expected[i]) matches++;
  }
  if (compared === 0) return { matches: 0, compared: 0, z: 0, pValue: 1 };
  // Under H0, matches ~ Binomial(compared, 1/2). Continuity-corrected z.
  const mean = compared / 2;
  const sd = Math.sqrt(compared) / 2;
  const z = sd === 0 ? 0 : (matches - 0.5 - mean) / sd;
  return { matches, compared, z, pValue: normalUpperTail(z) };
}

export interface AccuseOptions {
  /** Max tolerated false-accusation probability after correction (ε). */
  readonly epsilon?: number;
  readonly params?: WatermarkParams;
}

/**
 * Attributes a single-page leak to one of `candidates`, or to no one. Correlates
 * the leak's measured carrier signs against each candidate's expected signs and
 * accuses the best match only if its false-accusation probability, after a
 * Bonferroni correction for the number of candidates tried, is at most ε
 * (default 1e-6, matching 05 §4.3). Returns the full ranking for the report.
 */
export async function accuse(
  kWm: Uint8Array,
  page: number,
  observed: Int8Array,
  candidates: readonly bigint[],
  opts: AccuseOptions = {},
): Promise<{ accused: Attribution | null; ranked: Attribution[] }> {
  const epsilon = opts.epsilon ?? 1e-6;
  const params = opts.params ?? DEFAULT_WATERMARK;
  const ranked: Attribution[] = [];
  for (const sessionId of candidates) {
    const expected = await markSignature(kWm, sessionId, page, params);
    ranked.push({ sessionId, ...correlate(observed, expected) });
  }
  ranked.sort((a, b) => a.pValue - b.pValue || b.matches - a.matches);
  const best = ranked[0];
  const corrected = best === undefined ? 1 : Math.min(1, best.pValue * Math.max(1, candidates.length));
  const accused = best !== undefined && corrected <= epsilon ? best : null;
  return { accused, ranked };
}

/**
 * Attributes a multi-page leak, pooling evidence across pages (05 §4.1: "every
 * page carries the full id"). Matches and comparisons are summed across pages
 * before the significance test, so a few noisy pages that are each inconclusive
 * combine into a confident, low-probability attribution.
 */
export async function accuseMultiPage(
  kWm: Uint8Array,
  leaves: readonly { readonly page: number; readonly observed: Int8Array }[],
  candidates: readonly bigint[],
  opts: AccuseOptions = {},
): Promise<{ accused: Attribution | null; ranked: Attribution[] }> {
  const epsilon = opts.epsilon ?? 1e-6;
  const params = opts.params ?? DEFAULT_WATERMARK;
  const ranked: Attribution[] = [];
  for (const sessionId of candidates) {
    let matches = 0;
    let compared = 0;
    for (const leaf of leaves) {
      const expected = await markSignature(kWm, sessionId, leaf.page, params);
      const c = correlate(leaf.observed, expected);
      matches += c.matches;
      compared += c.compared;
    }
    const mean = compared / 2;
    const sd = Math.sqrt(compared) / 2;
    const z = sd === 0 ? 0 : (matches - 0.5 - mean) / sd;
    ranked.push({ sessionId, matches, compared, z, pValue: normalUpperTail(z) });
  }
  ranked.sort((a, b) => a.pValue - b.pValue || b.matches - a.matches);
  const best = ranked[0];
  const corrected = best === undefined ? 1 : Math.min(1, best.pValue * Math.max(1, candidates.length));
  const accused = best !== undefined && corrected <= epsilon ? best : null;
  return { accused, ranked };
}
