/**
 * Matching engines.
 *
 * ByteMatcher: Aho–Corasick compiled to a dense DFA (256-way transition table).
 * One linear pass finds every byte needle at once, and the state can be
 * carried across chunk boundaries, so streaming gigabytes of process memory
 * needs no overlap bookkeeping.
 */
import type { ByteNeedle, TextNeedle } from "./canary.ts";
import { normalizeText } from "./canary.ts";

export interface ByteHit {
  readonly needle: ByteNeedle;
  /** Offset of the first byte of the match, relative to the start of the stream. */
  readonly offset: number;
}

export class ByteMatcher {
  readonly needles: readonly ByteNeedle[];
  readonly #delta: Int32Array;
  /** For each state, indices into `needles` that end here (own + failure-chain outputs). */
  readonly #out: ReadonlyArray<readonly number[] | undefined>;

  constructor(needles: readonly ByteNeedle[]) {
    if (needles.length === 0) throw new Error("ByteMatcher: no needles");
    this.needles = needles;

    // 1. Trie.
    const goto: Array<Int32Array> = [new Int32Array(256).fill(-1)];
    const own: number[][] = [[]];
    needles.forEach((n, idx) => {
      if (n.bytes.length === 0) throw new Error("ByteMatcher: empty needle");
      let s = 0;
      for (const b of n.bytes) {
        const row = goto[s];
        if (row === undefined) throw new Error("unreachable");
        let next = row[b] ?? -1;
        if (next === -1) {
          next = goto.length;
          row[b] = next;
          goto.push(new Int32Array(256).fill(-1));
          own.push([]);
        }
        s = next;
      }
      own[s]?.push(idx);
    });

    // 2. Failure links (BFS) folded into a complete DFA.
    const states = goto.length;
    const delta = new Int32Array(states * 256);
    const fail = new Int32Array(states);
    const out: Array<number[] | undefined> = new Array<number[] | undefined>(states);
    const queue: number[] = [];
    const root = goto[0];
    if (root === undefined) throw new Error("unreachable");
    for (let b = 0; b < 256; b++) {
      const t = root[b] ?? -1;
      if (t === -1) {
        delta[b] = 0;
      } else {
        delta[b] = t;
        fail[t] = 0;
        queue.push(t);
      }
    }
    out[0] = undefined;
    for (let head = 0; head < queue.length; head++) {
      const s = queue[head];
      if (s === undefined) break;
      const merged = [...(own[s] ?? []), ...(out[fail[s] ?? 0] ?? [])];
      out[s] = merged.length > 0 ? merged : undefined;
      const row = goto[s];
      if (row === undefined) throw new Error("unreachable");
      for (let b = 0; b < 256; b++) {
        const t = row[b] ?? -1;
        const viaFail = delta[(fail[s] ?? 0) * 256 + b] ?? 0;
        if (t === -1) {
          delta[s * 256 + b] = viaFail;
        } else {
          delta[s * 256 + b] = t;
          fail[t] = viaFail;
          queue.push(t);
        }
      }
    }
    this.#delta = delta;
    this.#out = out;
  }

  /** Stateful scanner for streaming input (e.g. one memory region in chunks). */
  stream(): ByteStream {
    return new ByteStream(this.#delta, this.#out, this.needles);
  }

  /** Convenience: scan one complete buffer. */
  scan(data: Uint8Array): ByteHit[] {
    const s = this.stream();
    return s.feed(data);
  }
}

export class ByteStream {
  #state = 0;
  #consumed = 0;
  readonly #delta: Int32Array;
  readonly #out: ReadonlyArray<readonly number[] | undefined>;
  readonly #needles: readonly ByteNeedle[];

  constructor(
    delta: Int32Array,
    out: ReadonlyArray<readonly number[] | undefined>,
    needles: readonly ByteNeedle[],
  ) {
    this.#delta = delta;
    this.#out = out;
    this.#needles = needles;
  }

  /** Bytes consumed so far (stream-relative offset of the next byte). */
  get position(): number {
    return this.#consumed;
  }

  /** Discontinuity (e.g. unreadable page): restart matching from the root. */
  reset(skipBytes: number): void {
    this.#state = 0;
    this.#consumed += skipBytes;
  }

