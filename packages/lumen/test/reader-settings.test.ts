/**
 * The reader settings model (src/reader/settings.ts): the faithful detents and
 * defaults, validation that survives corrupt stored JSON, the two bridges to
 * layout and theme, and a store that persists through a mock storage and
 * degrades silently when storage throws.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import { MAX_DIM } from "../src/gl/themes.ts";
import {
  DEFAULT_SETTINGS,
  GLOBAL_KEY,
  SIZE_STEPS,
  SettingsStore,
  type SettingsStorage,
  defaultsFor,
  effectiveSize,
  loadSettings,
  normalize,
  readerLight,
  snapSize,
  stepSize,
  toReaderStyle,
} from "../src/reader/settings.ts";

/** An in-memory storage; `fail` makes every read and write throw. */
function memoryStorage(fail = false): SettingsStorage & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    read: (k) => {
      if (fail) throw new Error("blocked");
      return map.get(k) ?? null;
    },
    write: (k, v) => {
      if (fail) throw new Error("blocked");
      map.set(k, v);
    },
  };
}

test("size detents: snap to the nearest step, step within the ends", () => {
  assert.equal(snapSize(19), 18, "19 is nearer 18 than 20");
  assert.equal(snapSize(21), 20);
  assert.equal(snapSize(100), 40, "clamps to the top detent");
  assert.equal(snapSize(1), 14, "clamps to the bottom detent");
  assert.equal(snapSize(NaN), DEFAULT_SETTINGS.size, "degrades on a non-number");

  assert.equal(stepSize(20, 1), 22);
  assert.equal(stepSize(20, -1), 18);
  assert.equal(stepSize(40, 1), 40, "cannot step past the largest");
  assert.equal(stepSize(14, -1), 14, "cannot step below the smallest");
  // Every detent is reachable by stepping up from the smallest.
  let size = SIZE_STEPS[0] as number;
  const walked = [size];
  for (let i = 0; i < SIZE_STEPS.length; i++) {
    size = stepSize(size, 1);
    walked.push(size);
  }
  assert.deepEqual([...new Set(walked)], [...SIZE_STEPS]);
});

test("Arabic renders at +2px effective size (optical compensation)", () => {
  assert.equal(effectiveSize(20, "latin"), 20);
  assert.equal(effectiveSize(20, "arabic"), 22);
  const style = toReaderStyle({ ...DEFAULT_SETTINGS, size: 24 });
  assert.deepEqual(style.sizePx, [24, 26], "layout gets [latin, arabic] sizes");
});

test("form-factor and time-of-day defaults (18 phone / 20 desktop, warmth 40 at night)", () => {
  assert.equal(defaultsFor({ phone: true }).size, 18);
  assert.equal(defaultsFor({ phone: false }).size, 20);
  assert.equal(defaultsFor({}).size, 20);
  assert.equal(defaultsFor({ night: true }).warmth, 40);
  assert.equal(defaultsFor({ night: false }).warmth, 0);
});

test("normalize coerces untrusted input and clamps warmth and dim", () => {
  const s = normalize({
    theme: "chartreuse" as never,
    size: 19 as never,
    spacing: "zippy" as never,
    columns: 7 as never,
    flow: "nonsense" as never,
    warmth: 999,
    dim: 5,
  });
  assert.equal(s.theme, DEFAULT_SETTINGS.theme, "an unknown theme falls back");
  assert.equal(s.size, 18, "size snaps to a detent");
  assert.equal(s.spacing, DEFAULT_SETTINGS.spacing);
  assert.equal(s.columns, "auto", "an invalid column count falls back to auto");
  assert.equal(s.flow, "paged");
  assert.equal(s.warmth, 100, "warmth clamps to 100");
  assert.equal(s.dim, MAX_DIM, "dim clamps to MAX_DIM");

  // A valid patch is kept.
  const t = normalize({ theme: "night", columns: 2, warmth: 40, justify: false });
  assert.equal(t.theme, "night");
  assert.equal(t.columns, 2);
  assert.equal(t.warmth, 40);
  assert.equal(t.justify, false);
});

test("spacing presets map to the blueprint leading values", () => {
  assert.equal(toReaderStyle({ ...DEFAULT_SETTINGS, spacing: "compact" }).leading, 1.38);
  assert.equal(toReaderStyle({ ...DEFAULT_SETTINGS, spacing: "comfort" }).leading, 1.5);
  assert.equal(toReaderStyle({ ...DEFAULT_SETTINGS, spacing: "airy" }).leading, 1.7);
});

test("readerLight maps 0–100 warmth to the shader's 0–1 range", () => {
  assert.deepEqual(readerLight({ ...DEFAULT_SETTINGS, warmth: 50, dim: 0.3 }), { warmth: 0.5, dim: 0.3 });
});

test("the store persists a change and reloads it from storage", () => {
  const storage = memoryStorage();
  const a = new SettingsStore({ storage });
  assert.deepEqual(a.value, DEFAULT_SETTINGS);

  a.set({ theme: "night", size: 27 });
  a.stepSize(1);
  assert.equal(a.value.size, 30, "stepped one detent past 27");
  assert.ok(storage.map.has(GLOBAL_KEY), "the change was written");

  // A fresh store over the same storage sees the persisted settings.
  const b = new SettingsStore({ storage });
  assert.equal(b.value.theme, "night");
  assert.equal(b.value.size, 30);
  // And the loader agrees.
  assert.equal(loadSettings(storage).theme, "night");
});

test("the store notifies subscribers and supports reset", () => {
  const store = new SettingsStore();
  const seen: string[] = [];
  const unsubscribe = store.subscribe((s) => seen.push(s.theme));
  assert.deepEqual(seen, ["paper"], "subscriber is called immediately with the current value");

  store.set({ theme: "dusk" });
  assert.deepEqual(seen, ["paper", "dusk"]);

  store.reset();
  assert.deepEqual(store.value, DEFAULT_SETTINGS);
  unsubscribe();
  store.set({ theme: "oled" });
  assert.deepEqual(seen, ["paper", "dusk", "paper"], "no notification after unsubscribe");
});

test("a throwing storage never breaks the reader (private window)", () => {
  const storage = memoryStorage(true);
  // Construction falls back to defaults rather than throwing on a blocked read.
  const store = new SettingsStore({ storage });
  assert.deepEqual(store.value, DEFAULT_SETTINGS);
  // A set still applies in memory even though the write is swallowed.
  const next = store.set({ theme: "linen" });
  assert.equal(next.theme, "linen");
  assert.equal(store.value.theme, "linen");
});
