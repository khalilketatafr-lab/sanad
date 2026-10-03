/**
 * Decoding a Flow chunk on the client: open (AES-GCM) → zstd inflate → read
 * the FlatBuffer into shaped runs. This is the reader's whole view of a book's
 * content, and it is only ever permuted glyph ids and layout metrics — never
 * Unicode (P1).
 *
 * The FlatBuffer is read by hand against `crates/folio/schemas/folio.fbs`
 * (vtable offsets mirror `folio_generated.rs`); there is no code-gen step in
 * the reader build. `test/reader-flow.test.ts` checks it against a chunk
 * sealed by Atelier, field for field.
 */
import { decompress as zstdDecompress } from "fzstd";

import { type ChunkIdentity, FolioError, openChunk } from "./folio.ts";

const FLAG_ZSTD = 1 << 0;
/** Body union discriminant for FlowChunk (`folio.fbs`). */
const BODY_FLOW_CHUNK = 1;

/** Paragraph base direction. */
export type Direction = "ltr" | "rtl";

/** Block kind, in the schema's enum order. */
export const BLOCK_KINDS = [
  "paragraph",
  "heading",
  "quote",
  "list-item",
  "figure",
  "table",
  "rule",
  "break",
] as const;
export type BlockKind = (typeof BLOCK_KINDS)[number];

/** One shaped run: one font, one style, one direction. Logical order. */
export interface FlowRun {
  readonly font: number;
  /** Permuted glyph ids (π_edition). Never Unicode. */
  readonly gids: Uint16Array;
  /** Advances, font units. */
  readonly advances: Int16Array;
  /** `[x, y]` displacement per glyph, font units; empty means all zero. */
  readonly offsets: Int16Array;
  /** GlyphFlags per glyph. */
  readonly flags: Uint8Array;
  /** Resolved bidi level per glyph. */
  readonly bidiLevels: Uint8Array;
  /** Kashida priority per glyph; empty if none. */
  readonly kashidaPriority: Uint8Array;
  /** Max kashida elongation per glyph, font units; empty if none. */
  readonly kashidaMax: Uint16Array;
}

export interface FlowBlock {
  readonly kind: BlockKind;
  readonly level: number;
  readonly dir: Direction;
  readonly runs: readonly FlowRun[];
}

export interface FlowChunk {
  readonly blocks: readonly FlowBlock[];
}

// ── Minimal FlatBuffers reader ────────────────────────────────────────────

class Reader {
  private readonly bytes: Uint8Array;
  private readonly view: DataView;
  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  /** Root table position (uoffset at 0; the file identifier is at 4..8). */
  root(): number {
    return this.indirect(0);
  }

  /** Absolute target of a uoffset stored at `at`. */
  indirect(at: number): number {
    return at + this.view.getUint32(at, true);
  }

  /** Absolute offset of a table field, or 0 when the field is absent. */
  field(table: number, vtableOffset: number): number {
    const vtable = table - this.view.getInt32(table, true);
    const vtableSize = this.view.getUint16(vtable, true);
    if (vtableOffset >= vtableSize) return 0;
    const rel = this.view.getUint16(vtable + vtableOffset, true);
    return rel === 0 ? 0 : table + rel;
  }

  u8(table: number, vt: number, dflt = 0): number {
    const o = this.field(table, vt);
    return o === 0 ? dflt : this.view.getUint8(o);
  }

  /** `[start, len]` of a vector field, or null when absent. */
  vector(table: number, vt: number): readonly [number, number] | null {
    const o = this.field(table, vt);
    if (o === 0) return null;
    const v = this.indirect(o);
    return [v + 4, this.view.getUint32(v, true)];
  }

  u16Vec(table: number, vt: number): Uint16Array {
    const info = this.vector(table, vt);
    if (info === null) return new Uint16Array(0);
    const [start, len] = info;
    const out = new Uint16Array(len);
    for (let i = 0; i < len; i++) out[i] = this.view.getUint16(start + i * 2, true);
    return out;
  }

  i16Vec(table: number, vt: number): Int16Array {
    const info = this.vector(table, vt);
    if (info === null) return new Int16Array(0);
    const [start, len] = info;
    const out = new Int16Array(len);
    for (let i = 0; i < len; i++) out[i] = this.view.getInt16(start + i * 2, true);
    return out;
  }

