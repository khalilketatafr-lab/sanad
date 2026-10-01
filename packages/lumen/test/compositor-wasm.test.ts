/**
 * Interop tests for the Compositor's wasm build (packages/lumen-wasm, built
 * by `pnpm --filter @sanad/lumen-wasm build`) and the TS ring producer.
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { before, test } from "node:test";
import { Worker } from "node:worker_threads";
import { Compositor, PointerRingConsumer, initSync } from "@sanad/lumen-wasm";
import { PointerRingProducer, type PointerPhase } from "../src/input/pointer-ring.ts";
import { digestReflow, generateReflowFixture, prepare } from "./fixtures/reflow.ts";

const wasmPath = fileURLToPath(import.meta.resolve("@sanad/lumen-wasm/sanad_compositor_bg.wasm"));

before(() => {
  initSync({ module: readFileSync(wasmPath) });
});

// Flags (crates/folio/schemas/folio.fbs GlyphFlags bit positions).
const CLUSTER_START = 1;
const BREAK_OK = 2;
const GLUE = 4;
const WORD_START = 32;

/** Words of n glyphs (advance 500 units = 10 px at scale 0.02) separated by 250-unit spaces. */
function paragraph(words: readonly number[]) {
  const gids: number[] = [];
  const adv: number[] = [];
  const flags: number[] = [];
  words.forEach((n, w) => {
    if (w > 0) {
      gids.push(3);
      adv.push(250);
      flags.push(CLUSTER_START | GLUE | BREAK_OK);
    }
    for (let i = 0; i < n; i++) {
      gids.push(1000 + gids.length);
      adv.push(500);
      flags.push(CLUSTER_START | (i === 0 ? WORD_START : 0));
    }
  });
  const n = gids.length;
  // A GPOS-style displacement on glyph 1 (x +100, y +200 units): moves that
  // glyph only, never the pen.
  const offsets = new Int16Array(2 * n);
  offsets.set([100, 200], 2);
  return {
    gids: Uint16Array.from(gids),
    adv: Int16Array.from(adv),
    offsets,
    flags: Uint8Array.from(flags),
    levels: new Uint8Array(n),
  };
}

function compositorWithParagraph(): Compositor {
  const p = paragraph([3, 4, 5]);
  const c = new Compositor();
  const lines = c.layout_paragraph(p.gids, p.adv, p.offsets, p.flags, p.levels, new Uint8Array(), new Uint16Array(), 0.02, 500, 0, 99, 330, 0, 0);
  assert.equal(lines, 1);
  c.set_line_height(30);
  return c;
}

function tap(ring: PointerRingProducer, x: number, y: number, t0: number, dx = 1, dt = 80): void {
  const base = { pointerId: 7, device: "touch" as const, buttons: 1, pressure: 0.5 };
  const at = (phase: PointerPhase, px: number, t: number) => ring.push({ ...base, t, x: px, y, phase });
  at("down", x, t0);
  at("up", x + dx, t0 + dt);
}

test("wasm layout matches the native layout contract", () => {
  const c = compositorWithParagraph();
  const g = c.line_glyphs(0); // [gid, x, y, scale_x, kind]*
  assert.equal(g.length / 5, 12, "12 glyphs, spaces carry no glyph");
  assert.equal(g[1], 0); // first glyph at x = 0
  assert.equal(g[5 * 3 + 1], 35); // word 2 starts after 30 px of glyphs + 5 px space
  assert.deepEqual([g[5 + 1], g[5 + 2]], [12, 4], "glyph 1 drawn at pen 10 px + 2 px, 4 px above the baseline");
  assert.equal(g[10 + 1], 20, "the offset does not move the pen");
  c.free();
});

test("wasm lays out real Latin and Arabic paragraphs bit-identically to native", () => {
  // Atelier shapes and permutes the §10.4 fixture and records the native
  // Compositor's output digest at 5 measures; the wasm build must match it.
  const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
  const fixture = generateReflowFixture(repo);
  assert.equal(fixture.paragraphs.length, 20);
  const paragraphs = fixture.paragraphs.map(prepare);
  const c = new Compositor();
  for (const ref of fixture.reference) {
    const got = digestReflow(c, paragraphs, ref.width);
    assert.equal(got.lines, ref.lines, `${ref.width}px: line count`);
    assert.equal(got.digest, ref.digest, `${ref.width}px: glyph positions differ from native`);
  }
});

