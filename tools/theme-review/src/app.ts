/**
 * Theme review (roadmap §10.6), running on the reviewer's phone.
 *
 * The golden pages are drawn by the production renderer (Lumen, WebGL2) at
 * the device's own resolution: MSDF glyphs are resolution-independent, so a
 * 3× phone sees exactly what readers will see. The chrome around them uses
 * the same Marginalia tokens the reader ships, so page and chrome can be
 * compared side by side in a dark room and in daylight.
 */
import { createLumenContext, type LumenContext } from "@sanad/lumen/gl/context";
import { LumenRenderer, orthoCamera } from "@sanad/lumen/gl/renderer";
import { MAX_DIM, themeUniforms } from "@sanad/lumen/gl/themes";
import { THEMES, THEME_IDS, type CssVar, type ThemeId } from "@sanad/tokens/ts";

interface ReviewPage {
  readonly name: string;
  readonly label: string;
  readonly count: number;
  /** GLYPH_INSTANCE records, base64. */
  readonly instances: string;
}

interface ReviewData {
  readonly pageWidth: number;
  readonly pageHeight: number;
  readonly atlas: { readonly png: string; readonly width: number; readonly height: number; readonly pxRange: number };
  readonly pages: readonly ReviewPage[];
}

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`#${id} missing`);
  return el as T;
};

function base64Bytes(b64: string): Uint8Array {
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

/** Decodes the PNG atlas to exact RGBA8 (no color management, no premultiplication). */
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

// ── Color math for the swatches (WCAG 2.x) ───────────────────────────────
type Rgb8 = readonly [number, number, number];

function parseColor(css: string): { rgb: Rgb8; alpha: number } {
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/iu.exec(css);
  if (hex !== null) return { rgb: [1, 2, 3].map((i) => Number.parseInt(hex[i] ?? "0", 16)) as unknown as Rgb8, alpha: 1 };
  const rgb = /rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)/u.exec(css);
  if (rgb !== null) return { rgb: [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])], alpha: Number(rgb[4]) };
  throw new Error(`unparsed color ${css}`);
}

const lin = (c: number): number => (c / 255 <= 0.04045 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4);
const luminance = ([r, g, b]: Rgb8): number => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
function contrast(a: Rgb8, b: Rgb8): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}
const over = (fg: Rgb8, alpha: number, bg: Rgb8): Rgb8 =>
  fg.map((c, i) => Math.round(alpha * c + (1 - alpha) * (bg[i] ?? 0))) as unknown as Rgb8;

interface Pair {
  readonly label: string;
  readonly fg: CssVar;
  readonly bg: CssVar | "worst-glass";
  readonly floor: number;
}

const PAIRS: readonly Pair[] = [
  { label: "Ink on paper", fg: "--ink", bg: "--paper", floor: 7 },
  { label: "Ink 2 on paper", fg: "--ink-2", bg: "--paper", floor: 4.5 },
  { label: "Accent on paper", fg: "--accent", bg: "--paper", floor: 4.5 },
  { label: "Ink on yellow", fg: "--ink", bg: "--highlight-1", floor: 6.5 },
  { label: "Ink on green", fg: "--ink", bg: "--highlight-2", floor: 6.5 },
  { label: "Ink on blue", fg: "--ink", bg: "--highlight-3", floor: 6.5 },
  { label: "Ink on rose", fg: "--ink", bg: "--highlight-4", floor: 6.5 },
  { label: "Ink on surface 1", fg: "--ink", bg: "--surface-1", floor: 7 },
  { label: "Ink on surface 2", fg: "--ink", bg: "--surface-2", floor: 7 },
  { label: "Ink on glass, worst case", fg: "--ink", bg: "worst-glass", floor: 7 },
  { label: "Ink 2 on glass, worst case", fg: "--ink-2-on-glass", bg: "worst-glass", floor: 5.4 },
];

function renderSwatches(id: ThemeId): void {
  const css = THEMES[id].css;
  const color = (v: CssVar): Rgb8 => parseColor(css[v]).rgb;
  const glass = parseColor(css["--glass-tint"]);
  const worstGlass = (fg: Rgb8): number =>
    Math.min(...([[0, 0, 0], [255, 255, 255]] as const).map((backdrop) => contrast(fg, over(glass.rgb, glass.alpha, backdrop))));
  const rows = PAIRS.map((p) => {
    const ratio = p.bg === "worst-glass" ? worstGlass(color(p.fg)) : contrast(color(p.fg), color(p.bg));
    const ok = ratio >= p.floor;
    const bgVar = p.bg === "worst-glass" ? "--glass-tint" : p.bg;
    return `<li class="pair">
      <span class="chip" style="background:var(${bgVar});color:var(${p.fg})" aria-hidden="true">Aa</span>
      <span class="pair-label">${p.label}</span>
      <span class="pair-ratio ${ok ? "pass" : "fail"}">${ratio.toFixed(1)}:1<small> ≥ ${p.floor}</small></span>
    </li>`;
  });
  $("pairs").innerHTML = rows.join("");
  const keys: readonly CssVar[] = ["--paper", "--ink", "--ink-2", "--accent", "--highlight-1", "--highlight-2", "--highlight-3", "--highlight-4", "--surface-1", "--surface-2"];
  $("swatches").innerHTML = keys
    .map((k) => `<li class="swatch"><span class="swatch-fill" style="background:var(${k})"></span><span class="swatch-name">${k.slice(2)}</span><code>${css[k]}</code></li>`)
    .join("");
}

