/**
 * The reader path, end to end, in the browser: a real Folio chunk sealed by
 * Atelier is opened (AES-256-GCM decrypt → zstd inflate → FlatBuffer parse),
 * laid out by the Compositor (WASM), and rendered by Lumen (WebGL2). The page
 * holds only a sealed chunk, an atlas and the lease's chunk key — never text
 * (P1). The chunk key here is the canary edition's published dev key; a real
 * session gets a per-lease, non-extractable key.
 */
import { createLumenContext, type LumenContext } from "@sanad/lumen/gl/context";
import { LumenRenderer, orthoCamera } from "@sanad/lumen/gl/renderer";
import { MAX_DIM, themeUniforms } from "@sanad/lumen/gl/themes";
import { DEFAULT_STYLE, glyphTableFromJson, layoutChunk } from "@sanad/lumen/reader/layout";
import { openFlowChunk } from "@sanad/lumen/vault/flow";
import { parseChunk } from "@sanad/lumen/vault/folio";
import { THEME_IDS, THEMES, type ThemeId } from "@sanad/tokens/ts";
import { Compositor, initSync } from "@sanad/lumen-wasm";

interface ReaderData {
  readonly editionId: string;
  readonly chunkKeyHex: string;
  readonly chunk: string; // base64 .folio
  readonly wasm: string; // base64 compositor
  readonly atlas: { readonly png: string; readonly width: number; readonly height: number; readonly pxRange: number };
  readonly glyphs: Parameters<typeof glyphTableFromJson>[0];
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

function applyChrome(id: ThemeId): void {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(THEMES[id].css)) root.style.setProperty(k, v);
  root.style.setProperty("color-scheme", THEMES[id].mode);
  root.dataset["reading"] = id;
}

async function main(): Promise<void> {
  const data = JSON.parse($("reader-data").textContent ?? "{}") as ReaderData;
  const canvas = $<HTMLCanvasElement>("page");
  const status = $("status");

  initSync({ module: base64(data.wasm) });
  const atlasTable = glyphTableFromJson(data.glyphs);

  // Open the sealed chunk exactly as the reader will, with the lease key.
  const key = await crypto.subtle.importKey("raw", hexBytes(data.chunkKeyHex), "AES-GCM", false, ["decrypt"]);
  const sealed = base64(data.chunk);
  const header = parseChunk(sealed);
  const chunk = await openFlowChunk(key, sealed, { kind: "flow", editionId: data.editionId, chunkIndex: 0, variant: 0 }, header.flags);
  $("decoded").textContent = `${chunk.blocks.length} blocks · ${chunk.blocks.reduce((n, b) => n + b.runs.reduce((m, r) => m + r.gids.length, 0), 0)} glyphs recovered`;

  const atlasPixels = await decodeAtlas(data.atlas.png, data.atlas.width, data.atlas.height);

  let ctx: LumenContext;
  try {
    ctx = createLumenContext(canvas, { onRestored: () => draw() });
  } catch (e) {
    status.textContent = `This browser cannot run the reader's renderer (WebGL2): ${String(e)}`;
    status.hidden = false;
    return;
  }
  const renderer = new LumenRenderer(ctx);
  renderer.setAtlas({ width: data.atlas.width, height: data.atlas.height, data: atlasPixels, pxRange: data.atlas.pxRange });

  let theme: ThemeId = matchMedia("(prefers-color-scheme: dark)").matches ? "night" : "paper";
  let sizeScale = 1;
  let warmth = 0;
  let dim = 0;

  function draw(): void {
    if (ctx.state !== "live") return;
    const dpr = window.devicePixelRatio || 1;
    const style = { ...DEFAULT_STYLE, sizePx: [20 * sizeScale, 24 * sizeScale] as [number, number] };
    const page = layoutChunk(new Compositor(), chunk, atlasTable, style, dpr);
    canvas.width = page.width;
    canvas.height = page.height;
    canvas.style.aspectRatio = `${page.width} / ${page.height}`;
    renderer.resize(page.width, page.height);
    renderer.setCamera(orthoCamera(page.width, page.height, 1, 0, 0));
    renderer.setTheme(themeUniforms(theme, { warmth, dim }));
    renderer.setInstances(page.instances, page.count);
    renderer.render();
    $("device").textContent = `${dpr}× · ${page.width} × ${page.height} px · ${page.count} fragments · ${ctx.caps.renderer}`;
  }

  function selectTheme(id: ThemeId): void {
    theme = id;
    applyChrome(id);
    for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
      b.setAttribute("aria-checked", String(b.dataset["themeId"] === id));
    }
    draw();
  }

  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
    b.addEventListener("click", () => selectTheme(b.dataset["themeId"] as ThemeId));
  }
  const size = $<HTMLInputElement>("size");
  size.addEventListener("input", () => {
    sizeScale = Number(size.value) / 100;
    $("size-value").textContent = `${Math.round(20 * sizeScale)}px`;
    draw();
  });
  const dimInput = $<HTMLInputElement>("dim");
  dimInput.max = String(Math.round(MAX_DIM * 100));
  dimInput.addEventListener("input", () => {
    dim = Number(dimInput.value) / 100;
    $("dim-value").textContent = `${dimInput.value}%`;
    draw();
  });
  const warmthInput = $<HTMLInputElement>("warmth");
  warmthInput.addEventListener("input", () => {
    warmth = Number(warmthInput.value) / 100;
    $("warmth-value").textContent = warmthInput.value;
    draw();
  });

  new ResizeObserver(() => draw()).observe(canvas.parentElement ?? canvas);
  selectTheme(THEME_IDS.includes(theme) ? theme : "paper");
}

void main();
