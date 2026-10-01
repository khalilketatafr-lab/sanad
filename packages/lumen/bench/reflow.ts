/**
 * Reflow benchmark (roadmap §10.4, gate G0: "reflow of the visible spread
 * ≤ 16 ms on tier B"). Times the Compositor's WASM build in headless
 * Chromium at native speed and under CPU throttling (4× approximates a
 * mid-range Android, the tier B reference; 6× a low-end one). Prints the
 * full-fixture time and the cost of a 50-line, two-page spread. Exits 1 if
 * the spread p95 exceeds 16 ms at 4×.
 *
 *   pnpm --filter @sanad/lumen bench:reflow
 */
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";
import { generateReflowFixture } from "../test/fixtures/reflow.ts";
import type { ReflowTiming } from "./reflow-harness.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const SPREAD_LINES = 50;
const BUDGET_MS = 16;
const ROUNDS = 40;

const fixture = generateReflowFixture(repo);
const wasm = readFileSync(fileURLToPath(import.meta.resolve("@sanad/lumen-wasm/sanad_compositor_bg.wasm"))).toString("base64");
const bundle = await build({
  entryPoints: [join(here, "reflow-harness.ts")],
  bundle: true,
  write: false,
  format: "iife",
  globalName: "ReflowBench",
  target: "es2022",
  logLevel: "error",
});
const code = bundle.outputFiles[0]?.text ?? "";

const pct = (s: readonly number[], p: number): number => s[Math.min(s.length - 1, Math.round((s.length - 1) * p))] ?? Number.NaN;
const browser = await chromium.launch();
let worst = 0;
try {
  console.log(`fixture: ${fixture.paragraphs.length} paragraphs, ${fixture.paragraphs.reduce((n, p) => n + p.gids.length, 0)} glyphs, measures ${fixture.reference.map((r) => r.width).join("/")} px`);
  console.log("CPU      full reflow p50 / p95        per line   50-line spread p95");
  for (const rate of [1, 4, 6]) {
    const page = await browser.newPage();
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate });
    await page.setContent("<!doctype html><html><body></body></html>");
    await page.addScriptTag({ content: code });
    const t = (await page.evaluate(
      ([w, f, r]) => (globalThis as unknown as { ReflowBench: { run: (w: string, f: unknown, r: number) => unknown } }).ReflowBench.run(w, f, r),
      [wasm, fixture, ROUNDS] as const,
    )) as ReflowTiming;
    const perLine = pct(t.samples, 0.5) / t.linesPerReflow;
    const spreadP95 = (pct(t.samples, 0.95) / t.linesPerReflow) * SPREAD_LINES;
    if (rate === 4) worst = spreadP95;
    console.log(
      `${`${rate}×`.padEnd(8)} ${pct(t.samples, 0.5).toFixed(2).padStart(7)} / ${pct(t.samples, 0.95).toFixed(2).padStart(7)} ms   ${(perLine * 1000).toFixed(1).padStart(6)} µs   ${spreadP95.toFixed(2).padStart(7)} ms`,
    );
    await page.close();
  }
} finally {
  await browser.close();
}
const ok = worst <= BUDGET_MS;
console.log(`tier B (4×) spread p95 ${worst.toFixed(2)} ms vs budget ${BUDGET_MS} ms: ${ok ? "PASS" : "FAIL"}`);
process.exitCode = ok ? 0 : 1;