function applyChrome(id: ThemeId): void {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(THEMES[id].css)) root.style.setProperty(k, v);
  root.style.setProperty("color-scheme", THEMES[id].mode);
  root.dataset["reading"] = id;
}

// ── Checklist (per reviewer, local only) ─────────────────────────────────
const CHECK_KEY = "sanad-theme-review/v1";

function loadChecks(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(CHECK_KEY) ?? "{}") as Record<string, boolean>;
  } catch {
    return {};
  }
}

function wireChecklist(): void {
  const state = loadChecks();
  for (const box of document.querySelectorAll<HTMLInputElement>("#checklist input[type=checkbox]")) {
    box.checked = state[box.id] === true;
    box.addEventListener("change", () => {
      state[box.id] = box.checked;
      try {
        localStorage.setItem(CHECK_KEY, JSON.stringify(state));
      } catch {
        /* storage unavailable: the checklist still works for this visit */
      }
    });
  }
}

// ── Lumen ────────────────────────────────────────────────────────────────
async function main(): Promise<void> {
  const data = JSON.parse($("review-data").textContent ?? "{}") as ReviewData;
  const canvas = $<HTMLCanvasElement>("page");
  const status = $("status");
  const pages = data.pages.map((p) => ({ ...p, bytes: base64Bytes(p.instances) }));

  const prefersDark = document.documentElement.dataset["theme"] === "dark" ||
    (document.documentElement.dataset["theme"] !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  let theme: ThemeId = prefersDark ? "night" : "paper";
  let page = 0;
  let warmth = 0;
  let dim = 0;

  let ctx: LumenContext;
  try {
    ctx = createLumenContext(canvas, { onRestored: () => void setup().then(draw) });
  } catch (e) {
    status.textContent = `This phone cannot run the reader's renderer (WebGL2): ${String(e)}`;
    status.hidden = false;
    return;
  }
  const renderer = new LumenRenderer(ctx);
  const atlasPixels = await decodeAtlas(data.atlas.png, data.atlas.width, data.atlas.height);
  async function setup(): Promise<void> {
    renderer.setAtlas({ width: data.atlas.width, height: data.atlas.height, data: atlasPixels, pxRange: data.atlas.pxRange });
  }
  await setup();

  function draw(): void {
    if (ctx.state !== "live") return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.round((w * data.pageHeight) / data.pageWidth);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const p = pages[page];
    if (p === undefined) return;
    renderer.resize(w, h);
    renderer.setCamera(orthoCamera(w, h, w / data.pageWidth, 0, 0));
    renderer.setTheme(themeUniforms(theme, { warmth, dim }));
    renderer.setInstances(p.bytes, p.count);
    renderer.render();
    $("device").textContent = `${dpr}× · ${w} × ${h} device px · ${ctx.caps.renderer}`;
  }

  function selectTheme(id: ThemeId): void {
    theme = id;
    applyChrome(id);
    renderSwatches(id);
    for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
      b.setAttribute("aria-checked", String(b.dataset["themeId"] === id));
    }
    draw();
  }

  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-theme-id]")) {
    b.addEventListener("click", () => selectTheme(b.dataset["themeId"] as ThemeId));
  }
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-page]")) {
    b.addEventListener("click", () => {
      page = Number(b.dataset["page"]);
      for (const o of document.querySelectorAll<HTMLButtonElement>("[data-page]")) o.setAttribute("aria-selected", String(o === b));
      draw();
    });
  }
  const warmthInput = $<HTMLInputElement>("warmth");
  const dimInput = $<HTMLInputElement>("dim");
  warmthInput.addEventListener("input", () => {
    warmth = Number(warmthInput.value) / 100;
    $("warmth-value").textContent = warmthInput.value;
    draw();
  });
  dimInput.max = String(Math.round(MAX_DIM * 100));
  dimInput.addEventListener("input", () => {
    dim = Number(dimInput.value) / 100;
    $("dim-value").textContent = `${dimInput.value}%`;
    draw();
  });
  new ResizeObserver(() => draw()).observe(canvas);
  wireChecklist();
  selectTheme(THEME_IDS.includes(theme) ? theme : "paper");
}

void main();
