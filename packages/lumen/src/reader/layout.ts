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
import type { AtlasPage } from "../vault/manifest.ts";
import type { Anchor } from "./lifecycle.ts";

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

/** Page width before margins, CSS px (the reading measure drives the rest). */
const PAGE_WIDTH = 640;

/**
 * One laid-out line, in device px, captured with its positioned glyphs so it
 * can be placed either in one tall page (`layoutChunk`) or on a viewport page
 * (`paginateChunk`). The baseline sits `baselineOffset` below the line's top.
 */
interface LaidLine {
  readonly glyphs: Float32Array;
  readonly left: number;
  readonly ppt: number;
  /** Space added above the line (the block gap, on a block's first line only). */
  readonly gapBefore: number;
  readonly pitch: number;
  readonly baselineOffset: number;
  readonly blockIndex: number;
  readonly kind: FlowBlock["kind"];
}

interface LaidLines {
  readonly lines: readonly LaidLine[];
  readonly width: number;
  readonly margin: number;
}

/**
 * Runs every block through the Compositor and captures the positioned glyphs of
 * each line (`line_glyphs` returns a copy, so a line survives the next
 * paragraph's layout). The one place the Compositor is driven; both the scroll
 * layout and the pager build on it.
 */
function layoutLines(compositor: Compositor, chunk: FlowChunk, atlas: GlyphTable, style: ReaderStyle, dpr: number): LaidLines {
  const margin = style.margin * dpr;
  const width = Math.round(PAGE_WIDTH * dpr);
  const measure = width - 2 * margin;
  const lines: LaidLine[] = [];

  chunk.blocks.forEach((block, blockIndex) => {
    if (block.runs.length === 0 || block.kind === "rule") return;
    const { run, font } = concat(block);
    const sizePx = style.sizePx[font === 1 ? 1 : 0]! * dpr;
    // A representative texels-per-unit for the block's font (constant per font).
    const tpu = representativeTpu(run.gids, atlas);
    if (tpu === undefined) return;
    const scale = (sizePx * tpu) / EM_TEXELS;
    const ppt = sizePx / EM_TEXELS;
    const pitch = sizePx * style.leading;
    const gap = (block.kind === "heading" ? style.headingGap : style.paragraphGap) * sizePx;

    const n = compositor.layout_paragraph(
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
    for (let line = 0; line < n; line++) {
      lines.push({
        glyphs: compositor.line_glyphs(line),
        left: margin,
        ppt,
        gapBefore: line === 0 ? gap : 0,
        pitch,
        baselineOffset: pitch * 0.75,
        blockIndex,
        kind: block.kind,
      });
    }
  });

  return { lines, width, margin };
}

/**
 * Lays out a chunk as one tall page (scroll flow). `compositor` is a wasm
 * `Compositor`; `atlas` is the edition's glyph table; `style` sets sizes and
 * spacing.
 */
export function layoutChunk(
  compositor: Compositor,
  chunk: FlowChunk,
  atlas: GlyphTable,
  style: ReaderStyle = DEFAULT_STYLE,
  dpr = 1,
): LaidOutPage {
  const { lines, width, margin } = layoutLines(compositor, chunk, atlas, style, dpr);
  const out: number[] = [];
  let cursor = margin;
  for (const ln of lines) {
    cursor += ln.gapBefore;
    emitLine(out, ln.glyphs, atlas, ln.left, cursor + ln.baselineOffset, ln.ppt);
    cursor += ln.pitch;
  }
  const instances = new Float32Array(out);
  return { instances, count: instances.length / FLOATS, width, height: Math.ceil(cursor + margin) };
}

// ── Pagination (03 §3.2) ───────────────────────────────────────────────────

/** The metrics of one line the pager needs; `planPages` is pure over these. */
export interface PageBox {
  readonly gapBefore: number;
  readonly pitch: number;
  readonly blockIndex: number;
  readonly kind: FlowBlock["kind"];
}

/** A paged page: ready-to-render instances for a fixed-height viewport. */
export interface ReaderPage {
  readonly instances: Float32Array;
  readonly count: number;
  readonly width: number;
  readonly height: number;
  /** The anchor of the page's first line (block granularity in Phase 1a). */
  readonly anchor: Anchor;
}

export interface PagedChunk {
  readonly pages: readonly ReaderPage[];
  readonly width: number;
}

/**
 * The legal page-break points: `breakable[k]` is true iff a page may start at
 * line `k`. Encodes the blueprint's widow/orphan and heading rules (03 §3.2):
 * no break in the two lines after a heading, no lone first line of a multi-line
 * block left at the foot of a page (orphan), and no lone last line carried to
 * the head of a page (widow).
 */
function breakablePoints(boxes: readonly PageBox[]): boolean[] {
  const n = boxes.length;
  const b = new Array<boolean>(n).fill(true);
  const firstOf = (k: number): boolean => k === 0 || boxes[k]!.blockIndex !== boxes[k - 1]!.blockIndex;
  const lastOf = (k: number): boolean => k === n - 1 || boxes[k]!.blockIndex !== boxes[k + 1]!.blockIndex;
  for (let k = 1; k < n; k++) {
    let ok = true;
    if (boxes[k - 1]!.kind === "heading") {
      ok = false; // never strand a heading at the foot of a page
    } else if (k >= 2 && boxes[k - 2]!.kind === "heading" && boxes[k - 1]!.blockIndex !== boxes[k - 2]!.blockIndex) {
      ok = false; // keep a heading with at least its first two following lines
    }
    if (ok && firstOf(k - 1) && !lastOf(k - 1)) ok = false; // orphan
    if (ok && lastOf(k) && !firstOf(k)) ok = false; // widow
    b[k] = ok;
  }
  return b;
}

/**
 * Splits lines into `[start, end)` page ranges so each page's content fits
 * `avail` device px, honouring the legal break points. Breaks are moved earlier
 * to a legal point; a single line taller than `avail`, or the absence of any
 * legal earlier break, still makes progress (one line minimum per page).
 */
export function planPages(boxes: readonly PageBox[], avail: number): [number, number][] {
  const n = boxes.length;
  if (n === 0) return [];
  const breakable = breakablePoints(boxes);
  const ranges: [number, number][] = [];
  let start = 0;
  while (start < n) {
    let height = 0;
    let end = start;
    while (end < n) {
      const lineH = (end === start ? 0 : boxes[end]!.gapBefore) + boxes[end]!.pitch;
      if (end > start && height + lineH > avail) break;
      height += lineH;
      end++;
    }
    if (end === start) end = start + 1;
    if (end < n && !breakable[end]) {
      let e = end;
      while (e > start + 1 && !breakable[e]) e--;
      if (breakable[e] === true && e > start) end = e;
    }
    ranges.push([start, end]);
    start = end;
  }
  return ranges;
}

/**
 * Lays a chunk out into fixed-height viewport pages (paged flow), with stable,
 * memoizable boundaries: pages depend only on `(chunk, style, viewport, dpr)`.
 * `chunkIndex` sets the pages' anchors. `viewportHeightPx` is CSS px.
 */
export function paginateChunk(
  compositor: Compositor,
  chunk: FlowChunk,
  chunkIndex: number,
  atlas: GlyphTable,
  viewportHeightPx: number,
  style: ReaderStyle = DEFAULT_STYLE,
  dpr = 1,
): PagedChunk {
  const { lines, width, margin } = layoutLines(compositor, chunk, atlas, style, dpr);
  const pageHeight = Math.round(viewportHeightPx * dpr);
  const avail = Math.max(1, pageHeight - 2 * margin);
  const boxes: PageBox[] = lines.map((l) => ({ gapBefore: l.gapBefore, pitch: l.pitch, blockIndex: l.blockIndex, kind: l.kind }));
  const ranges = planPages(boxes, avail);

  const pages = ranges.map(([start, end]): ReaderPage => {
    const out: number[] = [];
    let cursor = margin;
    for (let k = start; k < end; k++) {
      const ln = lines[k]!;
      if (k !== start) cursor += ln.gapBefore; // no block gap at the top of a page
      emitLine(out, ln.glyphs, atlas, ln.left, cursor + ln.baselineOffset, ln.ppt);
      cursor += ln.pitch;
    }
    const instances = new Float32Array(out);
    return {
      instances,
      count: instances.length / FLOATS,
      width,
      height: pageHeight,
      anchor: [chunkIndex, lines[start]!.blockIndex, 0],
    };
  });

  return { pages, width };
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

/**
 * Builds a [`GlyphTable`] from a signed-manifest atlas page (see
 * `src/vault/manifest.ts`), so the reader lays out against the same geometry
 * the publisher signed. `pxRange` comes from the edition manifest.
 */
export function glyphTableFromManifest(atlasPage: AtlasPage, pxRange: number): GlyphTable {
  const glyphs = new Map<number, AtlasGlyph>();
  for (const g of atlasPage.glyphs) {
    glyphs.set(g.id, {
      origin: g.origin,
      texelsPerUnit: g.texelsPerUnit,
      slots: g.slots.map((s) => [s.x, s.y, s.w, s.h] as const),
    });
  }
  return { pxRange, width: atlasPage.width, height: atlasPage.height, glyphs };
}
