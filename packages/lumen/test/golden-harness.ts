/**
 * Runs inside headless Chromium. Renders the golden pages produced by
 * `cargo run -p sanad-atelier --example golden_pages` with the production
 * renderer in every theme, and reports, per page and theme:
 * - `union`: the shredded page against the same page drawn from whole glyphs
 *   (must be identical);
 * - `golden`: the page against the committed golden PNG, when one exists;
 * - the rendered page as PNG (base64) for review and golden updates.
 *
 * Files are served by the test under the page's own origin.
 */
import { THEME_IDS, type ThemeId } from "@sanad/tokens/ts";
import { createLumenContext } from "../src/gl/context.ts";
import { LumenRenderer, orthoCamera } from "../src/gl/renderer.ts";
import { themeUniforms } from "../src/gl/themes.ts";

interface Meta {
  readonly width: number;
  readonly height: number;
  readonly atlas: { readonly file: string; readonly width: number; readonly height: number; readonly pxRange: number };
  readonly atlasWhole: { readonly file: string; readonly width: number; readonly height: number };
  readonly pages: readonly { readonly name: string; readonly file: string; readonly count: number; readonly wholeFile: string; readonly wholeCount: number }[];
}

export interface Diff {
  readonly maxDiff: number;
  readonly diffPixels: number;
}

export interface PageResult {
  readonly page: string;
  readonly theme: ThemeId;
  readonly union: Diff;
  readonly golden: Diff | null;
  /** Pixels that differ from the paper color: the page actually has text. */
  readonly inkPixels: number;
  readonly png: string;
  /** When `union` differs: the page with differing pixels in magenta. */
  readonly unionDiffPng: string | null;
}

async function bytes(url: string): Promise<Uint8Array> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

function diff(a: Uint8Array, b: Uint8Array): Diff {
  let maxDiff = 0;
  let diffPixels = 0;
  for (let i = 0; i < a.length; i += 4) {
    const d = Math.max(Math.abs((a[i] ?? 0) - (b[i] ?? 0)), Math.abs((a[i + 1] ?? 0) - (b[i + 1] ?? 0)), Math.abs((a[i + 2] ?? 0) - (b[i + 2] ?? 0)));
    if (d > 0) diffPixels++;
    if (d > maxDiff) maxDiff = d;
  }
  return { maxDiff, diffPixels };
}

async function toPng(rgba: Uint8Array, w: number, h: number): Promise<string> {
  const c = new OffscreenCanvas(w, h);
  const ctx = c.getContext("2d");
  if (ctx === null) throw new Error("2d context");
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  const buf = new Uint8Array(await (await c.convertToBlob({ type: "image/png" })).arrayBuffer());
  let bin = "";
  for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
  return btoa(bin);
}

async function decodePng(url: string, w: number, h: number): Promise<Uint8Array | null> {
  const r = await fetch(url);
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  const bmp = await createImageBitmap(await r.blob(), { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  if (bmp.width !== w || bmp.height !== h) return new Uint8Array(0);
  const c = new OffscreenCanvas(w, h);
  const ctx = c.getContext("2d");
  if (ctx === null) throw new Error("2d context");
  ctx.drawImage(bmp, 0, 0);
  return new Uint8Array(ctx.getImageData(0, 0, w, h).data.buffer);
}

export async function run(): Promise<PageResult[]> {
  const meta = (await (await fetch("/golden/pages.json")).json()) as Meta;
  const [W, H] = [meta.width, meta.height];
  const canvas = document.createElement("canvas");
  canvas.width = W;
  canvas.height = H;
  document.body.append(canvas);
  const ctx = createLumenContext(canvas);
  const gl = ctx.gl;
  const r = new LumenRenderer(ctx);
  r.resize(W, H);
  r.setCamera(orthoCamera(W, H, 1, 0, 0));

  const shredded = { width: meta.atlas.width, height: meta.atlas.height, data: await bytes(`/golden/${meta.atlas.file}`), pxRange: meta.atlas.pxRange };
  const whole = { width: meta.atlasWhole.width, height: meta.atlasWhole.height, data: await bytes(`/golden/${meta.atlasWhole.file}`), pxRange: meta.atlas.pxRange };

  // Renders and reads back within the same task (preserveDrawingBuffer=false),
  // flipping GL's bottom-up rows to top-down.
  const draw = (instances: Uint8Array, count: number): Uint8Array => {
    r.setInstances(instances, count);
    r.renderCoverage("max");
    r.renderComposite();
    const px = new Uint8Array(W * H * 4);
    gl.readPixels(0, 0, W, H, gl.RGBA, gl.UNSIGNED_BYTE, px);
    const out = new Uint8Array(px.length);
    const row = W * 4;
    for (let y = 0; y < H; y++) out.set(px.subarray((H - 1 - y) * row, (H - y) * row), y * row);
    return out;
  };

  const results: PageResult[] = [];
  for (const page of meta.pages) {
    const inst = await bytes(`/golden/${page.file}`);
    const instWhole = await bytes(`/golden/${page.wholeFile}`);
    for (const theme of THEME_IDS) {
      r.setTheme(themeUniforms(theme));
      r.setAtlas(whole);
      const reference = draw(instWhole, page.wholeCount);
      r.setAtlas(shredded);
      const px = draw(inst, page.count);
      const paper = [px[0], px[1], px[2]];
      let inkPixels = 0;
      for (let i = 0; i < px.length; i += 4) if (px[i] !== paper[0] || px[i + 1] !== paper[1] || px[i + 2] !== paper[2]) inkPixels++;
      const golden = await decodePng(`/fixtures/golden/${page.name}-${theme}.png`, W, H);
      let unionDiffPng: string | null = null;
      const union = diff(px, reference);
      if (union.diffPixels > 0) {
        const map = Uint8Array.from(px);
        for (let i = 0; i < px.length; i += 4) {
          if (px[i] !== reference[i] || px[i + 1] !== reference[i + 1] || px[i + 2] !== reference[i + 2]) map.set([255, 0, 255, 255], i);
        }
        unionDiffPng = await toPng(map, W, H);
      }
      results.push({
        page: page.name,
        theme,
        union,
        golden: golden === null ? null : golden.length === px.length ? diff(px, golden) : { maxDiff: 255, diffPixels: W * H },
        inkPixels,
        png: await toPng(px, W, H),
        unionDiffPng,
      });
    }
  }
  return results;
}
