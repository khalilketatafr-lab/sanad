/**
 * Runs inside headless Chromium (real WebGL2 via ANGLE/SwiftShader). Drives
 * the production context, shaders and renderer against an atlas baked by the
 * production shredder (crates/atelier), and reports measurements.
 */
import { createLumenContext } from "../src/gl/context.ts";
import { LumenRenderer, orthoCamera } from "../src/gl/renderer.ts";
import { GLYPH_INSTANCE, ROLE } from "../src/gl/shaders.ts";
import { hexToLinear, type ThemeUniforms } from "../src/gl/theme.ts";

interface AtlasMeta {
  readonly width: number;
  readonly height: number;
  readonly slotSize: number;
  readonly pxRange: number;
  readonly slots: number;
}

const W = 288;
const H = 288;

const PAPER: ThemeUniforms = {
  paper: hexToLinear("#FBFAF7"),
  ink: hexToLinear("#1D1C1A"),
  ink2: hexToLinear("#5E5A53"),
  accent: hexToLinear("#2F5D8A"),
  highlights: [hexToLinear("#F7E8A4"), hexToLinear("#CFEBC9"), hexToLinear("#CFE0F5"), hexToLinear("#F5D3DC")],
  covGamma: 1,
  weightPx: 0,
  lumaCeil: 1,
  warmth: 0,
  dim: 0,
};

function instances(meta: AtlasMeta, slots: readonly number[], size: number): ArrayBuffer {
  const buf = new ArrayBuffer(slots.length * GLYPH_INSTANCE.stride);
  const f = new Float32Array(buf);
  const u = new Uint32Array(buf);
  slots.forEach((slot, i) => {
    const o = (i * GLYPH_INSTANCE.stride) / 4;
    f.set([16, 16, size, size], o); // a_rect (device px; camera = identity scale)
    f.set([(slot * meta.slotSize) / meta.width, 0, ((slot + 1) * meta.slotSize) / meta.width, 1], o + 4); // a_uv
    u[o + 8] = ROLE.ink;
    f[o + 9] = 0;
  });
  return buf;
}

function diff(a: Uint8Array, b: Uint8Array): { maxDiff: number; diffPixels: number } {
  let maxDiff = 0;
  let diffPixels = 0;
  for (let i = 0; i < a.length; i += 4) {
    const d = Math.max(Math.abs((a[i] ?? 0) - (b[i] ?? 0)), Math.abs((a[i + 1] ?? 0) - (b[i + 1] ?? 0)), Math.abs((a[i + 2] ?? 0) - (b[i + 2] ?? 0)));
    if (d > 0) diffPixels++;
    if (d > maxDiff) maxDiff = d;
  }
  return { maxDiff, diffPixels };
}

function inkMass(px: Uint8Array): number {
  let mass = 0;
  for (let i = 0; i < px.length; i += 4) mass += 255 - (px[i] ?? 0);
  return mass;
}

const nextFrame = (): Promise<void> => new Promise((r) => requestAnimationFrame(() => r()));
const nextTask = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  return Promise.race([p, new Promise<T>((_, reject) => setTimeout(() => reject(new Error(`timeout: ${what}`)), ms))]);
}

export async function run(meta: AtlasMeta, atlasB64: string): Promise<Record<string, unknown>> {
  const canvas = document.createElement("canvas");
  canvas.width = W;
  canvas.height = H;
  document.body.append(canvas);
  let restoredCalls = 0;
  const ctx = createLumenContext(canvas, { onRestored: () => restoredCalls++ });
  const gl = ctx.gl;
  const r = new LumenRenderer(ctx);
  r.resize(W, H);
  r.setCamera(orthoCamera(W, H, 1, 0, 0));
  const data = Uint8Array.from(atlasB64 === "" ? [] : atob(atlasB64), (c) => c.charCodeAt(0));
  r.setAtlas({ width: meta.width, height: meta.height, data, pxRange: meta.pxRange });

  const draw = (slots: readonly number[], size: number, blend: "max" | "add", theme: ThemeUniforms): Uint8Array => {
    r.setTheme(theme);
    r.setInstances(instances(meta, slots, size), slots.length);
    r.renderCoverage(blend);
    r.renderComposite();
    const px = new Uint8Array(W * H * 4);
    gl.readPixels(0, 0, W, H, gl.RGBA, gl.UNSIGNED_BYTE, px);
    return px;
  };

  const fragments = Array.from({ length: meta.slots - 1 }, (_, i) => i + 1);
  const scales: Record<string, unknown> = {};
  // 2× magnification and 0.5× minification of the 128-texel slots.
  for (const size of [256, 64]) {
    const whole = draw([0], size, "max", PAPER);
    scales[`size${size}`] = {
      maxUnion: diff(whole, draw(fragments, size, "max", PAPER)),
      additive: diff(whole, draw(fragments, size, "add", PAPER)),
      singleFragment: diff(whole, draw([1], size, "max", PAPER)),
      inkMass: inkMass(whole),
    };
  }

  const weight = {
    thinner: inkMass(draw([0], 64, "max", { ...PAPER, weightPx: -0.4 })),
    neutral: inkMass(draw([0], 64, "max", PAPER)),
    bolder: inkMass(draw([0], 64, "max", { ...PAPER, weightPx: 0.4 })),
  };

  // Anti-glare: white paper under a 0.45 ceiling must come out at Y ≈ 0.45.
  const glare = draw([], 64, "max", { ...PAPER, paper: [1, 1, 1], lumaCeil: 0.45 });
  const lin = (v: number): number => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const paperY = 0.2126 * lin(glare[0] ?? 0) + 0.7152 * lin(glare[1] ?? 0) + 0.0722 * lin(glare[2] ?? 0);

  // preserveDrawingBuffer=false: after the frame is composited the buffer is
  // cleared, so a later readback cannot recover the page.
  const inFrame = draw([0], 256, "max", PAPER);
  await withTimeout(nextFrame(), 5000, "rAF");
  await withTimeout(nextFrame(), 5000, "rAF");
  const later = new Uint8Array(W * H * 4);
  gl.readPixels(0, 0, W, H, gl.RGBA, gl.UNSIGNED_BYTE, later);
  const distinct = new Set<number>();
  for (let i = 0; i < later.length; i += 4) distinct.add(((later[i] ?? 0) << 16) | ((later[i + 1] ?? 0) << 8) | (later[i + 2] ?? 0));

  // Context loss/restore plumbing.
  const lost = new Promise<void>((res) => canvas.addEventListener("webglcontextlost", () => res(), { once: true }));
  const simulated = ctx.simulateLoss();
  await withTimeout(lost, 5000, "webglcontextlost");
  // Restoration is only permitted once the lost-event dispatch has completed
  // (its default prevented). Our promise resumes in a microtask *during*
  // dispatch, so yield one task first.
  await nextTask();
  const stateAfterLoss = ctx.state;
  const restored = new Promise<void>((res) => canvas.addEventListener("webglcontextrestored", () => res(), { once: true }));
  ctx.simulateRestore();
  await withTimeout(restored, 5000, "webglcontextrestored");

  return {
    renderer: ctx.caps.renderer,
    software: ctx.caps.software,
    attributes: gl.getContextAttributes(),
    scales,
    weight,
    paperY,
    laterReadback: {
      distinctColors: distinct.size,
      firstPixel: Array.from(later.subarray(0, 4)),
      inFrameDistinctColors: new Set(Array.from({ length: inFrame.length / 4 }, (_, i) => inFrame[i * 4] ?? 0)).size,
    },
    loss: { simulated, stateAfterLoss, stateAfterRestore: ctx.state, restoredCalls },
  };
}