test("taps travel main thread → SharedArrayBuffer → wasm and come back hit-tested", () => {
  const c = compositorWithParagraph();
  const ring = PointerRingProducer.create(64);
  c.attach_pointer_ring(ring.buffer);
  tap(ring, 46, 10, 1000); // glyph 1 of word 2 = cluster 5
  tap(ring, 1, 10, 2000); // cluster 0
  assert.deepEqual(Array.from(c.poll_pointer()), [0, 5, 0, 0]);
  assert.deepEqual(Array.from(c.poll_pointer()), [], "already consumed");
  c.free();
});

test("drags and slow presses are not taps", () => {
  const c = compositorWithParagraph();
  const ring = PointerRingProducer.create(64);
  c.attach_pointer_ring(ring.buffer);
  tap(ring, 46, 10, 1000, 30); // moved 30 px
  tap(ring, 46, 10, 2000, 1, 900); // held 900 ms
  assert.deepEqual(Array.from(c.poll_pointer()), []);
  c.free();
});

test("view transform maps surface pixels into layout space", () => {
  const c = compositorWithParagraph();
  const ring = PointerRingProducer.create(64);
  c.attach_pointer_ring(ring.buffer);
  // Surface is zoomed 2× and panned by (+100, +40): layout = (surface − pan) / 2.
  c.set_view_transform(0.5, 0, 0, 0.5, -50, -20);
  tap(ring, 2 * 46 + 100, 2 * 10 + 40, 1000);
  assert.deepEqual(Array.from(c.poll_pointer()), [0, 5]);
  c.free();
});

test("a lagging consumer loses the oldest samples, counted, never garbage", () => {
  const ring = PointerRingProducer.create(64);
  const consumer = new PointerRingConsumer(ring.buffer);
  for (let i = 0; i < 200; i++) {
    ring.push({ t: 2 * i, x: i, y: i, pointerId: i, phase: "move", device: "mouse", buttons: 0, pressure: 0 });
  }
  const s = consumer.poll();
  const delivered = s.length / 9;
  assert.equal(delivered + consumer.dropped(), 200);
  assert.ok(consumer.dropped() >= 136);
  assert.equal(s[(delivered - 1) * 9 + 2], 199, "newest sample kept");
  consumer.free();
});

test("rejects a buffer that is not an initialized ring", () => {
  const c = new Compositor();
  assert.throws(() => c.attach_pointer_ring(new SharedArrayBuffer(64)), /version/);
  c.free();
});

test("concurrent producer and wasm consumer on separate threads: no torn samples", async () => {
  const total = 300_000;
  const ring = PointerRingProducer.create(1024);
  const worker = new Worker(fileURLToPath(new URL("./fixtures/ring-consumer-worker.ts", import.meta.url)), {
    workerData: { sab: ring.buffer, wasmPath, total },
  });
  const result = await new Promise<{ delivered: number; dropped: number; torn: number; nonMonotonic: number; lastX: number }>(
    (resolve, reject) => {
      worker.on("error", reject);
      worker.on("message", (m: { ready?: boolean }) => {
        if (m.ready === true) {
          for (let i = 0; i < total; i++) {
            ring.push({ t: 2 * i, x: i, y: i, pointerId: i, phase: "move", device: "pen", buttons: 0, pressure: 0.25 });
          }
          return;
        }
        resolve(m as never);
      });
    },
  );
  await worker.terminate();
  assert.equal(result.torn, 0, "torn sample delivered");
  assert.equal(result.nonMonotonic, 0, "out-of-order sample delivered");
  assert.equal(result.lastX, total - 1, "consumer saw the final sample");
  assert.equal(result.delivered + result.dropped, total, JSON.stringify(result));
  console.log(`ring stress: ${result.delivered} delivered, ${result.dropped} dropped under full-speed contention`);
});
