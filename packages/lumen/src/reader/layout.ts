/**
 * Lays a decoded Flow chunk out into Lumen instances on the client.
 *
 * Runs go through the Compositor (WASM, Knuth–Plass + bidi + kashida); each
 * positioned glyph becomes one instance per atlas fragment, exactly as
 * `crates/atelier/src/page.rs::emit_glyph` does server-side. The reader never
 * has the fonts: a glyph's atlas entry carries its origin and texels-per-unit,
 * and the MSDF field is a fixed 48 texels/em, so device-px-per-texel is simply
 * `fontSizePx / 48`.
 *
 * The Compositor's wasm entry point lays out one run at a time, so a block's
 * runs are concatenated in logical order and set with the block's base
 * direction. Mixed-font blocks use the dominant run's scale (faithful
 * multi-font layout needs the multi-run entry point, not yet bound to wasm).
 */
import type { Compositor } from "@sanad/lumen-wasm";

import { GLYPH_INSTANCE } from "../gl/shaders.ts";
import type { FlowBlock, FlowChunk } from "../vault/flow.ts";

/** MSDF field density (crates/atelier/src/msdf.rs `FieldParams::em_texels`). */
const EM_TEXELS = 48;

export interface AtlasGlyph {
  /** Ink origin in the slot, texels. */
  readonly origin: readonly [number, number];
  /** Atlas texels per font unit (constant per font). */
  readonly texelsPerUnit: number;
  /** Fragment slots `[x, y, w, h]`, texels. */
  readonly slots: readonly (readonly [number, number, number, number])[];
}

export interface GlyphTable {
  readonly pxRange: number;
  readonly width: number;
  readonly height: number;
  readonly glyphs: ReadonlyMap<number, AtlasGlyph>;
}

export interface ReaderStyle {
  /** Reading size per font index (0 Latin, 1 Arabic), CSS px. */
  readonly sizePx: readonly [number, number];
  /** Line pitch as a multiple of the block's size. */
  readonly leading: number;
  /** Extra space before a paragraph, in ems of its size. */
  readonly paragraphGap: number;
  /** Side margin, CSS px. */
  readonly margin: number;
  /** Space above a heading, in ems. */
  readonly headingGap: number;
}

export const DEFAULT_STYLE: ReaderStyle = {
  sizePx: [20, 24],
  leading: 1.5,
  paragraphGap: 0.35,
  margin: 32,
  headingGap: 1.2,
};

export interface LaidOutPage {
  /** GLYPH_INSTANCE records, ready for `LumenRenderer.setInstances`. */
  readonly instances: Float32Array;
  readonly count: number;
  readonly width: number;
  readonly height: number;
}

/** Floats per instance (40 bytes / 4). */
const FLOATS = GLYPH_INSTANCE.stride / 4;

interface Run {
  gids: Uint16Array;
  advances: Int16Array;
  offsets: Int16Array;
  flags: Uint8Array;
  levels: Uint8Array;
  kprio: Uint8Array;
  kmax: Uint16Array;
}

/** Concatenates a block's runs into one SoA run (logical order). */
function concat(block: FlowBlock): { run: Run; font: number } {
  const n = block.runs.reduce((s, r) => s + r.gids.length, 0);
  const run: Run = {
    gids: new Uint16Array(n),
    advances: new Int16Array(n),
    offsets: new Int16Array(2 * n),
    flags: new Uint8Array(n),
    levels: new Uint8Array(n),
    kprio: new Uint8Array(n),
    kmax: new Uint16Array(n),
  };
  let at = 0;
  let dominant = { font: 0, glyphs: -1 };
  for (const r of block.runs) {
    if (r.gids.length > dominant.glyphs) dominant = { font: r.font, glyphs: r.gids.length };
    run.gids.set(r.gids, at);
    run.advances.set(r.advances, at);
    run.flags.set(r.flags, at);
    run.levels.set(r.bidiLevels, at);
    if (r.offsets.length === 2 * r.gids.length) run.offsets.set(r.offsets, 2 * at);
    if (r.kashidaPriority.length === r.gids.length) run.kprio.set(r.kashidaPriority, at);
    if (r.kashidaMax.length === r.gids.length) run.kmax.set(r.kashidaMax, at);
    at += r.gids.length;
  }
  return { run, font: dominant.font };
}

