/**
 * The reader path, end to end, in the browser: a real Folio chunk sealed by
 * Atelier is opened (AES-256-GCM decrypt → zstd inflate → FlatBuffer parse),
 * laid out by the Compositor (WASM), and rendered by Lumen (WebGL2). The page
 * holds only a sealed chunk, an atlas and the lease's chunk key — never text
 * (P1). The chunk key here is the canary edition's published dev key; a real
 * session gets a per-lease, non-extractable key.
 *
 * The reading controls are driven by the production settings model
 * (@sanad/lumen/reader/settings): a SettingsStore persisted to localStorage,
 * whose toReaderStyle()/readerLight() feed the very same layout and theme code
 * the reader uses. Reload the page and your size, theme, spacing and margins
 * come back.
 */
import { createLumenContext, type LumenContext } from "@sanad/lumen/gl/context";
import { LumenRenderer, orthoCamera } from "@sanad/lumen/gl/renderer";
import { MAX_DIM, themeUniforms } from "@sanad/lumen/gl/themes";
import { glyphTableFromJson, layoutChunk } from "@sanad/lumen/reader/layout";
import {
  SIZE_STEPS,
  SettingsStore,
  browserStorage,
  defaultsFor,
  type Margins,
  type Spacing,
} from "@sanad/lumen/reader/settings";
import { openFlowChunk } from "@sanad/lumen/vault/flow";
import { parseChunk } from "@sanad/lumen/vault/folio";
import { THEMES, type ThemeId } from "@sanad/tokens/ts";
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

  // The production settings model, persisted to localStorage, with form-factor
  // and time-of-day defaults.
  const store = new SettingsStore({
    storage: browserStorage(),
    base: defaultsFor({
      phone: matchMedia("(max-width: 760px)").matches,
      night: matchMedia("(prefers-color-scheme: dark)").matches,
    }),
  });

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

  function draw(): void {
    if (ctx.state !== "live") return;
    const s = store.value;
    const dpr = window.devicePixelRatio || 1;
    const page = layoutChunk(new Compositor(), chunk, atlasTable, store.style, dpr);
    canvas.width = page.width;
    canvas.height = page.height;
    canvas.style.aspectRatio = `${page.width} / ${page.height}`;
    renderer.resize(page.width, page.height);
    renderer.setCamera(orthoCamera(page.width, page.height, 1, 0, 0));
    applyChrome(s.theme);
    renderer.setTheme(themeUniforms(s.theme, store.light));
    renderer.setInstances(page.instances, page.count);
    renderer.render();
    $("device").textContent = `${dpr}× · ${page.width} × ${page.height} px · ${page.count} fragments · ${ctx.caps.renderer}`;
  }

  function reflect(): void {
    const s = store.value;
    for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
      b.setAttribute("aria-checked", String(b.dataset["themeId"] === s.theme));
    }
    for (const b of document.querySelectorAll<HTMLButtonElement>("[data-spacing]")) {
      b.setAttribute("aria-checked", String(b.dataset["spacing"] === s.spacing));
    }
    for (const b of document.querySelectorAll<HTMLButtonElement>("[data-margins]")) {
      b.setAttribute("aria-checked", String(b.dataset["margins"] === s.margins));
    }
    $("size-value").textContent = `${s.size}px`;
    $("warmth-value").textContent = String(s.warmth);
    $("dim-value").textContent = `${Math.round(s.dim * 100)}%`;
  }

  // Controls → store. Every change is normalized, persisted and pushed back.
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
    b.addEventListener("click", () => store.set({ theme: b.dataset["themeId"] as ThemeId }));
  }
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-spacing]")) {
    b.addEventListener("click", () => store.set({ spacing: b.dataset["spacing"] as Spacing }));
  }
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-margins]")) {
    b.addEventListener("click", () => store.set({ margins: b.dataset["margins"] as Margins }));
  }

  const size = $<HTMLInputElement>("size");
  size.max = String(SIZE_STEPS.length - 1);
  size.value = String(Math.max(0, SIZE_STEPS.indexOf(store.value.size)));
  size.addEventListener("input", () => store.set({ size: SIZE_STEPS[Number(size.value)] ?? store.value.size }));

  const warmth = $<HTMLInputElement>("warmth");
  warmth.value = String(store.value.warmth);
  warmth.addEventListener("input", () => store.set({ warmth: Number(warmth.value) }));

  const dim = $<HTMLInputElement>("dim");
  dim.max = String(Math.round(MAX_DIM * 100));
  dim.value = String(Math.round(store.value.dim * 100));
  dim.addEventListener("input", () => store.set({ dim: Number(dim.value) / 100 }));

  // The store drives both the picture and the control reflections.
  store.subscribe(() => {
    reflect();
    draw();
  });
  new ResizeObserver(() => draw()).observe(canvas.parentElement ?? canvas);

  // Signal first paint so the P1 harness scans a fully-rendered page.
  try {
    performance.mark("lumen:ready");
  } catch {
    /* performance API unavailable */
  }
}

void main();