  feed(data: Uint8Array): ByteHit[] {
    const delta = this.#delta;
    const out = this.#out;
    let s = this.#state;
    let hits: ByteHit[] | undefined;
    for (let i = 0; i < data.length; i++) {
      s = delta[(s << 8) | (data[i] as number)] as number;
      const ends = out[s];
      if (ends !== undefined) {
        hits ??= [];
        for (const idx of ends) {
          const needle = this.#needles[idx];
          if (needle === undefined) continue;
          hits.push({ needle, offset: this.#consumed + i + 1 - needle.bytes.length });
        }
      }
    }
    this.#state = s;
    this.#consumed += data.length;
    return hits ?? [];
  }
}

export interface TextHit {
  readonly needle: TextNeedle;
  readonly excerpt: string;
}

/** Normalizes the haystack once, then tests every text needle. */
export function matchText(haystack: string, needles: readonly TextNeedle[]): TextHit[] {
  if (haystack.length === 0) return [];
  const norm = normalizeText(haystack);
  const hits: TextHit[] = [];
  for (const needle of needles) {
    const at = norm.indexOf(needle.normalized);
    if (at === -1) continue;
    const from = Math.max(0, at - 24);
    const to = Math.min(norm.length, at + needle.normalized.length + 24);
    hits.push({ needle, excerpt: norm.slice(from, to) });
  }
  // A phrase hit implies its shingles; keep the most specific evidence only.
  const phraseCanaries = new Set(hits.filter((h) => h.needle.kind === "phrase").map((h) => h.needle.canaryId));
  return hits.filter((h) => h.needle.kind === "phrase" || !phraseCanaries.has(h.needle.canaryId));
}

const JS_ESCAPES: ReadonlyArray<readonly [RegExp, (m: string, hex: string) => string]> = [
  [/\\u\{([0-9a-fA-F]{1,6})\}/gu, (_m, h) => safeCodePoint(Number.parseInt(h, 16))],
  [/\\u([0-9a-fA-F]{4})/gu, (_m, h) => String.fromCharCode(Number.parseInt(h, 16))],
  [/\\x([0-9a-fA-F]{2})/gu, (_m, h) => String.fromCharCode(Number.parseInt(h, 16))],
  [/&#x([0-9a-fA-F]{1,6});/gu, (_m, h) => safeCodePoint(Number.parseInt(h, 16))],
  [/&#([0-9]{1,7});/gu, (_m, d) => safeCodePoint(Number.parseInt(d, 10))],
];
const NAMED_ENTITIES: Readonly<Record<string, string>> = {
  "&amp;": "&",
  "&lt;": "<",
  "&gt;": ">",
  "&quot;": '"',
  "&apos;": "'",
  "&nbsp;": " ",
};

function safeCodePoint(cp: number): string {
  return Number.isInteger(cp) && cp >= 0 && cp <= 0x10ffff ? String.fromCodePoint(cp) : "";
}

/**
 * Undo the encodings a bundler, serializer or URL could apply to a string
 * literal: JS/JSON escapes, HTML numeric/named entities, percent-encoding.
 */
export function decodeEscapes(text: string): string {
  let s = text;
  for (const [re, fn] of JS_ESCAPES) s = s.replace(re, fn);
  s = s.replace(/&(?:amp|lt|gt|quot|apos|nbsp);/gu, (m) => NAMED_ENTITIES[m] ?? m);
  s = s.replace(/(?:%[0-9a-fA-F]{2})+/gu, (m) => {
    try {
      return decodeURIComponent(m);
    } catch {
      return m;
    }
  });
  return s;
}

/** Decode `bytes` around a hit for human-readable reports. */
export function excerptBytes(buf: Uint8Array, start: number, length: number, encoding: ByteNeedle["encoding"]): string {
  const pad = encoding === "utf16le" ? 48 : 24;
  const from = Math.max(0, start - pad);
  const to = Math.min(buf.length, start + length + pad);
  const slice = buf.subarray(from, to);
  const label = encoding === "utf16le" ? "utf-16le" : encoding === "latin1" ? "latin1" : "utf-8";
  return new TextDecoder(label, { fatal: false }).decode(slice).replace(/[\u0000-\u001f\u007f]/gu, "·");
}
