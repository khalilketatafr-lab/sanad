/**
 * The validated theme model every output is printed from.
 *
 * Style Dictionary runs the `resolve*` functions as value transforms, then the
 * formats call `buildModel` on the transformed tokens. Anything malformed,
 * missing, extra or inconsistent throws, which fails the build: a generated
 * file is either complete and correct or not written at all.
 */
import { type Rgb, hexToLinear, isDtcgColor, linearToHex, oklchToLinear } from "./color.ts";

export const THEME_IDS = ["paper", "linen", "dusk", "night", "oled"] as const;
export type ThemeId = (typeof THEME_IDS)[number];

export const COLOR_NAMES = [
  "paper",
  "ink",
  "ink-2",
  "accent",
  "highlight-1",
  "highlight-2",
  "highlight-3",
  "highlight-4",
  "surface-1",
  "surface-2",
  "ink-2-on-glass",
  "glass-tint",
  "hairline",
] as const;
export type ColorName = (typeof COLOR_NAMES)[number];

/** Colors drawn by Lumen (Pass 3 uniforms): must be opaque. */
export const SHADER_COLORS = ["paper", "ink", "ink-2", "accent", "highlight-1", "highlight-2", "highlight-3", "highlight-4"] as const satisfies readonly ColorName[];

export const SHADER_PARAMS = ["covGamma", "weight", "lumaCeil"] as const;
export type ShaderParam = (typeof SHADER_PARAMS)[number];

export const IMAGE_POLICIES = ["natural", "dimmed", "smart-invert"] as const;
export type ImagePolicy = (typeof IMAGE_POLICIES)[number];
export type ThemeMode = "light" | "dark";

export class TokenError extends Error {
  override name = "TokenError";
}

/** A color token after the `sanad/color/resolve` transform. */
export interface ResolvedColor {
  /** `#rrggbb`, lowercase: what CSS ships. */
  readonly hex: string;
  readonly alpha: number;
  /** Authoritative OKLCH components [L 0–1, C, H°]. */
  readonly oklch: readonly [number, number, number];
  /**
   * LINEAR sRGB of the shipped hex: what shaders receive. Derived from the hex
   * (not the unrounded OKLCH) so the WebGL page and the CSS chrome produce the
   * same 8-bit pixels.
   */
  readonly linear: Rgb;
}

/** A shadow token after `sanad/shadow/resolve`; lengths in CSS px. */
export interface ResolvedShadow {
  readonly color: ResolvedColor;
  readonly offsetX: number;
  readonly offsetY: number;
  readonly blur: number;
  readonly spread: number;
}

/** Linear channels may exceed [0, 1] by this much before a color is out of gamut. */
const GAMUT_EPSILON = 1e-3;

export function resolveColor(value: unknown, where: string): ResolvedColor {
  if (!isDtcgColor(value)) {
    throw new TokenError(`${where}: expected a DTCG color { colorSpace: "oklch", components: [L, C, H], hex, alpha? }`);
  }
  const [l, c, h] = value.components;
  if (!(l >= 0 && l <= 1 && c >= 0 && c <= 0.4 && h >= 0 && h < 360)) {
    throw new TokenError(`${where}: OKLCH components out of range: [${value.components.join(", ")}]`);
  }
  const lin = oklchToLinear(value.components);
  if (lin.some((ch) => ch < -GAMUT_EPSILON || ch > 1 + GAMUT_EPSILON)) {
    throw new TokenError(`${where}: oklch(${value.components.join(" ")}) is outside the sRGB gamut`);
  }
  const hex = value.hex.toLowerCase();
  const exact = linearToHex(lin);
  if (hex !== exact) {
    throw new TokenError(`${where}: hex ${value.hex} is not the 8-bit rounding of its OKLCH value (expected ${exact})`);
  }
  return { hex, alpha: value.alpha ?? 1, oklch: value.components, linear: hexToLinear(hex) };
}

interface DtcgDimension {
  readonly value: number;
  readonly unit: "px" | "rem";
}

function isDimension(v: unknown): v is DtcgDimension {
  if (typeof v !== "object" || v === null) return false;
  const d = v as Partial<DtcgDimension>;
  return typeof d.value === "number" && Number.isFinite(d.value) && (d.unit === "px" || d.unit === "rem");
}

export function resolveDimensionPx(value: unknown, where: string): number {
  if (!isDimension(value)) throw new TokenError(`${where}: expected a DTCG dimension { value, unit }`);
  if (value.unit !== "px") throw new TokenError(`${where}: unit must be px (shader uniforms are in screen px)`);
  return value.value;
}

export function resolveShadow(value: unknown, where: string): ResolvedShadow {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TokenError(`${where}: expected a single DTCG shadow object`);
  }
  const s = value as Record<string, unknown>;
  const allowed = new Set(["color", "offsetX", "offsetY", "blur", "spread"]);
  for (const k of Object.keys(s)) {
    if (!allowed.has(k)) throw new TokenError(`${where}: unsupported shadow member "${k}"`);
  }
  const blur = resolveDimensionPx(s["blur"], `${where}.blur`);
  if (blur < 0) throw new TokenError(`${where}.blur: must be ≥ 0`);
  return {
    color: resolveColor(s["color"], `${where}.color`),
    offsetX: resolveDimensionPx(s["offsetX"], `${where}.offsetX`),
    offsetY: resolveDimensionPx(s["offsetY"], `${where}.offsetY`),
    blur,
    spread: resolveDimensionPx(s["spread"], `${where}.spread`),
  };
}

