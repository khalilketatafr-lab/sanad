/**
 * Reader settings: the control set from blueprint 04 §6.5, with the faithful
 * detents and defaults, validation that survives untrusted stored JSON, and
 * the two bridges that make it the single source of truth for the page —
 * [`toReaderStyle`] feeds the Compositor layout (src/reader/layout.ts) and
 * [`readerLight`] feeds the renderer's theme uniforms (src/gl/themes.ts).
 *
 * Scope (04 §6.1): size, theme, spacing and layout are *global* and follow the
 * reader across books and devices, which is what this module persists. The
 * "Original" font and publisher styles are *per book* (04 §6.1); that override
 * is a documented follow-up and is not persisted here yet.
 *
 * Persistence is a tiny [`SettingsStorage`] port, not `localStorage` directly,
 * so the store is testable in Node and degrades silently in a private window
 * (every read and write is guarded — a throw or a cleared store is not a
 * failure, it just falls back to defaults).
 */
import { MAX_DIM, type ReaderLight } from "../gl/themes.ts";
import type { ReaderStyle } from "./layout.ts";
import { THEME_IDS, type ThemeId } from "@sanad/tokens/ts";

/** The twelve type-size detents, CSS px (04 §3.3). */
export const SIZE_STEPS = [14, 15, 16, 17, 18, 20, 22, 24, 27, 30, 34, 40] as const;
export type SizePx = (typeof SIZE_STEPS)[number];

export type FontChoice = "literata" | "source-serif" | "atkinson" | "naskh" | "original";
export type Spacing = "compact" | "comfort" | "airy";
export type Margins = "narrow" | "normal" | "wide";
export type Columns = 1 | 2 | "auto";
export type Flow = "paged" | "scrolled";
export type Script = "latin" | "arabic";

const FONTS: readonly FontChoice[] = ["literata", "source-serif", "atkinson", "naskh", "original"];
const SPACINGS: readonly Spacing[] = ["compact", "comfort", "airy"];
const MARGINS: readonly Margins[] = ["narrow", "normal", "wide"];

/** Arabic glyphs read optically smaller; bump the effective size (04 §3.3). */
export const ARABIC_SIZE_BONUS = 2;

/** Target line length in characters per script (03 §3.1). */
export const MEASURE: Readonly<Record<Script, number>> = { latin: 66, arabic: 58 };

/** Spacing preset → line-height multiple (04, Leading) and paragraph rhythm. */
const SPACING: Readonly<Record<Spacing, { leading: number; paragraphGap: number }>> = {
  compact: { leading: 1.38, paragraphGap: 0.2 },
  comfort: { leading: 1.5, paragraphGap: 0.35 },
  airy: { leading: 1.7, paragraphGap: 0.6 },
};

/** Margin preset → side margin, CSS px (normal matches layout's default). */
const MARGIN_PX: Readonly<Record<Margins, number>> = { narrow: 22, normal: 32, wide: 48 };

/** Space above a heading, in ems (layout default). */
const HEADING_GAP = 1.2;

export interface ReaderSettings {
  readonly theme: ThemeId;
  readonly autoTheme: boolean;
  /** A detent from [`SIZE_STEPS`]. */
  readonly size: SizePx;
  readonly font: FontChoice;
  readonly spacing: Spacing;
  readonly margins: Margins;
  readonly justify: boolean;
  readonly hyphenate: boolean;
  readonly columns: Columns;
  readonly flow: Flow;
  /** Screen warmth, 0–100 (04 §6.5). */
  readonly warmth: number;
  /** In-app extra-dim below the OS floor, 0 … `MAX_DIM`. */
  readonly dim: number;
}

export const DEFAULT_SETTINGS: ReaderSettings = {
  theme: "paper",
  autoTheme: true,
  size: 20,
  font: "literata",
  spacing: "comfort",
  margins: "normal",
  justify: true,
  hyphenate: true,
  columns: "auto",
  flow: "paged",
  warmth: 0,
  dim: 0,
};

