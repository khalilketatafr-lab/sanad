/**
 * Builds the real Style Dictionary pipeline into a temporary directory and
 * checks the outputs, the design guarantees (contrast floors, glass
 * legibility, focus visibility), agreement with the blueprint tables, and that
 * malformed tokens fail the build instead of shipping.
 */
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import StyleDictionary from "style-dictionary";
import { createConfig } from "../sd-config.js";
import { contrastRatio, hexToLinear, over } from "../src/color.ts";
import { CSS_VARS, cssValues } from "../src/formats.ts";
import { THEME_IDS, buildModel, readMeta, type ColorName, type ThemeId, type ThemeModel } from "../src/model.ts";

const pkg = fileURLToPath(new URL("..", import.meta.url));
const repo = join(pkg, "../..");
const sourceText = readFileSync(join(pkg, "tokens.json"), "utf8");
const source = (): Record<string, unknown> => JSON.parse(sourceText) as Record<string, unknown>;

let dir: string;
let models: ThemeModel[];
const theme = (id: ThemeId): ThemeModel => {
  const m = models.find((x) => x.id === id);
  if (m === undefined) throw new Error(id);
  return m;
};

before(async () => {
  dir = mkdtempSync(join(tmpdir(), "sanad-tokens-test-"));
  const sd = new StyleDictionary(createConfig({ buildDir: join(dir, "build"), rustDir: dir }));
  await sd.buildAllPlatforms();
  const { allTokens } = await sd.getPlatformTokens("css");
  models = buildModel(allTokens, readMeta(source()));
});

after(() => rmSync(dir, { recursive: true, force: true }));

