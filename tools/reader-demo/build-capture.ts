/**
 * Builds the Capture Lab (temporal-multiplexing PoC) into a self-contained
 * page, reusing the reader-demo's fixtures (the sealed canary chunk, its dev
 * key, the atlas page, the glyph table and the committed wasm compositor).
 *
 *   node build-capture.ts
 *   dist/capture-lab.html             page content
 *   dist/capture-lab.standalone.html  a complete document (used by the headless
 *                                     measurement harness)
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const dist = join(here, "dist");
const folio = join(repo, "fixtures/folio");
const wasm = join(repo, "packages/lumen-wasm/pkg/sanad_compositor_bg.wasm");

if (!existsSync(join(folio, "canary-chunk0-v0.folio")) || !existsSync(join(folio, "canary-atlas0.png"))) {
  execFileSync("cargo", ["run", "-q", "-p", "sanad-atelier", "--example", "seal_reader_fixture"], {
    cwd: repo,
    stdio: ["ignore", "inherit", "inherit"],
  });
}

const meta = JSON.parse(readFileSync(join(folio, "canary-chunk0-v0.expected.json"), "utf8")) as {
  editionId: string;
  chunkKeyHex: string;
};
const glyphs = JSON.parse(readFileSync(join(folio, "canary-glyphs.json"), "utf8")) as {
  pxRange: number;
  width: number;
  height: number;
};

const data = {
  editionId: meta.editionId,
  chunkKeyHex: meta.chunkKeyHex,
  chunk: readFileSync(join(folio, "canary-chunk0-v0.folio")).toString("base64"),
  wasm: readFileSync(wasm).toString("base64"),
  atlas: {
    png: `data:image/png;base64,${readFileSync(join(folio, "canary-atlas0.png")).toString("base64")}`,
    width: glyphs.width,
    height: glyphs.height,
    pxRange: glyphs.pxRange,
  },
  glyphs: JSON.parse(readFileSync(join(folio, "canary-glyphs.json"), "utf8")),
};

function page(body: { data: string; script: string }): string {
  return `<title>Sanad Capture Lab</title>
<style>
:root { color-scheme: light; --ink: #1a1a17; --paper: #f7f4ec; --line: #ddd8cc; --accent: #7a5cff; }
body { background: #e9e5db; color: var(--ink); font: 400 14px/1.5 system-ui, sans-serif; margin: 0; padding: 0 16px 48px; }
.intro { max-width: 70ch; padding: 16px 0; }
.intro h1 { margin: 0 0 4px; font-size: 1.25rem; }
.intro p { margin: 4px 0; color: #55514a; font-size: 0.85rem; }
.warn { border-inline-start: 3px solid #b26b00; background: #fff6e6; padding: 8px 12px; border-radius: 6px; color: #5c3d00; }
.layout { display: grid; gap: 20px; grid-template-columns: minmax(0,1fr) 280px; align-items: start; margin-top: 12px; }
@media (max-width: 820px){ .layout { grid-template-columns: 1fr; } }
#page { display: block; width: 100%; max-width: 620px; border: 1px solid var(--line); border-radius: 8px; background: var(--paper); }
.panel { display: grid; gap: 16px; }
.panel h2 { margin: 0 0 6px; font-size: 0.95rem; }
.row { display: grid; grid-template-columns: 5.5rem 1fr 3rem; gap: 10px; align-items: center; min-height: 38px; }
.row output { text-align: end; color: #55514a; font-variant-numeric: tabular-nums; }
input[type="range"]{ width: 100%; accent-color: var(--accent); }
label.sw { display: inline-flex; gap: 8px; align-items: center; font-weight: 600; }
.note { color: #55514a; font-size: 0.78rem; }
#status { padding: 12px; border-radius: 8px; background: #fff; }
</style>
<header class="intro">
  <h1>Sanad Capture Lab — temporal multiplexing</h1>
  <p>The real page (decoded from a sealed Folio chunk, laid out by the Compositor, drawn by Lumen) is presented as several complementary sub-frames per cycle. Your eye integrates them into clean text; a single captured frame is missing a share of the ink.</p>
  <p class="warn">Honest PoC. This does <strong>not</strong> block capture: a screen <em>recording</em> can average the sub-frames back. It flickers, with a real accessibility cost (photosensitivity, eye strain). It is here to measure the trade-off, not to ship by default.</p>
</header>
<main class="layout">
  <section>
    <p id="status" hidden></p>
    <canvas id="page" width="620" height="860" aria-label="A page presented with temporal multiplexing"></canvas>
  </section>
  <aside class="panel">
    <section>
      <h2>Temporal multiplex</h2>
      <div class="row"><label class="sw" for="tm">On</label><input id="tm" type="checkbox" checked><span></span></div>
      <div class="row"><label for="frames">Sub-frames</label><input id="frames" type="range" min="2" max="6" step="1" value="3"><output id="frames-value">3</output></div>
      <div class="row"><label for="duty">Frag duty</label><input id="duty" type="range" min="20" max="90" step="5" value="50"><output id="duty-value">50%</output></div>
    </section>
    <section>
      <h2>What this shows</h2>
      <p class="note">Live, the text reads cleanly (your eye sums the sub-frames). A screenshot captures one sub-frame — with <em>ink duty</em> of the ink missing, in a per-pixel pattern. Lower duty and more sub-frames degrade a single frame more, at the cost of more flicker and fainter text.</p>
    </section>
  </aside>
</main>
<script type="application/json" id="reader-data">${body.data}</script>
<script type="module">${body.script}</script>`;
}

const bundle = await build({
  entryPoints: [join(here, "src/capture-lab.ts")],
  bundle: true,
  write: false,
  format: "esm",
  target: "es2022",
  minify: true,
  legalComments: "none",
});
const script = (bundle.outputFiles[0]?.text ?? "").replace(/<\/script/giu, "<\\/script");
const json = JSON.stringify(data).replace(/<\//gu, "<\\/");

const body = page({ data: json, script });
mkdirSync(dist, { recursive: true });
writeFileSync(join(dist, "capture-lab.html"), body);
writeFileSync(
  join(dist, "capture-lab.standalone.html"),
  `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">${body.slice(0, body.indexOf("<header"))}</head><body>${body.slice(body.indexOf("<header"))}</body></html>`,
);
console.log(`capture-lab: ${(Buffer.byteLength(body) / 1024 / 1024).toFixed(2)} MiB → ${join(dist, "capture-lab.html")}`);