/**
 * Lays out a chunk. `compositor` is a wasm `Compositor`; `atlas` is the
 * edition's glyph table; `style` sets sizes and spacing.
 */
export function layoutChunk(
  compositor: Compositor,
  chunk: FlowChunk,
  atlas: GlyphTable,
  style: ReaderStyle = DEFAULT_STYLE,
  dpr = 1,
): LaidOutPage {
  const margin = style.margin * dpr;
  const pageWidth = Math.round(640 * dpr);
  const measure = pageWidth - 2 * margin;
  const out: number[] = [];
  let cursor = margin;

  for (const block of chunk.blocks) {
    if (block.runs.length === 0 || block.kind === "rule") continue;
    const { run, font } = concat(block);
    const sizePx = style.sizePx[font === 1 ? 1 : 0]! * dpr;
    // A representative texels-per-unit for the block's font (constant per font).
    const tpu = representativeTpu(run.gids, atlas);
    if (tpu === undefined) continue;
    const scale = (sizePx * tpu) / EM_TEXELS;
    const ppt = sizePx / EM_TEXELS;
    const pitch = sizePx * style.leading;
    cursor += block.kind === "heading" ? style.headingGap * sizePx : style.paragraphGap * sizePx;

    const lines = compositor.layout_paragraph(
      run.gids,
      run.advances,
      run.offsets,
      run.flags,
      run.levels,
      run.kprio,
      run.kmax,
      scale,
      measure,
      block.dir === "rtl" ? 1 : 0,
      0,
      0,
      0,
      0,
    );
    for (let line = 0; line < lines; line++) {
      const baseline = cursor + pitch * 0.75;
      emitLine(out, compositor.line_glyphs(line), atlas, margin, baseline, ppt);
      cursor += pitch;
    }
  }

  const instances = new Float32Array(out);
  return {
    instances,
    count: instances.length / FLOATS,
    width: pageWidth,
    height: Math.ceil(cursor + margin),
  };
}

function representativeTpu(gids: Uint16Array, atlas: GlyphTable): number | undefined {
  for (const gid of gids) {
    const g = atlas.glyphs.get(gid);
    if (g !== undefined) return g.texelsPerUnit;
  }
  return undefined;
}

/** One line's positioned glyphs (`[gid, x, y, scale_x, kind]*`) → instances. */
function emitLine(
  out: number[],
  glyphs: Float32Array,
  atlas: GlyphTable,
  left: number,
  baseline: number,
  ppt: number,
): void {
  for (let i = 0; i + 5 <= glyphs.length; i += 5) {
    const gid = glyphs[i]!;
    const x = glyphs[i + 1]!;
    const y = glyphs[i + 2]!;
    const scaleX = glyphs[i + 3]!;
    const entry = atlas.glyphs.get(gid);
    if (entry === undefined) continue;
    const [ox, oy] = entry.origin;
    for (const [sx, sy, sw, sh] of entry.slots) {
      // rect
      out.push(left + x - ox * ppt * scaleX, baseline - y - oy * ppt, sw * ppt * scaleX, sh * ppt);
      // slot
      out.push(sx, sy, sw, sh);
      // role (ink), highlight
      out.push(0, 0);
    }
  }
}

/** Parses the fixture/manifest glyph-table JSON into a [`GlyphTable`]. */
export function glyphTableFromJson(json: {
  pxRange: number;
  width: number;
  height: number;
  glyphs: Record<string, { o: [number, number]; t: number; s: [number, number, number, number][] }>;
}): GlyphTable {
  const glyphs = new Map<number, AtlasGlyph>();
  for (const [id, g] of Object.entries(json.glyphs)) {
    glyphs.set(Number(id), { origin: g.o, texelsPerUnit: g.t, slots: g.s });
  }
  return { pxRange: json.pxRange, width: json.width, height: json.height, glyphs };
}
