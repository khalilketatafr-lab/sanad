/**
 * Golden pages (Phase 0 gate G0, Definition of Done): Latin justified with
 * hyphenation, Arabic with kashida, and mixed bidi, rendered by the
 * production renderer from real fonts through the full Atelier pipeline
 * (shape → permute → MSDF → shred → atlas → Compositor), in all 5 themes.
 *
 * 1. Shredded rendering is pixel-identical to rendering whole glyphs, in
 *    every theme.
 * 2. Pages match the committed golden PNGs (fixtures/golden) in one light
 *    and one dark theme. The other themes differ only in Pass 3 uniforms,
 *    which webgl.test.ts checks pixel-exactly against the CSS chrome.
 *
 * All 15 renders are written to test/out/golden/ for design review.
 * `UPDATE_GOLDEN=1` rewrites fixtures/golden from the current rendering.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium, type Browser } from "playwright";
import type { PageResult } from "./golden-harness.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const goldenDir = join(repo, "fixtures/golden");
const reviewDir = join(here, "out/golden");
const ORIGIN = "http://golden.sanad.test";
const update = process.env["UPDATE_GOLDEN"] === "1";

/** SwiftShader is deterministic per build; allow last-bit noise across CPUs. */
const MAX_CHANNEL_DIFF = 2;
const MAX_DIFF_FRACTION = 0.002;
const GOLDEN_THEMES: readonly string[] = ["paper", "night"];

let browser: Browser;
let results: PageResult[];

before(async () => {
  const out = mkdtempSync(join(tmpdir(), "lumen-golden-"));
  execFileSync("cargo", ["run", "-q", "-p", "sanad-atelier", "--example", "golden_pages", "--", out], { cwd: repo, stdio: "inherit" });
  const bundle = await build({
    entryPoints: [join(here, "golden-harness.ts")],
    bundle: true,
    write: false,
    format: "iife",
    globalName: "LumenGolden",
    target: "es2022",
  });
  const code = bundle.outputFiles[0]?.text ?? "";

  browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] });
  const page = await browser.newPage();
  page.on("console", (m) => {
    if (m.type() === "error") console.error(`[page] ${m.text()}`);
  });
  await page.route(`${ORIGIN}/**`, async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/") return route.fulfill({ contentType: "text/html", body: `<!doctype html><html><body><script>${code}</script></body></html>` });
    const file = path.startsWith("/golden/") ? join(out, path.slice("/golden/".length)) : path.startsWith("/fixtures/golden/") ? join(goldenDir, path.slice("/fixtures/golden/".length)) : null;
    if (file === null || !existsSync(file)) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ body: readFileSync(file) });
  });
  await page.goto(`${ORIGIN}/`);
  results = (await page.evaluate(() => (globalThis as unknown as { LumenGolden: { run: () => Promise<unknown> } }).LumenGolden.run())) as PageResult[];

  mkdirSync(reviewDir, { recursive: true });
  if (update) mkdirSync(goldenDir, { recursive: true });
  for (const r of results) {
    const png = Buffer.from(r.png, "base64");
    writeFileSync(join(reviewDir, `${r.page}-${r.theme}.png`), png);
    if (r.unionDiffPng !== null) writeFileSync(join(reviewDir, `${r.page}-${r.theme}.union-diff.png`), Buffer.from(r.unionDiffPng, "base64"));
    if (update && GOLDEN_THEMES.includes(r.theme)) writeFileSync(join(goldenDir, `${r.page}-${r.theme}.png`), png);
  }
});

after(async () => {
  await browser.close();
});

test("3 pages × 5 themes rendered, each with real text on it", () => {
  assert.equal(results.length, 15);
  for (const r of results) assert.ok(r.inkPixels > 20_000, `${r.page}/${r.theme}: only ${r.inkPixels} ink pixels`);
});

test("shredded glyphs reassemble pixel-identically on real pages (MAX blend)", () => {
  for (const r of results) {
    assert.equal(r.union.maxDiff, 0, `${r.page}/${r.theme}: shredded ≠ whole by ${r.union.maxDiff}/255 on ${r.union.diffPixels} px`);
  }
});

test("pages match the committed goldens", { skip: update ? "UPDATE_GOLDEN=1: goldens rewritten" : false }, () => {
  for (const r of results.filter((x) => GOLDEN_THEMES.includes(x.theme))) {
    assert.ok(r.golden !== null, `missing fixtures/golden/${r.page}-${r.theme}.png (run with UPDATE_GOLDEN=1 and review)`);
    assert.ok(r.golden.maxDiff <= MAX_CHANNEL_DIFF, `${r.page}/${r.theme}: max diff ${r.golden.maxDiff}/255 on ${r.golden.diffPixels} px`);
    assert.ok(r.golden.diffPixels <= MAX_DIFF_FRACTION * r.inkPixels, `${r.page}/${r.theme}: ${r.golden.diffPixels} px differ`);
  }
});
