/**
 * Folio chunk container on the client: header validation and AES-256-GCM
 * open. Mirrors `crates/folio/src/codec.rs` rule for rule (the Rust parser is
 * fuzzed; this one is tested against chunks sealed by Atelier's code).
 *
 *   0  magic "FOLI" · 4 version 1 · 5 kind · 6 flags u16 LE · 8 edition_id
 *   24 chunk_index u32 LE · 28 variant · 29 reserved (zero) · 32 nonce (12)
 *   44 plaintext_len u32 LE · 48 ciphertext · tag (16)
 *
 *   AAD = bytes[0, 32): edition, index, variant and kind are authenticated.
 *
 * Decryption happens with a non-extractable key from the lease. The plaintext
 * is a FlatBuffer of permuted glyph ids: never text (P1).
 */
export const HEADER_LEN = 48;
export const AAD_LEN = 32;
export const NONCE_LEN = 12;
export const TAG_LEN = 16;
export const MAX_PLAINTEXT_LEN = 4 * 1024 * 1024;
const MAGIC = [0x46, 0x4f, 0x4c, 0x49]; // "FOLI"
const VERSION = 1;
const MAX_VARIANTS = 4;
const FLAG_ZSTD = 1 << 0;
const FLAG_HAS_VARIANT = 1 << 1;
const FLAG_LAST_IN_CHAPTER = 1 << 2;
const KNOWN_FLAGS = FLAG_ZSTD | FLAG_HAS_VARIANT | FLAG_LAST_IN_CHAPTER;

export const CHUNK_KINDS = ["flow", "page", "atlas-page", "tile"] as const;
export type ChunkKind = (typeof CHUNK_KINDS)[number];

export class FolioError extends Error {
  override readonly name = "FolioError";
}

export interface ChunkHeader {
  readonly kind: ChunkKind;
  readonly flags: number;
  /** Canonical lowercase UUID. */
  readonly editionId: string;
  readonly chunkIndex: number;
  readonly variant: number;
  readonly plaintextLen: number;
}

/** What a lease promises for an object. */
export interface ChunkIdentity {
  readonly kind: ChunkKind;
  readonly editionId: string;
  readonly chunkIndex: number;
  readonly variant: number;
}

export function uuidToBytes(uuid: string): Uint8Array<ArrayBuffer> {
  const hex = uuid.replace(/-/gu, "");
  if (!/^[0-9a-f]{32}$/iu.test(hex)) throw new FolioError(`not a UUID: ${uuid}`);
  return Uint8Array.from({ length: 16 }, (_, i) => Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16));
}

export function bytesToUuid(b: Uint8Array): string {
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** Validates header and framing; does no cryptography. */
export function parseChunk(bytes: Uint8Array): ChunkHeader {
  if (bytes.length < HEADER_LEN + TAG_LEN) throw new FolioError(`truncated: ${bytes.length} bytes`);
  if (MAGIC.some((m, i) => bytes[i] !== m)) throw new FolioError("bad magic");
  if (bytes[4] !== VERSION) throw new FolioError(`unsupported version ${bytes[4]}`);
  const kind = CHUNK_KINDS[bytes[5] ?? 255];
  if (kind === undefined) throw new FolioError(`unknown kind ${bytes[5]}`);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const flags = view.getUint16(6, true);
  if ((flags & ~KNOWN_FLAGS) !== 0) throw new FolioError(`unknown flags 0x${flags.toString(16)}`);
  const variant = bytes[28] ?? 0;
  if (variant >= MAX_VARIANTS) throw new FolioError(`bad variant ${variant}`);
  if (variant !== 0 && (flags & FLAG_HAS_VARIANT) === 0) throw new FolioError("variant without the has-variant flag");
  if (bytes[29] !== 0 || bytes[30] !== 0 || bytes[31] !== 0) throw new FolioError("reserved bytes not zero");
  const plaintextLen = view.getUint32(44, true);
  if (plaintextLen > MAX_PLAINTEXT_LEN) throw new FolioError(`declared ${plaintextLen} bytes exceeds the limit`);
  if (bytes.length - HEADER_LEN - TAG_LEN !== plaintextLen) throw new FolioError("length does not match the header");
  return {
    kind,
    flags,
    editionId: bytesToUuid(bytes.subarray(8, 24)),
    chunkIndex: view.getUint32(24, true),
    variant,
    plaintextLen,
  };
}

/**
 * Checks the object is the one the lease asked for, then authenticates and
 * decrypts it. Returns the AEAD plaintext (still zstd-compressed if flagged).
 */
export async function openChunk(key: CryptoKey, bytes: Uint8Array, want: ChunkIdentity): Promise<ArrayBuffer> {
  const h = parseChunk(bytes);
  for (const field of ["kind", "editionId", "chunkIndex", "variant"] as const) {
    if (h[field] !== want[field]) throw new FolioError(`identity mismatch: ${field}`);
  }
  try {
    return await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: bytes.slice(AAD_LEN, AAD_LEN + NONCE_LEN), additionalData: bytes.slice(0, AAD_LEN), tagLength: 128 },
      key,
      bytes.slice(HEADER_LEN),
    );
  } catch {
    throw new FolioError("authentication failed");
  }
}