  u8Vec(table: number, vt: number): Uint8Array {
    const info = this.vector(table, vt);
    if (info === null) return new Uint8Array(0);
    const [start, len] = info;
    return this.bytes.slice(start, start + len);
  }

  /** `[x, y]` pairs of a GlyphOffset struct vector (4 bytes each), flattened. */
  offsetVec(table: number, vt: number): Int16Array {
    const info = this.vector(table, vt);
    if (info === null) return new Int16Array(0);
    const [start, len] = info;
    const out = new Int16Array(len * 2);
    for (let i = 0; i < len; i++) {
      out[2 * i] = this.view.getInt16(start + i * 4, true);
      out[2 * i + 1] = this.view.getInt16(start + i * 4 + 2, true);
    }
    return out;
  }

  /** Absolute positions of each table in a vector-of-tables field. */
  tableVec(table: number, vt: number): number[] {
    const info = this.vector(table, vt);
    if (info === null) return [];
    const [start, len] = info;
    const out: number[] = [];
    for (let i = 0; i < len; i++) out.push(this.indirect(start + i * 4));
    return out;
  }
}

// vtable offsets (folio_generated.rs)
const PAYLOAD_BODY_TYPE = 4;
const PAYLOAD_BODY = 6;
const FLOW_BLOCKS = 4;
const BLOCK_KIND = 4;
const BLOCK_LEVEL = 6;
const BLOCK_DIR = 10;
const BLOCK_RUNS = 12;
const RUN_FONT = 4;
const RUN_GIDS = 8;
const RUN_ADVANCES = 10;
const RUN_OFFSETS = 12;
const RUN_FLAGS = 14;
const RUN_BIDI = 16;
const RUN_KPRIO = 18;
const RUN_KMAX = 20;

function readRun(r: Reader, pos: number): FlowRun {
  return {
    font: r.u8(pos, RUN_FONT),
    gids: r.u16Vec(pos, RUN_GIDS),
    advances: r.i16Vec(pos, RUN_ADVANCES),
    offsets: r.offsetVec(pos, RUN_OFFSETS),
    flags: r.u8Vec(pos, RUN_FLAGS),
    bidiLevels: r.u8Vec(pos, RUN_BIDI),
    kashidaPriority: r.u8Vec(pos, RUN_KPRIO),
    kashidaMax: r.u16Vec(pos, RUN_KMAX),
  };
}

/** Parses an inflated FlowChunk FlatBuffer into blocks and runs. */
export function parseFlowChunk(buffer: ArrayBuffer | Uint8Array): FlowChunk {
  const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
  if (bytes.length < 8) throw new FolioError("payload too small");
  const r = new Reader(bytes);
  const payload = r.root();
  if (r.u8(payload, PAYLOAD_BODY_TYPE) !== BODY_FLOW_CHUNK) {
    throw new FolioError("payload is not a FlowChunk");
  }
  const bodyOffset = r.field(payload, PAYLOAD_BODY);
  if (bodyOffset === 0) throw new FolioError("FlowChunk body missing");
  const flow = r.indirect(bodyOffset);
  const blocks = r.tableVec(flow, FLOW_BLOCKS).map((pos): FlowBlock => {
    const kind = BLOCK_KINDS[r.u8(pos, BLOCK_KIND)] ?? "paragraph";
    return {
      kind,
      level: r.u8(pos, BLOCK_LEVEL),
      dir: r.u8(pos, BLOCK_DIR) === 1 ? "rtl" : "ltr",
      runs: r.tableVec(pos, BLOCK_RUNS).map((rp) => readRun(r, rp)),
    };
  });
  return { blocks };
}

/** Inflates a chunk plaintext if the header's zstd flag is set. */
export function inflateChunk(plaintext: ArrayBuffer | Uint8Array, flags: number): Uint8Array {
  const bytes = plaintext instanceof Uint8Array ? plaintext : new Uint8Array(plaintext);
  return (flags & FLAG_ZSTD) === 0 ? bytes : zstdDecompress(bytes);
}

/**
 * Opens a sealed Flow chunk end to end: AES-GCM decrypt (lease key) → zstd
 * inflate → FlatBuffer parse. `flags` comes from the chunk header the lease
 * delivered alongside the key.
 */
export async function openFlowChunk(
  key: CryptoKey,
  sealed: Uint8Array,
  want: ChunkIdentity,
  flags: number,
): Promise<FlowChunk> {
  const plaintext = await openChunk(key, sealed, want);
  return parseFlowChunk(inflateChunk(plaintext, flags));
}
