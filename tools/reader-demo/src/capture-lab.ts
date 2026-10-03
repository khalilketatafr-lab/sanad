/**
 * Capture Lab — an honest proof of concept for temporal multiplexing, at the
 * level of our shredded glyph fragments.
 *
 * The real page (decoded from a sealed Folio chunk, laid out by the Compositor)
 * is a cloud of MSDF *fragments* — each glyph is already shredded into several.
 * We assign every fragment to a subset of N sub-frames and present the
 * sub-frames in a fast cycle: each one draws only a fraction (the "duty") of the
 * fragments, chosen per-fragment at random. The eye integrates the sub-frames
 * back into clean text; a single captured frame — what a screenshot grabs — is
 * missing the rest, so every glyph is riddled with holes.
 *
 * This is a measurement tool, not a shipping feature, and it does NOT block
 * capture. A screen *recording* at a matching frame rate averages the
 * sub-frames back together, and the flicker has a real accessibility cost
 * (photosensitivity, eye strain) that conflicts with blueprint 04's "respect
 * the body". It exists so the trade-off — how degraded a single frame is vs.
 * how much perceived contrast and comfort it costs — can be judged from
 * evidence, not slogans. It reuses the production renderer unchanged: a
 * sub-frame is just the page drawn with a subset of its fragments.
 */
import { createLumenContext, type LumenContext } from "@sanad/lumen/gl/context";
import { LumenRenderer, orthoCamera } from "@sanad/lumen/gl/renderer";
import { themeUniforms } from "@sanad/lumen/gl/themes";
import { DEFAULT_STYLE, glyphTableFromJson, layoutChunk, type LaidOutPage } from "@sanad/lumen/reader/layout";
import { openFlowChunk } from "@sanad/lumen/vault/flow";
import { parseChunk } from "@sanad/lumen/vault/folio";
import { type ThemeId } from "@sanad/tokens/ts";
import { Compositor, initSync } from "@sanad/lumen-wasm";

interface ReaderData {
  readonly editionId: string;
  readonly chunkKeyHex: string;
  readonly chunk: string;
  readonly wasm: string;
  readonly atlas: { readonly png: string; readonly width: number; readonly height: number; readonly pxRange: number };
  readonly glyphs: Parameters<typeof glyphTableFromJson>[0];
}

interface TmConfig {
  on: boolean;
  /** Sub-frames per integration cycle (N). */
  frames: number;
  /** Fraction of the N sub-frames each fragment is shown on (duty cycle). */
  duty: number;
}

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`#${id} missing`);
  return el as T;
};

function base64(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function hexBytes(hex: string): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return out;
}

async function decodeAtlas(dataUri: string, width: number, height: number): Promise<Uint8Array> {
  const blob = await (await fetch(dataUri)).blob();
  const bmp = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d", { colorSpace: "srgb", willReadFrequently: true });
  if (ctx === null) throw new Error("2D canvas unavailable");
  ctx.drawImage(bmp, 0, 0);
  return new Uint8Array(ctx.getImageData(0, 0, width, height).data.buffer);
}

/** Integer hash of a fragment index → its random sub-frame start. */
function frameStart(i: number, frames: number): number {
  let x = (i * 2654435761) >>> 0;
  x ^= x >>> 15;
  x = (x * 2246822519) >>> 0;
  x ^= x >>> 13;
  return (x >>> 0) % frames;
}

/**
 * Splits a page's fragment instances into N sub-frames. Each fragment is shown
 * on `onCount = round(duty·N)` consecutive sub-frames from a per-fragment random
 * start, so every sub-frame holds about `duty` of the fragments and the N of
 * them together hold all of them (with `onCount`× overlap).
 */
function buildSubframes(page: LaidOutPage, frames: number, duty: number): { data: Float32Array; count: number }[] {
  const fl = page.count === 0 ? 0 : page.instances.length / page.count;
  const onCount = Math.max(1, Math.round(duty * frames));
  const buckets: number[][] = Array.from({ length: frames }, () => []);
  for (let i = 0; i < page.count; i++) {
    const start = frameStart(i, frames);
    const base = i * fl;
    for (let k = 0; k < onCount; k++) {
      const f = (start + k) % frames;
      const b = buckets[f]!;
      for (let j = 0; j < fl; j++) b.push(page.instances[base + j]!);
    }
  }
  return buckets.map((b) => ({ data: new Float32Array(b), count: fl === 0 ? 0 : b.length / fl }));
}

