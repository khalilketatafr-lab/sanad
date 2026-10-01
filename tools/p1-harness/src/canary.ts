/**
 * Canary loading and needle generation.
 *
 * A "needle" is one concrete representation of a canary that could appear on
 * the client if P1 were violated. Two families:
 *
 *  - Text needles: compared against *normalized* haystacks (NFKC, Arabic
 *    diacritics/tatweel stripped, Arabic letter variants folded, punctuation
 *    and invisible controls removed, whitespace collapsed, lower-cased). Used
 *    for semantic layers: DOM, accessibility tree, storage, console, decoded
 *    static assets, network text bodies, heap-snapshot strings.
 *  - Byte needles: exact byte sequences (UTF-8, UTF-16LE, base64) for raw
 *    layers: process memory, binary assets, binary network bodies.
 *
 * Every phrase also yields k-word shingles so partial leaks (a single line,
 * a search snippet) are caught, not just whole-phrase copies.
 */
import { readFile } from "node:fs/promises";

export interface CanaryPhrase {
  readonly id: string;
  readonly lang: string;
  readonly text: string;
}

export interface CanaryFile {
  readonly version: 1;
  readonly shingleWords: number;
  readonly minShingleChars: number;
  readonly phrases: readonly CanaryPhrase[];
}

export type NeedleKind = "phrase" | "shingle";
export type ByteEncoding = "utf8" | "utf16le" | "latin1" | "base64" | "base64url";

export interface TextNeedle {
  readonly canaryId: string;
  readonly kind: NeedleKind;
  readonly normalized: string;
}

export interface ByteNeedle {
  readonly canaryId: string;
  readonly kind: NeedleKind;
  /** Human label: which textual form and encoding produced these bytes. */
  readonly label: string;
  readonly encoding: ByteEncoding;
  readonly bytes: Uint8Array;
}

export interface NeedleSet {
  readonly canaries: CanaryFile;
  readonly text: readonly TextNeedle[];
  /** Raw-memory needles: UTF-8 and UTF-16LE forms. */
  readonly bytes: readonly ByteNeedle[];
  /** Asset/network needles: byte needles plus base64 forms. */
  readonly assetBytes: readonly ByteNeedle[];
}

// Arabic harakat, Quranic marks, superscript alef, tatweel.
const ARABIC_MARKS = /[ؐ-ًؚ-ٰٟۖ-ۜ۟-۪ۨ-ۭـ]/gu;
// Zero-width, soft hyphen, bidi controls/isolates, BOM, Arabic letter mark.
const INVISIBLES = /[­͏؜ᅟᅠ឴឵᠎​-‏‪-‮⁠-⁤⁦-⁯﻿]/gu;
const ARABIC_FOLDS: ReadonlyArray<readonly [RegExp, string]> = [
  [/[آأإٱ]/gu, "ا"], // آ أ إ ٱ → ا
  [/ى/gu, "ي"], // ى → ي
  [/ة/gu, "ه"], // ة → ه
  [/ؤ/gu, "و"], // ؤ → و
  [/ئ/gu, "ي"], // ئ → ي
];
const PUNCT_SYMBOL = /[\p{P}\p{S}]+/gu;
const SPACE = /\s+/gu;

/**
 * Canonical comparison form. Applied identically to needles and haystacks, so
 * a canary is found however it was re-encoded, re-shaped (presentation forms
 * fold under NFKC), stripped of diacritics, or split by invisible characters.
 */
export function normalizeText(input: string): string {
  let s = input.normalize("NFKC");
  s = s.replace(INVISIBLES, "");
  s = s.replace(ARABIC_MARKS, "");
  for (const [re, to] of ARABIC_FOLDS) s = s.replace(re, to);
  s = s.replace(PUNCT_SYMBOL, " ");
  s = s.replace(SPACE, " ").trim();
  return s.toLowerCase();
}

function words(s: string): string[] {
  return s.split(" ").filter((w) => w.length > 0);
}

function shingles(ws: readonly string[], k: number, minChars: number): string[] {
  const out: string[] = [];
  for (let i = 0; i + k <= ws.length; i++) {
    const sh = ws.slice(i, i + k).join(" ");
    if (sh.length >= minChars) out.push(sh);
  }
  return out;
}

/** Raw textual forms a leaked string could take in memory or in an asset. */
function rawForms(text: string): Map<string, string> {
  const forms = new Map<string, string>();
  const add = (label: string, value: string): void => {
    if (![...forms.values()].includes(value)) forms.set(label, value);
  };
  const nfc = text.normalize("NFC");
  add("nfc", nfc);
  add("nfd", text.normalize("NFD"));
  const stripped = nfc.replace(ARABIC_MARKS, "");
  add("nfc-stripped", stripped);
  const ascii = (s: string): string => s.replace(/[‘’ʼ]/gu, "'");
  add("nfc-ascii-quotes", ascii(nfc));
  add("nfd-ascii-quotes", ascii(text.normalize("NFD")));
  return forms;
}

const encoder = new TextEncoder();

/**
 * V8 ("one-byte" strings) and Blink (8-bit WTF::String) store any string whose
 * code units are all ≤ U+00FF as Latin-1, one byte per char. For text with
 * non-ASCII Latin-1 characters (é, à …) that differs from UTF-8, so it needs
 * its own needle. Returns undefined when Latin-1 is impossible or equals UTF-8.
 */