export interface ThemeMeta {
  readonly label: string;
  readonly mode: ThemeMode;
  readonly imagePolicy: ImagePolicy;
}

export interface TokenMeta {
  readonly themes: Readonly<Record<ThemeId, ThemeMeta>>;
  /** Theme used when the document has no `data-theme`, per `prefers-color-scheme`. */
  readonly defaults: { readonly light: ThemeId; readonly dark: ThemeId };
}

const isThemeId = (v: unknown): v is ThemeId => typeof v === "string" && (THEME_IDS as readonly string[]).includes(v);

function sanadExtension(node: unknown, where: string): Record<string, unknown> {
  const ext = (node as { $extensions?: Record<string, unknown> } | null)?.$extensions?.["app.sanad"];
  if (typeof ext !== "object" || ext === null) throw new TokenError(`${where}: missing $extensions["app.sanad"]`);
  return ext as Record<string, unknown>;
}

/**
 * Group-level metadata. Style Dictionary drops `$extensions` on groups, so it
 * is read from the source document directly.
 */
export function readMeta(source: unknown): TokenMeta {
  const root = sanadExtension(source, "tokens.json");
  const defaults = root["defaults"] as { light?: unknown; dark?: unknown } | undefined;
  if (!isThemeId(defaults?.light) || !isThemeId(defaults.dark)) {
    throw new TokenError(`tokens.json: $extensions["app.sanad"].defaults must name a light and a dark theme`);
  }
  const groups = (source as { theme?: Record<string, unknown> }).theme ?? {};
  const ids = Object.keys(groups).filter((k) => !k.startsWith("$"));
  if (ids.join() !== THEME_IDS.join()) {
    throw new TokenError(`tokens.json: themes must be exactly [${THEME_IDS.join(", ")}] in that order, got [${ids.join(", ")}]`);
  }
  const themes = Object.fromEntries(
    THEME_IDS.map((id): [ThemeId, ThemeMeta] => {
      const ext = sanadExtension(groups[id], `theme.${id}`);
      const { label, mode, imagePolicy } = ext;
      if (typeof label !== "string" || label === "") throw new TokenError(`theme.${id}: label must be a non-empty string`);
      if (mode !== "light" && mode !== "dark") throw new TokenError(`theme.${id}: mode must be "light" or "dark"`);
      if (!(IMAGE_POLICIES as readonly unknown[]).includes(imagePolicy)) {
        throw new TokenError(`theme.${id}: imagePolicy must be one of ${IMAGE_POLICIES.join(", ")}`);
      }
      return [id, { label, mode, imagePolicy: imagePolicy as ImagePolicy }];
    }),
  ) as Record<ThemeId, ThemeMeta>;
  if (themes[defaults.light].mode !== "light" || themes[defaults.dark].mode !== "dark") {
    throw new TokenError("tokens.json: default themes must match their color scheme");
  }
  return { themes, defaults: { light: defaults.light, dark: defaults.dark } };
}

export interface ThemeShader {
  readonly covGamma: number;
  /** Optical weight compensation in screen px (token `weight`, u_weightPx). */
  readonly weightPx: number;
  readonly lumaCeil: number;
}

export interface ThemeModel extends ThemeMeta {
  readonly id: ThemeId;
  readonly colors: Readonly<Record<ColorName, ResolvedColor>>;
  readonly focusRing: ResolvedShadow;
  readonly shader: ThemeShader;
}

/** The subset of Style Dictionary's TransformedToken the model reads. */
export interface TokenLike {
  readonly path: readonly string[];
  readonly $type?: string | undefined;
  readonly $value?: unknown;
  readonly $extensions?: Readonly<Record<string, unknown>> | undefined;
}

/** WCAG 2.x relative luminance of a linear sRGB triple. */
export const luminance = ([r, g, b]: Rgb): number => 0.2126 * r + 0.7152 * g + 0.0722 * b;

