/**
 * Runs in Chromium: times the Compositor's WASM build reflowing the §10.4
 * fixture (20 paragraphs) at each fixture measure.
 */
import { Compositor, initSync } from "@sanad/lumen-wasm";
import { layout, prepare, type ReflowFixture } from "../test/fixtures/reflow-layout.ts";

export interface ReflowTiming {
  /** Milliseconds per full reflow (all 20 paragraphs at one measure), sorted. */
  readonly samples: number[];
  /** Lines laid out per reflow, averaged over the measures. */
  readonly linesPerReflow: number;
}

export function run(wasmBase64: string, fixture: ReflowFixture, rounds: number): ReflowTiming {
  initSync({ module: Uint8Array.from(atob(wasmBase64), (ch) => ch.charCodeAt(0)) });
  const paragraphs = fixture.paragraphs.map(prepare);
  const widths = fixture.reference.map((r) => r.width);
  const c = new Compositor();
  // Warm-up: JIT tiers and wasm compilation.
  for (let i = 0; i < 5; i++) for (const w of widths) for (const p of paragraphs) layout(c, p, w);
  const samples: number[] = [];
  let lines = 0;
  for (let r = 0; r < rounds; r++) {
    for (const w of widths) {
      const t0 = performance.now();
      for (const p of paragraphs) lines += layout(c, p, w);
      samples.push(performance.now() - t0);
    }
  }
  samples.sort((a, b) => a - b);
  return { samples, linesPerReflow: lines / Math.max(1, samples.length) };
}
