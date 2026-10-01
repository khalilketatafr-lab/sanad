/**
 * Real-GPU test of Lumen's WebGL2 context and shaders in headless Chromium.
 * Proves on actual rasterization hardware (ANGLE/SwiftShader) what the Rust
 * property tests prove numerically: the MAX-blended union of shredded glyph
 * fragments is pixel-identical to the whole glyph, while additive blending
 * produces visible seams.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium, type Browser } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");

interface Diff {
  maxDiff: number;
  diffPixels: number;
}
interface Result {
  renderer: string;
  attributes: WebGLContextAttributes;
  scales: Record<string, { maxUnion: Diff; additive: Diff; singleFragment: Diff; inkMass: number }>;
  weight: { thinner: number; neutral: number; bolder: number };
  paperY: number;
  chrome: Record<string, { paper: number[]; expectedPaper: number[]; inkPixels: number; expectedInk: number[] }>;
  laterReadback: { distinctColors: number; firstPixel: number[]; inFrameDistinctColors: number };
  loss: { simulated: boolean; stateAfterLoss: string; stateAfterRestore: string; restoredCalls: number };
}

let browser: Browser;
let result: Result;

before(async () => {
  const out = mkdtempSync(join(tmpdir(), "lumen-atlas-"));
  execFileSync("cargo", ["run", "-q", "-p", "sanad-atelier", "--example", "shred_fixture", "--", out], { cwd: repo, stdio: "inherit" });
  const meta = JSON.parse(readFileSync(join(out, "atlas.json"), "utf8")) as Record<string, unknown>;
  const atlas = readFileSync(join(out, "atlas.rgba")).toString("base64");

  const bundle = await build({
    entryPoints: [join(here, "gl-harness.ts")],
    bundle: true,
    write: false,
    format: "iife",
    globalName: "LumenHarness",
    target: "es2022",
  });
  const code = bundle.outputFiles[0]?.text ?? "";

  browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
  const page = await browser.newPage();
  page.on("console", (m) => {
    if (m.type() === "error") console.error(`[page] ${m.text()}`);
  });
  await page.setContent("<!doctype html><html><body></body></html>");
  await page.addScriptTag({ content: code });
  result = (await page.evaluate(
    ([m, a]) => (globalThis as unknown as { LumenHarness: { run: (m: unknown, a: string) => Promise<unknown> } }).LumenHarness.run(m, a),
    [meta, atlas] as const,
  )) as Result;
  if (process.env["LUMEN_TEST_VERBOSE"] === "1") console.log(JSON.stringify(result, null, 2));
});

after(async () => {
  await browser.close();
});

test("context honors the P1 rendering contract", () => {
  assert.equal(result.attributes.preserveDrawingBuffer, false);
  assert.equal(result.attributes.antialias, false);
  assert.equal(result.attributes.alpha, false);
});

test("MAX-union of shredded fragments is bit-identical to the whole glyph", () => {
  for (const [scale, s] of Object.entries(result.scales)) {
    assert.ok(s.inkMass > 0, `${scale}: glyph must render`);
    assert.equal(s.maxUnion.maxDiff, 0, `${scale}: union differs by ${s.maxUnion.maxDiff}/255 on ${s.maxUnion.diffPixels} px`);
  }
});

test("additive blending of the same fragments shows seams (why MAX is required)", () => {
  for (const [scale, s] of Object.entries(result.scales)) {
    assert.ok(s.additive.maxDiff >= 16, `${scale}: expected visible seams, max diff ${s.additive.maxDiff}`);
  }
});

test("a single fragment is not the glyph (shredding actually removes ink)", () => {
  for (const [scale, s] of Object.entries(result.scales)) {
    assert.ok(s.singleFragment.diffPixels > 50, `${scale}: one fragment differs from the glyph on only ${s.singleFragment.diffPixels} px`);
  }
});

test("u_weightPx compensates stroke weight monotonically", () => {
  const { thinner, neutral, bolder } = result.weight;
  assert.ok(thinner < neutral && neutral < bolder, JSON.stringify(result.weight));
});

test("u_lumaCeil clamps luminance (anti-glare)", () => {
  assert.ok(Math.abs(result.paperY - 0.45) < 0.01, `paper Y ${result.paperY}`);
});

test("every theme renders exactly its CSS chrome colors (@sanad/tokens)", () => {
  assert.equal(Object.keys(result.chrome).length, 5);
  for (const [id, c] of Object.entries(result.chrome)) {
    c.paper.forEach((v, i) => assert.ok(Math.abs(v - (c.expectedPaper[i] ?? -9)) <= 1, `${id}: paper ${c.paper} ≠ ${c.expectedPaper}`));
    assert.ok(c.inkPixels > 500, `${id}: only ${c.inkPixels} px render the CSS ink ${c.expectedInk}`);
  }
});

test("context loss and restore are handled", () => {
  assert.equal(result.loss.simulated, true);
  assert.equal(result.loss.stateAfterLoss, "lost");
  assert.equal(result.loss.stateAfterRestore, "live");
  assert.equal(result.loss.restoredCalls, 1);
});

test("drawing buffer is not readable after the frame (preserveDrawingBuffer=false)", () => {
  assert.ok(result.laterReadback.inFrameDistinctColors > 2, "sanity: the in-frame readback showed the page");
  assert.equal(result.laterReadback.distinctColors, 1, "page pixels survived past the frame");
  assert.deepEqual(result.laterReadback.firstPixel, [0, 0, 0, 255], "expected the cleared (black, opaque) buffer");
});
