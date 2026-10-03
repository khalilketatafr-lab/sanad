/**
 * Lays a real decoded chunk out into Lumen instances (Compositor WASM +
 * emit_glyph) and checks the geometry: every fragment lands on the atlas and
 * inside the page. No browser; the pixels are checked by the headless render
 * harness.
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { before, test } from "node:test";

import { Compositor, initSync } from "@sanad/lumen-wasm";

import { DEFAULT_STYLE, glyphTableFromJson, layoutChunk } from "../src/reader/layout.ts";
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

test("a decoded chunk lays out into on-atlas, in-page instances", async () => {
  const chunk = await decoded();
  const page = layoutChunk(new Compositor(), chunk, atlas, DEFAULT_STYLE, 2);

  assert.ok(page.count > 500, `laid out ${page.count} fragments`);
  assert.equal(page.instances.length, page.count * 10);
  assert.ok(page.width > 0 && page.height > 0);

  let maxBottom = 0;
  for (let i = 0; i < page.instances.length; i += 10) {
    const [rx, ry, rw, rh] = [page.instances[i]!, page.instances[i + 1]!, page.instances[i + 2]!, page.instances[i + 3]!];
    const [sx, sy, sw, sh] = [page.instances[i + 4]!, page.instances[i + 5]!, page.instances[i + 6]!, page.instances[i + 7]!];
    // Every fragment's slot lies on the atlas.
    assert.ok(sx >= 0 && sy >= 0 && sx + sw <= atlas.width && sy + sh <= atlas.height, "slot on atlas");
    assert.ok(rw > 0 && rh > 0, "positive quad");
    // Quads stay within the page horizontally (small padding for distance range).
    assert.ok(rx > -20 && rx + rw < page.width + 20, `x in page: ${rx}..${rx + rw}`);
    maxBottom = Math.max(maxBottom, ry + rh);
  }
  assert.ok(maxBottom <= page.height + 1, "content fits the page height");
});

test("larger text produces a taller page (layout responds to size)", async () => {
  const chunk = await decoded();
  const small = layoutChunk(new Compositor(), chunk, atlas, { ...DEFAULT_STYLE, sizePx: [14, 17] }, 2);
  const large = layoutChunk(new Compositor(), chunk, atlas, { ...DEFAULT_STYLE, sizePx: [28, 34] }, 2);
  assert.ok(large.height > small.height, `${large.height} > ${small.height}`);
  assert.ok(large.count >= small.count, "more or equal fragments at a larger size");
});