/**
 * The defaults for a given form factor and time of day (04 §3.3, §6.5): 18 px
 * on phones, 20 px on tablets and desktop; warmth 40 at night, 0 by day.
 */
export function defaultsFor(opts: { phone?: boolean; night?: boolean }): ReaderSettings {
  return { ...DEFAULT_SETTINGS, size: opts.phone === true ? 18 : 20, warmth: opts.night === true ? 40 : 0 };
}

const clamp = (v: number, lo: number, hi: number): number => (v < lo ? lo : v > hi ? hi : v);

/** The detent nearest `px` (used by the slider and by pinch-zoom). */
export function snapSize(px: number): SizePx {
  let best: SizePx = SIZE_STEPS[0];
  if (!Number.isFinite(px)) return DEFAULT_SETTINGS.size;
  for (const s of SIZE_STEPS) {
    if (Math.abs(s - px) < Math.abs(best - px)) best = s;
  }
  return best;
}

/** Moves `delta` detents from the one nearest `current`, clamped to the ends. */
export function stepSize(current: number, delta: number): SizePx {
  const i = SIZE_STEPS.indexOf(snapSize(current));
  return SIZE_STEPS[clamp(i + delta, 0, SIZE_STEPS.length - 1)]!;
}

/** The size a script renders at: Arabic gets the optical-size bonus. */
export function effectiveSize(size: SizePx, script: Script): number {
  return script === "arabic" ? size + ARABIC_SIZE_BONUS : size;
}

const oneOf = <T,>(set: readonly T[], v: unknown, fallback: T): T => (set.includes(v as T) ? (v as T) : fallback);

/**
 * Coerces an untrusted partial (stored JSON, a URL param, an event) into a
 * valid, fully-populated [`ReaderSettings`], snapping the size to a detent and
 * clamping warmth and dim. Any missing or malformed field falls back to `base`.
 */
export function normalize(patch: Partial<ReaderSettings>, base: ReaderSettings = DEFAULT_SETTINGS): ReaderSettings {
  const s = { ...base, ...patch };
  return {
    theme: oneOf(THEME_IDS, s.theme, base.theme),
    autoTheme: typeof s.autoTheme === "boolean" ? s.autoTheme : base.autoTheme,
    size: typeof s.size === "number" ? snapSize(s.size) : base.size,
    font: oneOf(FONTS, s.font, base.font),
    spacing: oneOf(SPACINGS, s.spacing, base.spacing),
    margins: oneOf(MARGINS, s.margins, base.margins),
    justify: typeof s.justify === "boolean" ? s.justify : base.justify,
    hyphenate: typeof s.hyphenate === "boolean" ? s.hyphenate : base.hyphenate,
    columns: s.columns === 1 || s.columns === 2 ? s.columns : "auto",
    flow: s.flow === "scrolled" ? "scrolled" : "paged",
    warmth: typeof s.warmth === "number" && Number.isFinite(s.warmth) ? clamp(Math.round(s.warmth), 0, 100) : base.warmth,
    dim: typeof s.dim === "number" && Number.isFinite(s.dim) ? clamp(s.dim, 0, MAX_DIM) : base.dim,
  };
}

/** Builds the Compositor layout style (src/reader/layout.ts) from settings. */
export function toReaderStyle(s: ReaderSettings): ReaderStyle {
  const { leading, paragraphGap } = SPACING[s.spacing];
  return {
    sizePx: [s.size, effectiveSize(s.size, "arabic")],
    leading,
    paragraphGap,
    margin: MARGIN_PX[s.margins],
    headingGap: HEADING_GAP,
  };
}

/** Builds the renderer's light uniforms (src/gl/themes.ts) from settings. */
export function readerLight(s: ReaderSettings): ReaderLight {
  return { warmth: s.warmth / 100, dim: s.dim };
}

/** The character measure for a script (03 §3.1), honoured by line breaking. */
export function measureFor(script: Script): number {
  return MEASURE[script];
}

