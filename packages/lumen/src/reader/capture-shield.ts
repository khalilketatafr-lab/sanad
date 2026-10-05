/**
 * Capture shield — the opt-in, client-side deterrent layer (blueprint 05 §3.2),
 * folding in the field-proven mechanics of the WordPress "Ultimate Content
 * Shield" and wiring them to Sanad's server-side machinery instead of a
 * client-authoritative ban.
 *
 * Three honest, additive deterrents against *casual* desktop capture:
 *
 *   1. Focus/pointer veil — a paper-coloured overlay shown the instant the
 *      window loses focus OR the pointer leaves it, hidden ~100 ms after both
 *      are true again. Win+Shift+S, Alt+Tab, the Snipping Tool and clicking
 *      outside the window all blur the window first, so the veil blanks the
 *      page before such a capture lands. It also covers the app-switcher
 *      thumbnail (visibilitychange → hidden), which §3.2 already called for.
 *   2. PrintScreen signal — `keyup` (Windows delivers only keyup for this key)
 *      raises a graduated strike: a first/second warning, then a "blocked"
 *      signal reported to the Kernel.
 *   3. Clipboard neutralisation — a copy attempt is answered by overwriting the
 *      clipboard with a notice (there is no text to copy under P1 anyway).
 *
 * What this is NOT: a block. The OS hardware screenshot still fires before any
 * script on an instantaneous full-screen grab, a second camera is untouchable,
 * and a patched client ignores all of it. So the strike ladder here is only
 * *local, immediate feedback*; the authoritative response — pacing, pause,
 * account action — stays in the Kernel's Sentinel (05 §5), which a patched
 * client cannot clear, and which keeps due process. It is off by default
 * (focus-loss happens for innocent reasons; §3.2) and enabled per title or by
 * the reader for high-value content.
 */

/** A detected capture attempt, surfaced to the shell and reported to the Kernel. */
export interface CaptureSignal {
  readonly kind: "printscreen" | "copy";
  /** Local strike count after this signal. */
  readonly strikes: number;
  /** True once the local strike ladder has reached its ceiling. */
  readonly blocked: boolean;
}

export interface CaptureShieldConfig {
  /** Show the focus/pointer veil. Off by default (innocent focus loss). */
  readonly veil: boolean;
  /** Allow the veil on touch/mobile user agents (document.hasFocus is flaky there). */
  readonly veilOnMobile: boolean;
  /** Local strikes before a `blocked` signal (mirrors the plugin's 3). */
  readonly maxStrikes: number;
  /** Minimum time the veil stays up once shown, ms. */
  readonly minVeilMs: number;
  /** Veil colour (defaults to the reader's paper, so it reads as a blank page). */
  readonly veilColor: string;
}

export const DEFAULT_CAPTURE_SHIELD: CaptureShieldConfig = {
  veil: false,
  veilOnMobile: false,
  maxStrikes: 3,
  minVeilMs: 100,
  veilColor: "#ffffff",
};

// ── Pure logic (tested without a DOM) ──────────────────────────────────────

/** The local strike state. `blocked` latches once the ceiling is reached. */
export interface StrikeState {
  readonly strikes: number;
  readonly blocked: boolean;
}

export const NO_STRIKES: StrikeState = { strikes: 0, blocked: false };

/** Advances the strike ladder by one. Pure: same input → same output. */
export function nextStrike(state: StrikeState, maxStrikes: number): StrikeState {
  const strikes = state.strikes + 1;
  return { strikes, blocked: state.blocked || strikes >= Math.max(1, maxStrikes) };
}

/** The message tier for a strike count (immediate, local feedback only). */
export function strikeTier(strikes: number, maxStrikes: number): "warn" | "final" | "blocked" {
  if (strikes >= Math.max(1, maxStrikes)) return "blocked";
  if (strikes >= Math.max(1, maxStrikes) - 1) return "final";
  return "warn";
}

/**
 * The veil's single rule (the plugin's invariant): the page is veiled iff the
 * window has lost focus OR the pointer has left it.
 */
export function shouldVeil(signals: { focusLost: boolean; pointerOutside: boolean }): boolean {
  return signals.focusLost || signals.pointerOutside;
}

/** May the veil be hidden now? Only after the min-shown time, and only if unwanted. */
export function canHideVeil(opts: { shownAt: number; now: number; minVeilMs: number; wantVeil: boolean }): boolean {
  return !opts.wantVeil && opts.now - opts.shownAt >= opts.minVeilMs;
}

// ── DOM installer (browser only) ────────────────────────────────────────────

export interface CaptureShield {
  /** The current local strike state. */
  readonly strikes: StrikeState;
  /** Removes every listener and the veil element. */
  dispose(): void;
}

type Win = typeof globalThis & { document: Document };

const isMobileUA = (nav: Navigator): boolean => {
  const ua = nav.userAgent || "";
  if (/Android|iPhone|iPad|iPod|Mobile|Silk|Opera Mini|IEMobile/iu.test(ua)) return true;
  return /Macintosh/u.test(ua) && nav.maxTouchPoints > 1; // iPadOS reports as a Mac
};

