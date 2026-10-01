/**
 * The roadmap §10.4 reflow fixture (10 Latin + 10 Arabic paragraphs, shaped
 * and permuted by Atelier): types and the layout helpers shared by the WASM
 * parity test and the in-browser reflow benchmark. No Node APIs here.
 */
import type { Compositor } from "@sanad/lumen-wasm";

export interface FixtureParagraph {
  readonly baseLevel: number;
  readonly scale: number;
  readonly gids: number[];
  readonly advances: number[];
  readonly offsets: number[];
  readonly flags: number[];
  readonly levels: number[];
  readonly kashidaPriority: number[];
  readonly kashidaMax: number[];
  readonly hyphen: [number, number] | null;
  readonly tatweel: [number, number] | null;
}

export interface ReflowFixture {
  readonly paragraphs: FixtureParagraph[];
  /** Native layout digest per measure (device px). */
  readonly reference: { readonly width: number; readonly lines: number; readonly digest: number }[];
}

/** Typed arrays ready for `Compositor.layout_paragraph`. */
export interface PreparedParagraph {
  readonly baseLevel: number;
  readonly scale: number;
  readonly gids: Uint16Array;
  readonly advances: Int16Array;
  readonly offsets: Int16Array;
  readonly flags: Uint8Array;
  readonly levels: Uint8Array;
  readonly kashidaPriority: Uint8Array;
  readonly kashidaMax: Uint16Array;
  readonly hyphen: readonly [number, number];
  readonly tatweel: readonly [number, number];
}

export function prepare(p: FixtureParagraph): PreparedParagraph {
  return {
    baseLevel: p.baseLevel,
    scale: p.scale,
    gids: Uint16Array.from(p.gids),
    advances: Int16Array.from(p.advances),
    offsets: Int16Array.from(p.offsets),
    flags: Uint8Array.from(p.flags),
    levels: Uint8Array.from(p.levels),
    kashidaPriority: Uint8Array.from(p.kashidaPriority),
    kashidaMax: Uint16Array.from(p.kashidaMax),
    hyphen: p.hyphen ?? [0, 0],
    tatweel: p.tatweel ?? [0, 0],
  };
}

export function layout(c: Compositor, p: PreparedParagraph, width: number): number {
  return c.layout_paragraph(p.gids, p.advances, p.offsets, p.flags, p.levels, p.kashidaPriority, p.kashidaMax, p.scale, width, p.baseLevel, p.hyphen[0], p.hyphen[1], p.tatweel[0], p.tatweel[1]);
}

/** FNV-1a over the little-endian bits of every line's `line_glyphs` output (as the native digest). */
export function digestReflow(c: Compositor, paragraphs: readonly PreparedParagraph[], width: number): { digest: number; lines: number } {
  let h = 0x811c9dc5;
  let lines = 0;
  for (const p of paragraphs) {
    const n = layout(c, p, width);
    for (let i = 0; i < n; i++) {
      const g = c.line_glyphs(i);
      const bytes = new Uint8Array(g.buffer, g.byteOffset, g.byteLength);
      for (const b of bytes) {
        h ^= b;
        h = Math.imul(h, 0x01000193) >>> 0;
      }
      lines++;
    }
  }
  return { digest: h >>> 0, lines };
}