/** A persistence backend. The browser adapter wraps `localStorage`; tests pass a map. */
export interface SettingsStorage {
  read(key: string): string | null;
  write(key: string, value: string): void;
}

/** Global-scope key; the `v1` suffix lets the shape migrate without clobbering. */
export const GLOBAL_KEY = "sanad.reader.settings.v1";

/** Reads and normalizes the persisted global settings, or `base` if absent/corrupt. */
export function loadSettings(storage: SettingsStorage, base: ReaderSettings = DEFAULT_SETTINGS): ReaderSettings {
  try {
    const raw = storage.read(GLOBAL_KEY);
    if (raw === null) return base;
    return normalize(JSON.parse(raw) as Partial<ReaderSettings>, base);
  } catch {
    return base;
  }
}

/** Persists the global settings; a failure (private window, quota) is swallowed. */
export function saveSettings(storage: SettingsStorage, settings: ReaderSettings): void {
  try {
    storage.write(GLOBAL_KEY, JSON.stringify(settings));
  } catch {
    // Best effort: a reader in a private window still reads fine this session.
  }
}

/** A `localStorage`-backed storage, or `undefined` where storage is unavailable. */
export function browserStorage(): SettingsStorage | undefined {
  try {
    const ls = globalThis.localStorage as Storage | undefined;
    if (ls == null) return undefined;
    return {
      read: (k) => {
        try {
          return ls.getItem(k);
        } catch {
          return null;
        }
      },
      write: (k, v) => {
        try {
          ls.setItem(k, v);
        } catch {
          /* ignore */
        }
      },
    };
  } catch {
    return undefined;
  }
}

/**
 * The live settings the shell reads and mutates. Every change is normalized,
 * persisted (when a storage was given) and pushed to subscribers, so the page
 * can re-layout and re-theme behind the glass in one frame (04 §6.1, "state is
 * saved continuously; there is never an Apply or Cancel button").
 */
export class SettingsStore {
  #value: ReaderSettings;
  readonly #storage: SettingsStorage | undefined;
  readonly #listeners = new Set<(s: ReaderSettings) => void>();

  constructor(
    opts: {
      storage?: SettingsStorage | undefined;
      initial?: Partial<ReaderSettings> | undefined;
      base?: ReaderSettings | undefined;
    } = {},
  ) {
    const base = opts.base ?? DEFAULT_SETTINGS;
    const persisted = opts.storage !== undefined ? loadSettings(opts.storage, base) : base;
    this.#value = opts.initial !== undefined ? normalize(opts.initial, persisted) : persisted;
    this.#storage = opts.storage;
  }

  get value(): ReaderSettings {
    return this.#value;
  }

  /** The current Compositor layout style. */
  get style(): ReaderStyle {
    return toReaderStyle(this.#value);
  }

  /** The current renderer light uniforms. */
  get light(): ReaderLight {
    return readerLight(this.#value);
  }

  /** Applies a patch (normalized against the current value), persists, notifies. */
  set(patch: Partial<ReaderSettings>): ReaderSettings {
    const next = normalize(patch, this.#value);
    this.#value = next;
    if (this.#storage !== undefined) saveSettings(this.#storage, next);
    for (const listener of this.#listeners) listener(next);
    return next;
  }

  /** Moves the type size by `delta` detents (the slider and pinch-zoom). */
  stepSize(delta: number): ReaderSettings {
    return this.set({ size: stepSize(this.#value.size, delta) });
  }

  /** Resets every control to a baseline ("Reset to defaults" in More, 04 §6.5). */
  reset(base: ReaderSettings = DEFAULT_SETTINGS): ReaderSettings {
    return this.set(base);
  }

  /** Subscribes to changes; the listener is called immediately with the current value. */
  subscribe(listener: (s: ReaderSettings) => void): () => void {
    this.#listeners.add(listener);
    listener(this.#value);
    return () => {
      this.#listeners.delete(listener);
    };
  }
}
