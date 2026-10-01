/**
 * Color math for Marginalia tokens. OKLCH is the source of truth; everything
 * else is derived here, once, for CSS (hex + oklch), TS/Rust shader uniforms
 * (LINEAR sRGB) and contrast validation (WCAG 2.x relative luminance).
 * Matrices: Björn Ottosson, "A perceptual color space for image processing" (2020).
 */

export type Rgb = readonly [number, number, number];

/** DTCG 2025.10 color value as used in tokens.json. */
export interface DtcgColor {
  readonly colorSpace: "oklch";
  /** [L 0–1, C ≥ 0, H degrees] */
  readonly components: readonly [number, number, number];
  readonly alpha?: number;
  /** sRGB fallback; must equal the OKLCH value rounded to 8 bits per channel. */
  readonly hex: string;
}

export function isDtcgColor(v: unknown): v is DtcgColor {
  if (typeof v !== "object" || v === null) return false;
  const c = v as Partial<DtcgColor>;
  return (
    c.colorSpace === "oklch" &&
    Array.isArray(c.components) &&
    c.components.length === 3 &&
    c.components.every((n) => typeof n === "number" && Number.isFinite(n)) &&
    typeof c.hex === "string" &&
    (c.alpha === undefined || (c.alpha >= 0 && c.alpha <= 1))
  );
}

/** OKLCH → linear sRGB (unclamped). */
export function oklchToLinear([l, c, h]: readonly [number, number, number]): Rgb {
  const a = c * Math.cos((h * Math.PI) / 180);
  const b = c * Math.sin((h * Math.PI) / 180);
  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.291485548 * b;
  const [L, M, S] = [l_ ** 3, m_ ** 3, s_ ** 3];
  return [
    4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
    -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
    -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S,
  ];
}

const clamp01 = (x: number): number => Math.min(1, Math.max(0, x));

export function linearToSrgb(c: number): number {
  const x = clamp01(c);
  return x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
}

export function srgbToLinear(c: number): number {
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

export function linearToHex(rgb: Rgb): string {
  return `#${rgb.map((c) => Math.round(linearToSrgb(c) * 255).toString(16).padStart(2, "0")).join("")}`;
}

export function hexToLinear(hex: string): Rgb {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/iu.exec(hex);
  if (m === null) throw new Error(`invalid hex ${hex}`);
  const ch = (i: 1 | 2 | 3): number => srgbToLinear(Number.parseInt(m[i] ?? "0", 16) / 255);
  return [ch(1), ch(2), ch(3)];
}

/** Linear sRGB, clamped to the gamut: what shaders receive. */
export function shaderRgb(v: DtcgColor): Rgb {
  const [r, g, b] = oklchToLinear(v.components);
  return [clamp01(r), clamp01(g), clamp01(b)];
}

export function oklchCss(v: DtcgColor): string {
  const [l, c, h] = v.components;
  const pct = `${+(l * 100).toFixed(2)}%`;
  const alpha = v.alpha === undefined || v.alpha === 1 ? "" : ` / ${v.alpha}`;
  return `oklch(${pct} ${+c.toFixed(4)} ${+h.toFixed(2)}${alpha})`;
}

export function hexCss(v: DtcgColor): string {
  if (v.alpha === undefined || v.alpha === 1) return v.hex.toLowerCase();
  const [r, g, b] = [1, 3, 5].map((i) => Number.parseInt(v.hex.slice(i, i + 2), 16));
  return `rgb(${r} ${g} ${b} / ${v.alpha})`;
}

export function relativeLuminance(hex: string): number {
  const [r, g, b] = hexToLinear(hex);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** `fg` at `alpha` over `bg`, composited in gamma space as browsers do for CSS colors. */
export function over(fg: string, alpha: number, bg: string): string {
  const f = [1, 3, 5].map((i) => Number.parseInt(fg.slice(i, i + 2), 16));
  const b = [1, 3, 5].map((i) => Number.parseInt(bg.slice(i, i + 2), 16));
  return `#${f.map((c, i) => Math.round(alpha * c + (1 - alpha) * (b[i] ?? 0)).toString(16).padStart(2, "0")).join("")}`;
}