async function main(): Promise<void> {
  const data = JSON.parse($("reader-data").textContent ?? "{}") as ReaderData;
  const canvas = $<HTMLCanvasElement>("page");

  initSync({ module: base64(data.wasm) });
  const atlasTable = glyphTableFromJson(data.glyphs);
  const key = await crypto.subtle.importKey("raw", hexBytes(data.chunkKeyHex), "AES-GCM", false, ["decrypt"]);
  const sealed = base64(data.chunk);
  const chunk = await openFlowChunk(
    key,
    sealed,
    { kind: "flow", editionId: data.editionId, chunkIndex: 0, variant: 0 },
    parseChunk(sealed).flags,
  );
  const atlasPixels = await decodeAtlas(data.atlas.png, data.atlas.width, data.atlas.height);

  let ctx: LumenContext;
  try {
    ctx = createLumenContext(canvas, {});
  } catch (e) {
    $("status").textContent = `WebGL2 unavailable: ${String(e)}`;
    $("status").hidden = false;
    return;
  }
  const renderer = new LumenRenderer(ctx);
  renderer.setAtlas({ width: data.atlas.width, height: data.atlas.height, data: atlasPixels, pxRange: data.atlas.pxRange });

  const theme: ThemeId = "paper";
  const tm: TmConfig = { on: true, frames: 3, duty: 0.5 };

  let page: LaidOutPage = layoutChunk(new Compositor(), chunk, atlasTable, DEFAULT_STYLE, 1);
  let subframes = buildSubframes(page, tm.frames, tm.duty);
  let phase = 0;

  function sizeTo(): void {
    canvas.width = page.width;
    canvas.height = page.height;
    canvas.style.aspectRatio = `${page.width} / ${page.height}`;
    renderer.resize(page.width, page.height);
    renderer.setCamera(orthoCamera(page.width, page.height, 1, 0, 0));
    renderer.setTheme(themeUniforms(theme, { warmth: 0, dim: 0 }));
  }

  function relayout(): void {
    page = layoutChunk(new Compositor(), chunk, atlasTable, DEFAULT_STYLE, window.devicePixelRatio || 1);
    subframes = buildSubframes(page, tm.frames, tm.duty);
    sizeTo();
  }

  function drawClean(): void {
    renderer.setInstances(page.instances, page.count);
    renderer.render();
  }

  function drawSubframe(f: number): void {
    const s = subframes[f % subframes.length]!;
    renderer.setInstances(s.data, s.count);
    renderer.render();
  }

  // Frozen state keeps the loop redrawing one frame so a WebGL screenshot stays
  // reliable (the drawing buffer is cleared after compositing).
  let frozen: { mode: "clean" | "tm"; phase: number } | null = null;
  function loop(): void {
    if (ctx.state === "live") {
      if (frozen !== null) {
        if (frozen.mode === "clean") drawClean();
        else drawSubframe(frozen.phase);
      } else if (tm.on) {
        phase = (phase + 1) % tm.frames;
        drawSubframe(phase);
      } else {
        drawClean();
      }
    }
    requestAnimationFrame(loop);
  }

  sizeTo();
  drawClean();

  // ── Controls ───────────────────────────────────────────────────────────
  $<HTMLInputElement>("tm").addEventListener("change", (e) => {
    tm.on = (e.target as HTMLInputElement).checked;
  });
  const bindRange = (id: string, f: (v: number) => void, out: (v: number) => string): void => {
    const el = $<HTMLInputElement>(id);
    el.addEventListener("input", () => {
      const v = Number(el.value);
      f(v);
      subframes = buildSubframes(page, tm.frames, tm.duty);
      $(`${id}-value`).textContent = out(v);
    });
  };
  bindRange("frames", (v) => (tm.frames = v), (v) => String(v));
  bindRange("duty", (v) => (tm.duty = v / 100), (v) => `${v}%`);

  // ── Headless measurement hooks ─────────────────────────────────────────
  type Caplab = {
    ready: boolean;
    dims: () => [number, number];
    fragments: () => number;
    config: (c: Partial<TmConfig>) => void;
    still: (mode: "clean" | "tm", phase: number) => void;
    play: () => void;
    /** Analytic single-frame degradation: fragments shown and perceived duty. */
    measure: (frames: number, duty: number) => { total: number; perFrame: number; recall: number };
  };
  (globalThis as unknown as { __caplab: Caplab }).__caplab = {
    ready: true,
    dims: () => [page.width, page.height],
    fragments: () => page.count,
    config: (c) => {
      Object.assign(tm, c);
      subframes = buildSubframes(page, tm.frames, tm.duty);
    },
    still: (mode, ph) => {
      frozen = { mode, phase: ph };
    },
    play: () => {
      frozen = null;
    },
    measure: (frames, duty) => {
      const subs = buildSubframes(page, frames, duty);
      const perFrame = subs[0]?.count ?? 0;
      return { total: page.count, perFrame, recall: page.count === 0 ? 0 : perFrame / page.count };
    },
  };

  requestAnimationFrame(loop);
}

void main();