/**
 * Installs the capture shield on the current document. Returns a handle whose
 * `dispose()` fully removes it. Safe to call only in a browser; `onSignal` is
 * invoked for each capture attempt so the shell can toast locally and report to
 * the Kernel (which owns the authoritative Sentinel response).
 */
export function installCaptureShield(
  root: Win,
  config: Partial<CaptureShieldConfig>,
  onSignal: (signal: CaptureSignal) => void,
): CaptureShield {
  const cfg: CaptureShieldConfig = { ...DEFAULT_CAPTURE_SHIELD, ...config };
  const doc = root.document;
  const veilEnabled = cfg.veil && (cfg.veilOnMobile || !isMobileUA(root.navigator));

  let strikes: StrikeState = NO_STRIKES;
  let focusLost = false;
  let pointerOutside = false;
  let shownAt = 0;
  const disposers: (() => void)[] = [];

  // Veil element: a paper-coloured overlay in the browser's top layer (a
  // "manual" popover so modals/fullscreen cannot sit above it), with a
  // z-index fallback for engines without the Popover API.
  let veil: HTMLElement | undefined;
  function ensureVeil(): HTMLElement | undefined {
    if (!veilEnabled) return undefined;
    const host = doc.body ?? doc.documentElement;
    if (veil === undefined && host !== null) {
      const el = doc.createElement("div");
      el.setAttribute("aria-hidden", "true");
      // Explicit width/height, not `inset:0`: a top-layer popover gets UA
      // `width:fit-content`, which with no content collapses to 0×0.
      el.style.cssText = `display:none;position:fixed;top:0;left:0;width:100%;height:100%;margin:0;padding:0;border:0;overflow:hidden;background:${cfg.veilColor};z-index:2147483647;pointer-events:none`;
      try {
        el.setAttribute("popover", "manual");
      } catch {
        /* older engines: z-index fallback */
      }
      host.appendChild(el);
      veil = el;
    }
    return veil;
  }
  const veilShown = (): boolean => veil !== undefined && veil.style.display === "block";
  function showVeil(): void {
    const el = ensureVeil();
    if (el === undefined) return;
    if (el.style.display !== "block") {
      el.style.display = "block";
      shownAt = Date.now();
    }
    const withPopover = el as HTMLElement & { showPopover?: () => void };
    if (typeof withPopover.showPopover === "function") {
      try {
        withPopover.showPopover();
      } catch {
        /* already open, or unsupported */
      }
    }
  }
  function hideVeil(): void {
    if (veil === undefined) return;
    const withPopover = veil as HTMLElement & { hidePopover?: () => void; matches(s: string): boolean };
    try {
      if (typeof withPopover.hidePopover === "function" && withPopover.matches(":popover-open")) withPopover.hidePopover();
    } catch {
      /* ignore */
    }
    veil.style.display = "none";
  }
  function settle(): void {
    if (!veilEnabled) return;
    if (shouldVeil({ focusLost, pointerOutside })) showVeil();
    else if (veilShown() && canHideVeil({ shownAt, now: Date.now(), minVeilMs: cfg.minVeilMs, wantVeil: false })) hideVeil();
  }

  // Capture signals.
  function raise(kind: CaptureSignal["kind"]): void {
    strikes = nextStrike(strikes, cfg.maxStrikes);
    onSignal({ kind, strikes: strikes.strikes, blocked: strikes.blocked });
  }
  function onKeyUp(e: KeyboardEvent): void {
    if (e.key === "PrintScreen" || e.code === "PrintScreen" || e.keyCode === 44) raise("printscreen");
  }
  function onCopy(e: ClipboardEvent): void {
    // There is no text to copy under P1; answer a copy attempt with a notice.
    try {
      e.clipboardData?.setData("text/plain", "Protected content — Sanad");
      e.preventDefault();
    } catch {
      /* ignore */
    }
    raise("copy");
  }

  const on = <K extends keyof WindowEventMap>(t: EventTarget, type: K | string, fn: EventListener): void => {
    t.addEventListener(type, fn);
    disposers.push(() => t.removeEventListener(type, fn));
  };

  on(doc, "keyup", onKeyUp as EventListener);
  on(doc, "copy", onCopy as EventListener);

  if (veilEnabled) {
    on(root, "blur", () => {
      focusLost = true;
      showVeil();
    });
    on(root, "focus", () => {
      focusLost = false;
      root.setTimeout(settle, cfg.minVeilMs);
    });
    on(doc, "mouseleave", () => {
      pointerOutside = true;
      showVeil();
    });
    on(doc, "mouseenter", () => {
      pointerOutside = false;
      root.setTimeout(settle, cfg.minVeilMs);
    });
    on(doc, "visibilitychange", () => {
      focusLost = doc.visibilityState === "hidden" ? true : focusLost;
      settle();
    });
    // Safety net: apply the rule even if an event was missed.
    const timer = root.setInterval(settle, cfg.minVeilMs);
    disposers.push(() => root.clearInterval(timer));
  }

  return {
    get strikes() {
      return strikes;
    },
    dispose() {
      for (const d of disposers.splice(0)) d();
      if (veil !== undefined) {
        hideVeil();
        veil.remove();
        veil = undefined;
      }
    },
  };
}