export function buildModel(tokens: readonly TokenLike[], meta: TokenMeta): ThemeModel[] {
  const byPath = new Map<string, TokenLike>();
  for (const t of tokens) {
    const where = t.path.join(".");
    if (t.path.length !== 4 || t.path[0] !== "theme" || !isThemeId(t.path[1])) {
      throw new TokenError(`${where}: tokens must live at theme.<id>.<group>.<name>`);
    }
    byPath.set(where, t);
  }

  const take = (path: string, type: string): TokenLike => {
    const t = byPath.get(path);
    if (t === undefined) throw new TokenError(`${path}: missing`);
    if (t.$type !== type) throw new TokenError(`${path}: $type must be "${type}", got "${String(t.$type)}"`);
    byPath.delete(path);
    return t;
  };
  const asColor = (t: TokenLike): ResolvedColor => {
    const v = t.$value as Partial<ResolvedColor> | undefined;
    if (typeof v?.hex !== "string" || v.linear === undefined) {
      throw new TokenError(`${t.path.join(".")}: color not resolved (is the sanad/color/resolve transform registered?)`);
    }
    return v as ResolvedColor;
  };
  const asNumber = (t: TokenLike): number => {
    if (typeof t.$value !== "number" || !Number.isFinite(t.$value)) throw new TokenError(`${t.path.join(".")}: expected a number`);
    return t.$value;
  };

  const models = THEME_IDS.map((id): ThemeModel => {
    const p = `theme.${id}`;
    const colorTokens = Object.fromEntries(COLOR_NAMES.map((n) => [n, take(`${p}.color.${n}`, "color")])) as Record<ColorName, TokenLike>;
    const colors = Object.fromEntries(COLOR_NAMES.map((n) => [n, asColor(colorTokens[n])])) as Record<ColorName, ResolvedColor>;
    const ringToken = take(`${p}.shadow.focus-ring`, "shadow");
    const focusRing = ringToken.$value as ResolvedShadow;
    if (typeof focusRing.spread !== "number") throw new TokenError(`${p}.shadow.focus-ring: shadow not resolved`);
    const weight = take(`${p}.shader.weight`, "dimension").$value;
    if (typeof weight !== "number") throw new TokenError(`${p}.shader.weight: dimension not resolved`);
    const shader: ThemeShader = {
      covGamma: asNumber(take(`${p}.shader.covGamma`, "number")),
      weightPx: weight,
      lumaCeil: asNumber(take(`${p}.shader.lumaCeil`, "number")),
    };
    const model: ThemeModel = { id, ...meta.themes[id], colors, focusRing, shader };

    // Derived tokens restate their base color with their own alpha; the base
    // must still match, so editing a base can never silently orphan them.
    const derived: [string, TokenLike, ResolvedColor][] = [
      [`${p}.color.glass-tint`, colorTokens["glass-tint"], colors["glass-tint"]],
      [`${p}.color.hairline`, colorTokens.hairline, colors.hairline],
      [`${p}.shadow.focus-ring`, ringToken, focusRing.color],
    ];
    for (const [where, token, color] of derived) {
      const from = (token.$extensions?.["app.sanad"] as { derivedFrom?: unknown } | undefined)?.derivedFrom;
      const base = typeof from === "string" && from.startsWith(`${p}.color.`) ? colors[from.slice(p.length + 7) as ColorName] : undefined;
      if (base === undefined) throw new TokenError(`${where}: $extensions["app.sanad"].derivedFrom must name a color of theme ${id}`);
      if (base.hex !== color.hex || base.oklch.join() !== color.oklch.join()) {
        throw new TokenError(`${where}: color must equal its base ${String(from)} (${base.hex}), got ${color.hex}`);
      }
    }

    validateShader(model);
    return model;
  });

  if (byPath.size > 0) throw new TokenError(`unknown tokens: ${[...byPath.keys()].join(", ")}`);
  return models;
}

/** Invariants the Pass 3 composite relies on. */
function validateShader(m: ThemeModel): void {
  const where = `theme.${m.id}.shader`;
  for (const n of SHADER_COLORS) {
    if (m.colors[n].alpha !== 1) throw new TokenError(`theme.${m.id}.color.${n}: shader colors must be opaque`);
  }
  const { covGamma, weightPx, lumaCeil } = m.shader;
  if (!(covGamma > 0.5 && covGamma < 2)) throw new TokenError(`${where}.covGamma: must be in (0.5, 2)`);
  if (!(Math.abs(weightPx) <= 0.5)) throw new TokenError(`${where}.weight: |weight| must be ≤ 0.5 px`);
  if ((m.mode === "dark") !== weightPx < 0) {
    throw new TokenError(`${where}.weight: light-on-dark text blooms, so dark themes thin (< 0) and light themes do not`);
  }
  if (!(lumaCeil > 0 && lumaCeil <= 1)) throw new TokenError(`${where}.lumaCeil: must be in (0, 1]`);
  // The ceiling scales every output pixel. It must never touch the theme's own
  // palette, or the page would stop matching the CSS chrome.
  for (const n of SHADER_COLORS) {
    const y = luminance(m.colors[n].linear);
    if (y > lumaCeil) {
      throw new TokenError(`${where}.lumaCeil: ${lumaCeil} would dim ${n} (Y = ${y.toFixed(4)}); the ceiling must be ≥ every palette color`);
    }
  }
  // In dark themes the ceiling is the ink's luminance (rounded up to 0.01):
  // no image pixel may outshine the text.
  const inkY = luminance(m.colors.ink.linear);
  if (m.mode === "dark" && lumaCeil > Math.ceil(inkY * 100) / 100 + 1e-9) {
    throw new TokenError(`${where}.lumaCeil: dark themes cap images at the ink's luminance (≤ ${Math.ceil(inkY * 100) / 100}), got ${lumaCeil}`);
  }
}
