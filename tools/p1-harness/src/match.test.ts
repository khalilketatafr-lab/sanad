import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { test } from "node:test";
import { base64Alignments, buildNeedles, normalizeText, type ByteNeedle, type CanaryFile } from "./canary.ts";
import { ByteMatcher, decodeEscapes, matchText } from "./match.ts";

const CANARIES: CanaryFile = {
  version: 1,
  shingleWords: 4,
  minShingleChars: 18,
  phrases: [
    { id: "en", lang: "en", text: "Quillwort herons count seven silver lanterns beneath the drowned clocktower" },
    { id: "ar", lang: "ar", text: "يعدُّ مالكُ الحزينِ سبعَ فوانيسَ فضّيةٍ تحتَ برجِ الساعةِ الغارقِ" },
  ],
};

function naive(hay: Uint8Array, needles: readonly ByteNeedle[]): string[] {
  const buf = Buffer.from(hay);
  const out: string[] = [];
  needles.forEach((n, i) => {
    for (let at = buf.indexOf(n.bytes); at !== -1; at = buf.indexOf(n.bytes, at + 1)) out.push(`${i}@${at}`);
  });
  return out.sort();
}

test("Aho–Corasick DFA finds exactly what naive search finds, across chunk splits", () => {
  const needles = buildNeedles(CANARIES).bytes;
  const matcher = new ByteMatcher(needles);
  for (let round = 0; round < 20; round++) {
    const parts: Uint8Array[] = [randomBytes(5000)];
    for (const n of needles.slice(round, round + 3)) parts.push(n.bytes, randomBytes(97));
    const hay = new Uint8Array(Buffer.concat(parts));
    const expected = naive(hay, needles);

    const stream = matcher.stream();
    const got: string[] = [];
    for (let at = 0; at < hay.length; at += 333) {
      for (const h of stream.feed(hay.subarray(at, at + 333))) got.push(`${needles.indexOf(h.needle)}@${h.offset}`);
    }
    assert.deepEqual(got.sort(), expected);
  }
});

test("normalization defeats diacritics, presentation forms, invisibles and case", () => {
  const needles = buildNeedles(CANARIES).text;
  const disguised = "﻿يعد مالك​ الحزين سبع فوانيس فضية تحت برج الساعة الغارق";
  assert.equal(matchText(disguised, needles)[0]?.needle.canaryId, "ar");
  // Arabic presentation forms (as some PDF extractors emit) fold under NFKC.
  assert.equal(normalizeText("ﻣﺮﺣﺒﺎ"), normalizeText("مرحبا"));
  assert.equal(matchText("QUILLWORT HERONS COUNT SEVEN", needles)[0]?.needle.kind, "shingle");
  assert.equal(matchText("a perfectly ordinary sentence about herons", needles).length, 0);
});

test("escape decoding exposes bundler-escaped literals", () => {
  const needles = buildNeedles(CANARIES).text;
  const escaped = [..."الحزين سبع فوانيس فضية"].map((c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`).join("");
  assert.equal(matchText(decodeEscapes(`"${escaped}"`), needles).length, 1);
  assert.equal(matchText(decodeEscapes("count%20seven%20silver%20lanterns"), needles).length, 1);
});

test("base64 alignments are stable substrings of any enclosing encoding", () => {
  const data = new TextEncoder().encode("Quillwort herons count seven silver lanterns");
  for (let prefix = 0; prefix < 6; prefix++) {
    const enclosing = Buffer.concat([randomBytes(prefix), data, randomBytes(4)]).toString("base64");
    assert.ok(base64Alignments(data).some((a) => enclosing.includes(a)), `prefix ${prefix}`);
  }
});
