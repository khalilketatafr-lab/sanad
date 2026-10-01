/**
 * Theme selection for the renderer: the generated token uniforms of a theme
 * plus the reader's light settings, and the 240 ms theme crossfade, which is a
 * pure uniform interpolation in linear RGB (no re-layout, no Pass 1 re-render).
 */
import { DEFAULT_THEME, THEMES, type Rgb, type ThemeId } from "@sanad/tokens/ts";
import type { ThemeUniforms } from "./theme.ts";

/** Reader light settings (Settings → Light); not part of any theme. */
export interface ReaderLight {
  /** 0 neutral … 1 full warm. */
  readonly warmth: number;
  /** Extra-dim below the OS brightness floor, 0 … MAX_DIM. */
  readonly dim: number;
}

export const NEUTRAL_LIGHT: ReaderLight = { warmth: 0, dim: 0 };
export const MAX_DIM = 0.6;
export const THEME_CROSSFADE_MS = 240;

const clamp = (x: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, Number.isFinite(x) ? x : lo));

export function themeUniforms(id: ThemeId, light: ReaderLight = NEUTRAL_LIGHT): ThemeUniforms {
  return { ...THEMES[id].uniforms, warmth: clamp(light.warmth, 0, 1), dim: clamp(light.dim, 0, MAX_DIM) };
}

/** The theme used when the reader has not chosen one. */
export function defaultThemeId(prefersDark: boolean): ThemeId {
  return prefersDark ? DEFAULT_THEME.dark : DEFAULT_THEME.light;
}

const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
const lerpRgb = (a: Rgb, b: Rgb, t: number): Rgb => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];

/** Crossfade frame `t` ∈ [0, 1] between two themes, in linear light. */
export function mixThemes(a: ThemeUniforms, b: ThemeUniforms, t: number): ThemeUniforms {
  const k = clamp(t, 0, 1);
  return {
    paper: lerpRgb(a.paper, b.paper, k),
    ink: lerpRgb(a.ink, b.ink, k),
    ink2: lerpRgb(a.ink2, b.ink2, k),
    accent: lerpRgb(a.accent, b.accent, k),
    highlights: [
      lerpRgb(a.highlights[0], b.highlights[0], k),
      lerpRgb(a.highlights[1], b.highlights[1], k),
      lerpRgb(a.highlights[2], b.highlights[2], k),
      lerpRgb(a.highlights[3], b.highlights[3], k),
    ],
    covGamma: lerp(a.covGamma, b.covGamma, k),
    weightPx: lerp(a.weightPx, b.weightPx, k),
    // The ceiling interpolates too, so it is ≥ both palettes' luminance at
    // every frame (linear Y is linear in RGB): the fade never dims text.
    lumaCeil: lerp(a.lumaCeil, b.lumaCeil, k),
    warmth: lerp(a.warmth, b.warmth, k),
    dim: lerp(a.dim, b.dim, k),
  };
}
