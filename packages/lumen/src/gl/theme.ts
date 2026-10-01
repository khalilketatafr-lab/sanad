/**
 * Theme uniforms for Pass 3. The theme-owned part is generated from
 * @sanad/tokens (D6); colors are LINEAR sRGB of the shipped hex, never
 * gamma-encoded, so blending happens in linear light and the page matches the
 * CSS chrome exactly. Warmth and extra-dim are reader settings layered on top.
 */
import type { Rgb, ThemeShaderUniforms } from "@sanad/tokens/ts";

export type { Rgb };

export interface ThemeUniforms extends ThemeShaderUniforms {
  /** 0 neutral … 1 = full warm (≈ 3400 K). */
  readonly warmth: number;
  /** Extra-dim, 0 … 0.6. */
  readonly dim: number;
}

/**
 * Linear-light white-balance gains of a ≈3400 K blackbody relative to D65,
 * normalized to R = 1 (Tanner Helland's fit, linearized).
 */
const WARM_3400K: Rgb = [1.0, 0.514, 0.242];

export function warmGain(warmth: number): Rgb {
  const w = Math.min(1, Math.max(0, warmth));
  return [1 + (WARM_3400K[0] - 1) * w, 1 + (WARM_3400K[1] - 1) * w, 1 + (WARM_3400K[2] - 1) * w];
}

export function srgbToLinear(channel8: number): number {
  const c = channel8 / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

export function hexToLinear(hex: string): Rgb {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/iu.exec(hex);
  if (m === null) throw new Error(`invalid hex color ${hex}`);
  return [srgbToLinear(Number.parseInt(m[1] ?? "0", 16)), srgbToLinear(Number.parseInt(m[2] ?? "0", 16)), srgbToLinear(Number.parseInt(m[3] ?? "0", 16))];
}
