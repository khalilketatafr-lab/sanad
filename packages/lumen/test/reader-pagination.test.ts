/**
 * Paged layout (blueprint 03 §3.2). The widow/orphan and heading-keep rules are
 * checked as pure logic over synthetic line boxes (`planPages`); the end-to-end
 * paging is checked on the real decoded canary chunk: every fragment of the
 * scroll layout appears exactly once across the pages, a smaller viewport makes
 * more pages, and page boundaries are stable (memoizable).
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { before, test } from "node:test";

import { Compositor, initSync } from "@sanad/lumen-wasm";

import { DEFAULT_STYLE, type PageBox, glyphTableFromJson, layoutChunk, paginateChunk, planPages } from "../src/reader/layout.ts";
import { openFlowChunk } from "../src/vault/flow.ts";
import { type ChunkIdentity, parseChunk } from "../src/vault/folio.ts";

const dir = fileURLToPath(new URL("../../../fixtures/folio/", import.meta.url));
const sealed = new Uint8Array(readFileSync(`${dir}canary-chunk0-v0.folio`));
const meta = JSON.parse(readFileSync(`${dir}canary-chunk0-v0.expected.json`, "utf8")) as {
  editionId: string;
  chunkKeyHex: string;
};
const atlas = glyphTableFromJson(
  JSON.parse(readFileSync(`${dir}canary-glyphs.json`, "utf8")) as Parameters<typeof glyphTableFromJson>[0],
);
const wasmPath = fileURLToPath(import.meta.resolve("@sanad/lumen-wasm/sanad_compositor_bg.wasm"));

before(() => {
  initSync({ module: readFileSync(wasmPath) });
});

function keyBytes(hex: string): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return out;
}

async function decoded() {
  const key = await crypto.subtle.importKey("raw", keyBytes(meta.chunkKeyHex), "AES-GCM", false, ["decrypt"]);
  const want: ChunkIdentity = { kind: "flow", editionId: meta.editionId, chunkIndex: 0, variant: 0 };
  return openFlowChunk(key, sealed, want, parseChunk(sealed).flags);
}

/** A paragraph of `n` body lines in block `b`, each `pitch` tall. */
const para = (b: number, n: number, pitch = 10): PageBox[] =>
  Array.from({ length: n }, (_, i) => ({ gapBefore: i === 0 ? 4 : 0, pitch, blockIndex: b, kind: "paragraph" as const }));

test("planPages fills greedily and makes progress on an over-tall line", () => {
  // Three 1-line paragraphs, 10px each, into a 25px page → 2 + 1.
  const boxes: PageBox[] = [...para(0, 1), ...para(1, 1), ...para(2, 1)];
  assert.deepEqual(planPages(boxes, 25), [
    [0, 2],
    [2, 3],
  ]);
  // A single line taller than the page still occupies its own page.
  assert.deepEqual(planPages([{ gapBefore: 0, pitch: 100, blockIndex: 0, kind: "paragraph" }], 25), [[0, 1]]);
  assert.deepEqual(planPages([], 25), []);
});

test("no orphan: a block's lone first line is not stranded at a page foot", () => {
  // Block 0 has 3 lines; page holds 2 lines. A naive break after line 2 would
  // leave block 1's first line (index 2) alone at the foot → pushed down.
  const boxes: PageBox[] = [...para(0, 2), ...para(1, 3)];
  const ranges = planPages(boxes, 25);
  // The break must not end a page right after block 1's first line (index 2).
  for (const [, end] of ranges) {
    const last = end - 1;
    if (last >= 0 && last < boxes.length) {
      const isFirstOfBlock = last === 0 || boxes[last]!.blockIndex !== boxes[last - 1]!.blockIndex;
      const continues = last + 1 < boxes.length && boxes[last + 1]!.blockIndex === boxes[last]!.blockIndex;
      assert.ok(!(isFirstOfBlock && continues), `orphan at line ${last}`);
    }
  }
});

test("no widow: a block's lone last line is not carried alone to a page head", () => {
  const boxes: PageBox[] = [...para(0, 4)]; // one 4-line block
  const ranges = planPages(boxes, 25); // ~2 lines per page
  for (let p = 1; p < ranges.length; p++) {
    const start = ranges[p]![0];
    const isLastOfBlock = start === boxes.length - 1 || boxes[start]!.blockIndex !== boxes[start + 1]!.blockIndex;
    const sameAsPrev = start > 0 && boxes[start - 1]!.blockIndex === boxes[start]!.blockIndex;
    assert.ok(!(isLastOfBlock && sameAsPrev), `widow at page start ${start}`);
  }
});

test("a heading is never left at the foot of a page", () => {
  const boxes: PageBox[] = [
    ...para(0, 2),
    { gapBefore: 6, pitch: 10, blockIndex: 1, kind: "heading" },
    ...para(2, 3),
  ];
  const ranges = planPages(boxes, 35);
  for (const [, end] of ranges) {
    assert.notEqual(boxes[end - 1]!.kind, "heading", "a page ended on a heading");
  }
  // And the heading keeps at least two of its following lines on its page.
  const headingPage = ranges.find(([s, e]) => s <= 2 && 2 < e)!;
  const after = headingPage[1] - 3; // lines after the heading (index 2) on that page
  assert.ok(after >= 2, `heading kept with ${after} lines`);
});

test("paging the real chunk conserves every fragment and responds to the viewport", async () => {
  const chunk = await decoded();
  const whole = layoutChunk(new Compositor(), chunk, atlas, DEFAULT_STYLE, 2);

  const tall = paginateChunk(new Compositor(), chunk, 0, atlas, 100_000, DEFAULT_STYLE, 2);
  assert.equal(tall.pages.length, 1, "an enormous viewport is a single page");
  assert.equal(tall.pages[0]!.count, whole.count, "one page holds every fragment");

  const paged = paginateChunk(new Compositor(), chunk, 0, atlas, 700, DEFAULT_STYLE, 2);
  assert.ok(paged.pages.length > 1, `paginated into ${paged.pages.length} pages`);
  const total = paged.pages.reduce((n, p) => n + p.count, 0);
  assert.equal(total, whole.count, "every fragment appears exactly once across pages");
  assert.ok(paged.pages.length > tall.pages.length, "a smaller viewport makes more pages");

  // Anchors are this chunk's, start at the beginning, and never go backwards.
  assert.deepEqual(paged.pages[0]!.anchor, [0, 0, 0]);
  for (let i = 1; i < paged.pages.length; i++) {
    assert.ok(paged.pages[i]!.anchor[1] >= paged.pages[i - 1]!.anchor[1], "block anchors are monotonic");
  }

  // Stable/memoizable: same inputs → identical boundaries.
  const again = paginateChunk(new Compositor(), chunk, 0, atlas, 700, DEFAULT_STYLE, 2);
  assert.deepEqual(
    again.pages.map((p) => p.count),
    paged.pages.map((p) => p.count),
  );
});
