/**
 * Builds the theme review page (roadmap §10.6): runs Atelier's golden-page
 * pipeline, encodes the shredded atlas as a lossless PNG, bundles the review
 * app with Lumen and the tokens, and writes one self-contained HTML file:
 *
 *   dist/theme-review.html             page content (the published artifact)
 *   dist/theme-review.standalone.html  the same, as a complete document
 *
 *   pnpm --filter @sanad/theme-review build
 *
 * Like every client artifact, the page carries only permuted glyph ids and a
 * shredded atlas: no text (P1).
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";
import { build } from "esbuild";
import { THEMES, type CssVar, type ThemeId } from "@sanad/tokens/ts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const dist = join(here, "dist");

interface GoldenMeta {
  readonly width: number;
  readonly height: number;
  readonly atlas: { readonly file: string; readonly width: number; readonly height: number; readonly pxRange: number };
  readonly pages: readonly { readonly name: string; readonly file: string; readonly count: number }[];
}

/** Minimal lossless PNG encoder: RGBA8, no filtering, zlib level 9. */
function encodePng(rgba: Uint8Array, width: number, height: number): Buffer {
  const chunk = (type: string, body: Buffer): Buffer => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(body.length);
    const typed = Buffer.concat([Buffer.from(type, "ascii"), body]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(typed) >>> 0);
    return Buffer.concat([len, typed, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([8, 6, 0, 0, 0], 8); // 8-bit, RGBA, deflate, no filter, no interlace
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) raw.set(rgba.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const LABELS: Readonly<Record<string, string>> = {
  latin: "Latin",
  arabic: "Arabic",
  mixed: "Mixed",
};

function vars(id: ThemeId): string {
  return Object.entries(THEMES[id].css)
    .map(([k, v]) => `${k}: ${v};`)
    .join(" ");
}

const themeButtons = (["paper", "linen", "dusk", "night", "oled"] as const)
  .map((id) => {
    const css = THEMES[id].css;
    const sw = (v: CssVar): string => css[v];
    return `<button type="button" role="radio" aria-checked="false" data-theme-id="${id}" class="theme">
        <span class="theme-swatch" style="background:${sw("--paper")};color:${sw("--ink")}" aria-hidden="true">Aa</span>
        <span>${THEMES[id].label}</span>
      </button>`;
  })
  .join("");

function page(content: { data: string; script: string; tabs: string }): string {
  return `<title>Marginalia Theme Review</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&family=Literata:opsz,wght@7..72,400;7..72,600&display=swap">
<style>
/* Layout: the page under review on the left (phone: on top), the instruments
   beside it. Every color is a Marginalia token; picking a reading theme
   repaints the whole page with that theme. */
:root {
  ${vars("paper")}
  --font-display: "Literata", Georgia, serif;
  --font-ui: "Inter", "IBM Plex Sans Arabic", system-ui, sans-serif;
  --pass: #2e6b3f;
  --fail: #9a2c1f;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) { ${vars("night")} --pass: #8fc29b; --fail: #e59a8e; color-scheme: dark; }
}
:root[data-theme="dark"] { ${vars("night")} --pass: #8fc29b; --fail: #e59a8e; color-scheme: dark; }
:root[data-reading="dusk"], :root[data-reading="night"], :root[data-reading="oled"] { --pass: #8fc29b; --fail: #e59a8e; }
:root[data-reading="paper"], :root[data-reading="linen"] { --pass: #2e6b3f; --fail: #9a2c1f; }

body {
  background: var(--paper);
  color: var(--ink);
  font: 400 15px/1.5 var(--font-ui);
  padding-inline: 16px;
  padding-block: 0 48px;
}
.intro { padding-block: 14px 2px; }
.intro h1 { margin: 0; font: 600 1.25rem/1.2 var(--font-display); text-wrap: balance; }
.intro p { margin: 0; color: var(--ink-2); font-size: 0.8125rem; }
/* Only the theme switch stays pinned: on a phone the page needs the height. */
.bar {
  position: sticky;
  top: env(safe-area-inset-top, 0px);
  z-index: 2;
  padding-block: 10px;
  background: var(--paper);
  border-bottom: 1px solid var(--hairline);
}
.themes { display: flex; flex-wrap: wrap; gap: 8px; }
.theme {
  display: inline-flex; align-items: center; gap: 8px;
  min-block-size: 44px; padding: 4px 12px 4px 4px;
  border: 1px solid var(--hairline); border-radius: 22px;
  background: var(--surface-1); color: var(--ink);
  font: 500 0.875rem var(--font-ui); cursor: pointer;
}
.theme[aria-checked="true"] { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
.theme-swatch {
  display: grid; place-items: center; inline-size: 34px; block-size: 34px;
  border-radius: 50%; border: 1px solid var(--hairline);
  font: 600 0.8125rem var(--font-display);
}
button:focus-visible, input:focus-visible { outline: none; box-shadow: var(--focus-ring); }

.layout {
  display: grid; gap: 24px; margin-block-start: 16px;
  grid-template-columns: minmax(0, 480px) minmax(0, 1fr);
  align-items: start;
}
@media (max-width: 860px) { .layout { grid-template-columns: minmax(0, 1fr); } }

.stage { display: grid; gap: 10px; min-width: 0; }
.tabs { display: flex; gap: 4px; }
.tabs button {
  min-block-size: 40px; padding: 0 14px; border: 0; border-radius: 20px;
  background: transparent; color: var(--ink-2); font: 500 0.875rem var(--font-ui); cursor: pointer;
}
.tabs button[aria-selected="true"] { background: var(--surface-2); color: var(--ink); }
.canvas-wrap { position: relative; max-width: 480px; }
#page {
  display: block; inline-size: 100%; aspect-ratio: 2 / 3; max-width: 100%;
  border: 1px solid var(--hairline); border-radius: 6px; background: var(--paper);
}
.glass {
  position: absolute; inset-inline: 12px; inset-block-end: 12px;
  display: grid; gap: 10px; padding: 14px 16px;
  background: var(--glass-tint);
  -webkit-backdrop-filter: blur(24px) saturate(140%);
  backdrop-filter: blur(24px) saturate(140%);
  border: 1px solid var(--hairline); border-radius: 20px;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.06), 0 12px 40px rgb(0 0 0 / 0.18);
}
@media (prefers-reduced-transparency: reduce) { .glass { background: var(--surface-1); backdrop-filter: none; -webkit-backdrop-filter: none; } }
.glass-row { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.glass strong { font-weight: 600; }
.glass span { color: var(--ink-2-on-glass); font-size: 0.875rem; }
.glass .demo-focus {
  min-block-size: 36px; padding: 0 14px; border: 0; border-radius: 18px;
  background: var(--surface-2); color: var(--ink); font: 500 0.875rem var(--font-ui);
  box-shadow: var(--focus-ring);
}
.meta { margin: 0; color: var(--ink-2); font-size: 0.75rem; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
#status { padding: 12px; border-radius: 8px; background: var(--surface-2); }

.panel { display: grid; gap: 28px; min-width: 0; }
.panel h2 { margin: 0 0 10px; font: 600 1rem/1.3 var(--font-display); }
.panel p { margin: 0 0 10px; max-width: 62ch; color: var(--ink-2); font-size: 0.875rem; }
.control { display: grid; grid-template-columns: 7rem minmax(0, 1fr) 3.5rem; align-items: center; gap: 12px; min-block-size: 44px; }
.control label { font-weight: 500; }
.control output { text-align: end; font-variant-numeric: tabular-nums; color: var(--ink-2); }
input[type="range"] { inline-size: 100%; accent-color: var(--accent); }
.toggle { display: flex; align-items: center; gap: 10px; min-block-size: 44px; }

ul.pairs, ul.swatches, ul.checks { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.pair { display: grid; grid-template-columns: 40px minmax(0, 1fr) auto; align-items: center; gap: 12px; min-block-size: 40px; }
.chip { display: grid; place-items: center; inline-size: 40px; block-size: 32px; border-radius: 6px; border: 1px solid var(--hairline); font: 600 0.9375rem var(--font-display); }
.pair-label { font-size: 0.875rem; }
.pair-ratio { font: 600 0.875rem var(--font-ui); font-variant-numeric: tabular-nums; }
.pair-ratio small { font-weight: 400; color: var(--ink-2); margin-inline-start: 4px; }
.pair-ratio.pass { color: var(--pass); }
.pair-ratio.fail { color: var(--fail); }
ul.swatches { grid-template-columns: repeat(auto-fill, minmax(9.5rem, 1fr)); gap: 10px; }
.swatch { display: grid; grid-template-columns: 28px minmax(0, 1fr); column-gap: 10px; align-items: center; }
.swatch-fill { grid-row: span 2; inline-size: 28px; block-size: 28px; border-radius: 50%; border: 1px solid var(--hairline); }
.swatch-name { font-size: 0.8125rem; font-weight: 500; }
.swatch code { font: 0.75rem ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--ink-2); }
.checks li { display: grid; grid-template-columns: 24px minmax(0, 1fr); gap: 10px; align-items: start; }
.checks input { inline-size: 20px; block-size: 20px; margin-block-start: 2px; accent-color: var(--accent); }
.checks .where { display: block; color: var(--ink-2); font-size: 0.8125rem; }
@media (prefers-reduced-motion: no-preference) { body, .bar, .theme, .chip, .swatch-fill { transition: background-color 240ms, color 240ms; } }
</style>

<header class="intro">
  <h1>Marginalia Theme Review</h1>
  <p>Golden pages drawn by the reader's own renderer on this screen. Review each theme in a dark room and in daylight.</p>
</header>
<div class="bar"><div class="themes" role="radiogroup" aria-label="Reading theme">${themeButtons}</div></div>

<main class="layout">
  <section class="stage" aria-label="Page under review">
    <div class="tabs" role="tablist" aria-label="Golden page">${content.tabs}</div>
    <div class="canvas-wrap">
      <canvas id="page" width="480" height="720" aria-label="Golden page rendered by Lumen"></canvas>
      <div class="glass" id="glass">
        <div class="glass-row"><strong>Theme</strong><span>Follows the time of day</span></div>
        <div class="glass-row"><strong>Text size</strong><button type="button" class="demo-focus" tabindex="-1">Focused control</button></div>
      </div>
    </div>
    <p class="meta" id="device">Starting the renderer…</p>
    <p id="status" hidden></p>
  </section>

  <aside class="panel">
    <section aria-labelledby="light-h">
      <h2 id="light-h">Light</h2>
      <p>Reader settings, applied by the same shader as in the app. Night reading usually runs warmth 40.</p>
      <div class="control"><label for="warmth">Warmth</label><input id="warmth" type="range" min="0" max="100" step="1" value="0"><output id="warmth-value" for="warmth">0</output></div>
      <div class="control"><label for="dim">Extra-dim</label><input id="dim" type="range" min="0" max="60" step="1" value="0"><output id="dim-value" for="dim">0%</output></div>
      <label class="toggle" for="show-glass"><input id="show-glass" type="checkbox" checked> Show the settings sheet over the page</label>
    </section>

    <section aria-labelledby="contrast-h">
      <h2 id="contrast-h">Contrast floors</h2>
      <p>WCAG 2.x ratios for this theme against the floors the token build enforces. Worst-case glass is the sheet over a pure black or pure white page.</p>
      <ul class="pairs" id="pairs"></ul>
    </section>

    <section aria-labelledby="palette-h">
      <h2 id="palette-h">Palette</h2>
      <ul class="swatches" id="swatches"></ul>
    </section>

    <section id="checklist" aria-labelledby="check-h">
      <h2 id="check-h">Review checklist</h2>
      <p>Saved on this phone only. Note the phone model with the device line under the page.</p>
      <ul class="checks">
        <li><input type="checkbox" id="c-dark-bloom"><label for="c-dark-bloom">Light text does not bloom or look bold<span class="where">Dusk, Night, OLED · dark room, low brightness</span></label></li>
        <li><input type="checkbox" id="c-oled"><label for="c-oled">OLED ink is calm against true black, with no glow around strokes<span class="where">OLED · dark room</span></label></li>
        <li><input type="checkbox" id="c-dim"><label for="c-dim">At extra-dim 60% and warmth 40 the page is still comfortable to read<span class="where">Night · dark room</span></label></li>
        <li><input type="checkbox" id="c-day"><label for="c-day">Paper does not glare and ink looks solid, not thin<span class="where">Paper, Linen · daylight, full brightness</span></label></li>
        <li><input type="checkbox" id="c-arabic"><label for="c-arabic">Arabic joins cleanly and kashida looks even, not stretched<span class="where">Arabic page · every theme</span></label></li>
        <li><input type="checkbox" id="c-glass"><label for="c-glass">Sheet labels stay readable over text and the focus ring is obvious<span class="where">Settings sheet · every theme</span></label></li>
        <li><input type="checkbox" id="c-seam"><label for="c-seam">The page and the area around it are the same paper color, with no visible edge<span class="where">Every theme</span></label></li>
      </ul>
    </section>
  </aside>
</main>
<script id="review-data" type="application/json">${content.data}</script>
<script>${content.script}</script>
<script>
  document.getElementById("show-glass").addEventListener("change", (e) => {
    document.getElementById("glass").hidden = !e.target.checked;
  });
</script>
`;
}

const out = mkdtempSync(join(tmpdir(), "theme-review-"));
execFileSync("cargo", ["run", "-q", "-p", "sanad-atelier", "--example", "golden_pages", "--", out], { cwd: repo, stdio: ["ignore", "ignore", "inherit"] });
const meta = JSON.parse(readFileSync(join(out, "pages.json"), "utf8")) as GoldenMeta;
const atlas = readFileSync(join(out, meta.atlas.file));
const png = encodePng(atlas, meta.atlas.width, meta.atlas.height);

const data = {
  pageWidth: meta.width,
  pageHeight: meta.height,
  atlas: { png: `data:image/png;base64,${png.toString("base64")}`, width: meta.atlas.width, height: meta.atlas.height, pxRange: meta.atlas.pxRange },
  pages: meta.pages.map((p) => ({
    name: p.name,
    label: LABELS[p.name] ?? p.name,
    count: p.count,
    instances: readFileSync(join(out, p.file)).toString("base64"),
  })),
};
const tabs = data.pages
  .map((p, i) => `<button type="button" role="tab" data-page="${i}" aria-selected="${i === 0}">${p.label}</button>`)
  .join("");

const bundle = await build({
  entryPoints: [join(here, "src/app.ts")],
  bundle: true,
  write: false,
  format: "iife",
  target: "es2022",
  minify: true,
  legalComments: "none",
});
const script = (bundle.outputFiles[0]?.text ?? "").replace(/<\/script/giu, "<\\/script");
// JSON inside <script>: neutralize "</" so no value can close the element.
const json = JSON.stringify(data).replace(/<\//gu, "<\\/");

const body = page({ data: json, script, tabs });
mkdirSync(dist, { recursive: true });
writeFileSync(join(dist, "theme-review.html"), body);
writeFileSync(
  join(dist, "theme-review.standalone.html"),
  `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">${body.slice(0, body.indexOf("<header"))}</head><body>${body.slice(body.indexOf("<header"))}</body></html>`,
);
const size = (Buffer.byteLength(body) / 1024 / 1024).toFixed(2);
console.log(`theme-review: atlas PNG ${(png.length / 1024).toFixed(0)} KiB, page ${size} MiB → ${join(dist, "theme-review.html")}`);