describe("outputs", () => {
  test("committed Rust uniforms are exactly what the build generates", () => {
    const fresh = readFileSync(join(dir, "theme_uniforms.rs"), "utf8");
    const committed = readFileSync(join(repo, "crates/tokens/src/theme_uniforms.rs"), "utf8");
    assert.equal(committed, fresh, "stale crates/tokens/src/theme_uniforms.rs: run `pnpm --filter @sanad/tokens build`");
  });

  test("CSS declares every variable for every theme, and the OS-dark default mirrors the dark theme", () => {
    const css = readFileSync(join(dir, "build/css/themes.css"), "utf8");
    const blocks = new Map<string, Map<string, string>>();
    for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/gu)) {
      const selector = (m[1] ?? "").replace(/\/\*.*?\*\//gsu, "").replace(/@media[^{]*$/u, "").trim();
      const decls = new Map<string, string>();
      for (const d of (m[2] ?? "").replace(/\/\*.*?\*\//gsu, "").split(";")) {
        const i = d.indexOf(":");
        if (i > 0) decls.set(d.slice(0, i).trim(), d.slice(i + 1).trim());
      }
      blocks.set(selector, decls);
    }
    const light = readMeta(source()).defaults.light;
    for (const id of THEME_IDS) {
      const selector = id === light ? `:root,\n[data-theme="${id}"]` : `[data-theme="${id}"]`;
      const decls = blocks.get(selector);
      assert.ok(decls, `missing block for ${id}`);
      assert.equal(decls.get("color-scheme"), theme(id).mode);
      assert.deepEqual([...decls.keys()].filter((k) => k.startsWith("--")), [...CSS_VARS]);
      for (const [k, v] of Object.entries(cssValues(theme(id)))) assert.equal(decls.get(k), v, `${id} ${k}`);
    }
    const dark = blocks.get(":root:not([data-theme])");
    assert.ok(dark, "missing prefers-color-scheme: dark block");
    assert.deepEqual(Object.fromEntries(dark), Object.fromEntries(blocks.get(`[data-theme="night"]`) ?? []));
    assert.match(css, /@media \(prefers-color-scheme: dark\) \{\n {2}:root:not\(\[data-theme\]\)/u);
  });

  test("TS constants equal the CSS values, and uniforms are the linearized shipped hex", async () => {
    const ts = (await import(pathToFileURL(join(dir, "build/ts/tokens.ts")).href)) as typeof import("../build/ts/tokens.ts");
    assert.deepEqual(ts.THEME_IDS, [...THEME_IDS]);
    for (const id of THEME_IDS) {
      const t = ts.THEMES[id];
      assert.deepEqual(t.css, cssValues(theme(id)));
      const close = (a: readonly number[], hex: string, what: string): void => {
        hexToLinear(hex).forEach((v, i) => assert.ok(Math.abs((a[i] ?? Number.NaN) - v) <= 5e-7, `${id} ${what}[${i}]`));
      };
      close(t.uniforms.paper, t.css["--paper"], "paper");
      close(t.uniforms.ink, t.css["--ink"], "ink");
      close(t.uniforms.ink2, t.css["--ink-2"], "ink2");
      close(t.uniforms.accent, t.css["--accent"], "accent");
      t.uniforms.highlights.forEach((h, i) => close(h, t.css[`--highlight-${i + 1}` as "--highlight-1"], `highlight ${i}`));
      assert.equal(t.uniforms.weightPx, theme(id).shader.weightPx);
    }
  });
});

describe("design guarantees", () => {
  const hex = (id: ThemeId, n: ColorName): string => theme(id).colors[n].hex;
  const atLeast = (fg: string, bg: string, floor: number, what: string): void => {
    const r = contrastRatio(fg, bg);
    assert.ok(r >= floor, `${what}: ${r.toFixed(2)}:1 < ${floor}:1`);
  };

  test("text contrast floors (WCAG 2.x)", () => {
    for (const id of THEME_IDS) {
      atLeast(hex(id, "ink"), hex(id, "paper"), 7, `${id} ink/paper`);
      atLeast(hex(id, "ink-2"), hex(id, "paper"), 4.5, `${id} ink-2/paper`);
      atLeast(hex(id, "accent"), hex(id, "paper"), 4.5, `${id} accent/paper`);
      for (const h of ["highlight-1", "highlight-2", "highlight-3", "highlight-4"] as const) {
        atLeast(hex(id, "ink"), hex(id, h), 6.5, `${id} ink/${h}`);
      }
      for (const s of ["surface-1", "surface-2"] as const) {
        atLeast(hex(id, "ink"), hex(id, s), 7, `${id} ink/${s}`);
        atLeast(hex(id, "ink-2-on-glass"), hex(id, s), 5.4, `${id} ink-2-on-glass/${s}`);
      }
    }
  });

  // Minifiers ship rgb(r g b / a) as #rrggbbaa: check the 8-bit alpha too.
  const alphas = (a: number): number[] => [a, Math.round(a * 255) / 255];

  test("glass stays legible over the worst-case backdrop (pure black and pure white)", () => {
    for (const id of THEME_IDS) {
      const g = theme(id).colors["glass-tint"];
      for (const backdrop of ["#000000", "#ffffff"]) {
        for (const a of alphas(g.alpha)) {
          const fill = over(g.hex, a, backdrop);
          atLeast(hex(id, "ink"), fill, 7, `${id} ink on glass (α ${a}) over ${backdrop}`);
          atLeast(hex(id, "ink-2-on-glass"), fill, 5.4, `${id} ink-2-on-glass (α ${a}) over ${backdrop}`);
        }
      }
    }
  });

  test("focus ring reaches 3:1 against every background it can sit on (SC 1.4.11)", () => {
    for (const id of THEME_IDS) {
      const ring = theme(id).focusRing;
      assert.ok(ring.spread >= 2, `${id}: ring spread ${ring.spread}px`);
      const g = theme(id).colors["glass-tint"];
      const bgs = [hex(id, "paper"), hex(id, "surface-1"), hex(id, "surface-2"), over(g.hex, g.alpha, "#000000"), over(g.hex, g.alpha, "#ffffff")];
      for (const bg of bgs) {
        for (const a of alphas(ring.color.alpha)) atLeast(over(ring.color.hex, a, bg), bg, 3, `${id} focus ring (α ${a}) on ${bg}`);
      }
    }
  });

  test("dark elevation is lightness: surfaces step up in OKLCH L", () => {
    for (const id of THEME_IDS.filter((t) => theme(t).mode === "dark")) {
      const [p, s1, s2] = (["paper", "surface-1", "surface-2"] as const).map((n) => theme(id).colors[n].oklch[0]);
      assert.ok(p !== undefined && s1 !== undefined && s2 !== undefined);
      assert.ok(s1 - p >= 0.025 && s2 - s1 >= 0.025, `${id}: L ${p} → ${s1} → ${s2}`);
    }
  });
});

describe("blueprint agreement (docs/blueprint/04-design-system.md)", () => {
  const doc = readFileSync(join(repo, "docs/blueprint/04-design-system.md"), "utf8");
  const rows = (label: string): string[] => doc.split("\n").filter((l) => new RegExp(`^\\| (\\*\\*)?${label}(\\*\\*)? \\|`, "u").test(l));
  const num = (s: string): number => Number(s.replace("−", "-").replace(/^\+/u, ""));
  const near = (actual: number, stated: number, what: string): void => {
    assert.ok(Math.abs(actual - stated) <= 0.051, `${what}: tokens give ${actual.toFixed(2)}, blueprint says ${stated}`);
  };

  for (const id of THEME_IDS) {
    test(`${id}: palette, ratios, highlights and shader parameters`, () => {
      const m = theme(id);
      const [palette, shader, highlights] = rows(m.label);
      assert.ok(palette !== undefined && highlights !== undefined && shader !== undefined, `${m.label}: expected 3 table rows`);

      const hexes = [...palette.matchAll(/`(#[0-9A-F]{6})`/gu)].map((x) => (x[1] ?? "").toLowerCase());
      assert.deepEqual(hexes, [m.colors.paper.hex, m.colors.ink.hex, m.colors["ink-2"].hex, m.colors.accent.hex]);
      const ratios = [...palette.matchAll(/(\d+(?:\.\d+)?):1/gu)].map((x) => Number(x[1]));
      const [rInk, rInk2, rAcc] = ratios;
      assert.ok(rInk !== undefined && rInk2 !== undefined && rAcc !== undefined);
      near(contrastRatio(m.colors.ink.hex, m.colors.paper.hex), rInk, `${id} ink/paper`);
      near(contrastRatio(m.colors["ink-2"].hex, m.colors.paper.hex), rInk2, `${id} ink-2/paper`);
      near(contrastRatio(m.colors.accent.hex, m.colors.paper.hex), rAcc, `${id} accent/paper`);

      const hl = [...highlights.matchAll(/`(#[0-9A-F]{6})` (\d+(?:\.\d+)?)/gu)];
      assert.equal(hl.length, 4);
      hl.forEach((x, i) => {
        const c = m.colors[`highlight-${i + 1}` as ColorName];
        assert.equal((x[1] ?? "").toLowerCase(), c.hex);
        near(contrastRatio(m.colors.ink.hex, c.hex), Number(x[2]), `${id} ink/highlight-${i + 1}`);
      });

      const cells = shader.split("|").map((c) => c.trim()).filter((c) => c !== "");
      const [, weight, covGamma, lumaCeil, policy] = cells;
      assert.ok(weight !== undefined && covGamma !== undefined && lumaCeil !== undefined && policy !== undefined);
      assert.equal(num(weight), m.shader.weightPx);
      assert.equal(num(covGamma), m.shader.covGamma);
      assert.equal(num(lumaCeil), m.shader.lumaCeil);
      assert.ok(policy.startsWith(m.imagePolicy), `${id} image policy`);
    });
  }
});

describe("the build refuses bad tokens", () => {
  type Doc = { theme: Record<string, { color: Record<string, { $value: Record<string, unknown> }>; shader: Record<string, { $value: unknown }> }> };
  const rejects = async (mutate: (d: Doc) => void, message: RegExp): Promise<void> => {
    const d = source() as unknown as Doc;
    mutate(d);
    const out = mkdtempSync(join(tmpdir(), "sanad-tokens-bad-"));
    try {
      const sd = new StyleDictionary(createConfig({ tokens: d as unknown as Record<string, unknown>, buildDir: out, rustDir: out }));
      await assert.rejects(sd.buildAllPlatforms(), message);
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  };

  test("a hex that is not the exact rounding of its OKLCH", () =>
    rejects((d) => {
      const v = d.theme["paper"]?.color["ink"]?.$value;
      if (v) v["hex"] = "#1d1c1b";
    }, /not the 8-bit rounding of its OKLCH value \(expected #1d1c1a\)/u));

  test("an out-of-gamut OKLCH color", () =>
    rejects((d) => {
      const v = d.theme["dusk"]?.color["accent"]?.$value;
      if (v) v["components"] = [0.7736, 0.3, 253.6];
    }, /outside the sRGB gamut/u));

  test("a glare ceiling that would dim the theme's own ink", () =>
    rejects((d) => {
      const s = d.theme["dusk"]?.shader["lumaCeil"];
      if (s) s.$value = 0.55;
    }, /0\.55 would dim ink/u));

  test("a derived token drifting from its base", () =>
    rejects((d) => {
      const g = d.theme["night"]?.color["glass-tint"]?.$value;
      if (g) Object.assign(g, { components: [0.257, 0.0037, 286.1], hex: "#232325" });
    }, /glass-tint: color must equal its base theme\.night\.color\.surface-1/u));

  test("a dark theme that thickens strokes", () =>
    rejects((d) => {
      const s = d.theme["oled"]?.shader["weight"];
      if (s) s.$value = { value: 0.1, unit: "px" };
    }, /dark themes thin/u));

  test("an unknown token", () =>
    rejects((d) => {
      const c = d.theme["linen"]?.color;
      if (c) c["ink-3"] = { $value: { colorSpace: "oklch", components: [0.5, 0, 0], hex: "#636363" } };
    }, /unknown tokens: theme\.linen\.color\.ink-3/u));

  test("a missing token", () =>
    rejects((d) => {
      delete d.theme["paper"]?.color["hairline"];
    }, /theme\.paper\.color\.hairline: missing/u));
});