function latin1(s: string): Uint8Array | undefined {
  let nonAscii = false;
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) {
    const cu = s.charCodeAt(i);
    if (cu > 0xff) return undefined;
    if (cu > 0x7f) nonAscii = true;
    out[i] = cu;
  }
  return nonAscii ? out : undefined;
}

function utf16le(s: string): Uint8Array {
  const out = new Uint8Array(s.length * 2);
  for (let i = 0; i < s.length; i++) {
    const cu = s.charCodeAt(i);
    out[i * 2] = cu & 0xff;
    out[i * 2 + 1] = cu >>> 8;
  }
  return out;
}

/**
 * Base64 of `data` as it would appear embedded at any of the three byte
 * alignments inside a larger base64 blob. Only fully-determined characters
 * are kept.
 */
export function base64Alignments(data: Uint8Array): string[] {
  const out: string[] = [];
  for (let o = 0; o < 3; o++) {
    const padded = new Uint8Array(o + data.length);
    padded.set(data, o);
    const b64 = Buffer.from(padded).toString("base64");
    const first = Math.ceil((8 * o) / 6);
    const last = Math.floor((8 * (o + data.length)) / 6); // exclusive
    if (last - first >= 16) out.push(b64.slice(first, last));
  }
  return out;
}

function dedupeBytes(needles: ByteNeedle[]): ByteNeedle[] {
  const seen = new Set<string>();
  return needles.filter((n) => {
    const key = Buffer.from(n.bytes).toString("hex");
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function buildNeedles(canaries: CanaryFile): NeedleSet {
  const text: TextNeedle[] = [];
  const bytes: ByteNeedle[] = [];
  const b64: ByteNeedle[] = [];
  const textSeen = new Set<string>();

  for (const phrase of canaries.phrases) {
    const norm = normalizeText(phrase.text);
    const normWords = words(norm);
    const candidates: Array<readonly [NeedleKind, string]> = [
      ["phrase", norm],
      ...shingles(normWords, canaries.shingleWords, canaries.minShingleChars).map(
        (s) => ["shingle", s] as const,
      ),
    ];
    for (const [kind, normalized] of candidates) {
      if (textSeen.has(normalized)) continue;
      textSeen.add(normalized);
      text.push({ canaryId: phrase.id, kind, normalized });
    }

    for (const [formLabel, form] of rawForms(phrase.text)) {
      const raw: Array<readonly [NeedleKind, string]> = [
        ["phrase", form],
        ...shingles(form.split(/\s+/u), canaries.shingleWords, canaries.minShingleChars).map(
          (s) => ["shingle", s] as const,
        ),
      ];
      for (const [kind, value] of raw) {
        const u8 = encoder.encode(value);
        bytes.push({ canaryId: phrase.id, kind, label: `${formLabel}/utf8`, encoding: "utf8", bytes: u8 });
        // Pure-ASCII strings are stored one byte per char by V8 and Blink, so the
        // UTF-16 form only matters for strings containing non-Latin-1 characters,
        // but it is cheap and catches UTF-16 serializations everywhere.
        bytes.push({
          canaryId: phrase.id,
          kind,
          label: `${formLabel}/utf16le`,
          encoding: "utf16le",
          bytes: utf16le(value),
        });
        const l1 = latin1(value);
        if (l1 !== undefined) {
          bytes.push({ canaryId: phrase.id, kind, label: `${formLabel}/latin1`, encoding: "latin1", bytes: l1 });
        }
        if (kind === "phrase") {
          for (const s of base64Alignments(u8)) {
            b64.push({ canaryId: phrase.id, kind, label: `${formLabel}/base64`, encoding: "base64", bytes: encoder.encode(s) });
            const url = s.replace(/\+/gu, "-").replace(/\//gu, "_");
            if (url !== s) {
              b64.push({ canaryId: phrase.id, kind, label: `${formLabel}/base64url`, encoding: "base64url", bytes: encoder.encode(url) });
            }
          }
        }
      }
    }
  }

  const memNeedles = dedupeBytes(bytes);
  return { canaries, text, bytes: memNeedles, assetBytes: dedupeBytes([...memNeedles, ...b64]) };
}

function isCanaryFile(v: unknown): v is CanaryFile {
  if (typeof v !== "object" || v === null) return false;
  const o = v as Record<string, unknown>;
  return (
    o["version"] === 1 &&
    typeof o["shingleWords"] === "number" &&
    typeof o["minShingleChars"] === "number" &&
    Array.isArray(o["phrases"]) &&
    o["phrases"].every(
      (p: unknown) =>
        typeof p === "object" &&
        p !== null &&
        typeof (p as Record<string, unknown>)["id"] === "string" &&
        typeof (p as Record<string, unknown>)["text"] === "string",
    )
  );
}

export async function loadCanaries(path: string): Promise<CanaryFile> {
  const parsed: unknown = JSON.parse(await readFile(path, "utf8"));
  if (!isCanaryFile(parsed)) throw new Error(`invalid canary file: ${path}`);
  if (parsed.phrases.length === 0) throw new Error("canary file has no phrases");
  if (parsed.shingleWords < 3) throw new Error("shingleWords < 3 would cause false positives");
  return parsed;
}
