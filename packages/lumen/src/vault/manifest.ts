/**
 * The edition manifest: the first thing the reader fetches when opening an
 * edition (blueprint 03 §2, "Manifest — structure, TOC, font metrics, style
 * table — public, signed"). It carries the object-storage layout (which sealed
 * chunk is which chapter/variant) and the atlas geometry (every permuted glyph
 * id's origin, texels-per-unit and shredded-fragment slots), but no text and no
 * keys.
 *
 * It is public but signed: Atelier seals it with Ed25519 (crates/atelier/
 * src/publish.rs `seal_edition`) over the exact `manifest.json` bytes, with a
 * detached `manifest.sig` and the raw public key alongside. The reader verifies
 * the signature against a *pinned* publisher key — never a key read from the
 * same untrusted store — before trusting a single field, so a tampered manifest
 * (a swapped chunk path, a forged atlas slot) is rejected. Verification is
 * WebCrypto Ed25519; the bytes are verified as received and only then parsed.
 */

/** Must match `crates/atelier/src/publish.rs` `MANIFEST_SCHEMA`. */
export const MANIFEST_SCHEMA = 1;

export interface ManifestSlot {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

export interface ManifestGlyph {
  readonly id: number;
  /** Ink origin in the slot, texels. */
  readonly origin: readonly [number, number];
  /** Atlas texels per font unit (constant per font). */
  readonly texelsPerUnit: number;
  readonly slots: readonly ManifestSlot[];
}

export interface AtlasPage {
  readonly page: number;
  /** Storage-relative path of the sealed atlas page. */
  readonly path: string;
  readonly width: number;
  readonly height: number;
  readonly coverage: number;
  readonly glyphs: readonly ManifestGlyph[];
}

export interface ChunkRef {
  readonly chapter: number;
  /** Storage-relative, opaque path of the sealed Folio chunk. */
  readonly path: string;
  readonly chunkIndex: number;
  readonly variant: number;
  /** Folio chunk flags (ZSTD, LAST_IN_CHAPTER, …). */
  readonly flags: number;
  /** SHA-256 (hex) of the compressed plaintext, for integrity before sealing. */
  readonly plaintextSha256: string;
  readonly sealedLen: number;
}

export interface Manifest {
  readonly schema: number;
  readonly editionId: string;
  readonly version: number;
  readonly kind: string;
  readonly variants: number;
  readonly fonts: readonly string[];
  readonly pxRange: number;
  readonly chunks: readonly ChunkRef[];
  readonly atlas: readonly AtlasPage[];
}

export class ManifestError extends Error {
  override readonly name = "ManifestError";
}

const dec = new TextDecoder("utf-8", { fatal: true });

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function num(o: Record<string, unknown>, key: string): number {
  const v = o[key];
  if (typeof v !== "number" || !Number.isFinite(v)) throw new ManifestError(`manifest.${key}: expected a number`);
  return v;
}

function str(o: Record<string, unknown>, key: string): string {
  const v = o[key];
  if (typeof v !== "string") throw new ManifestError(`manifest.${key}: expected a string`);
  return v;
}

function arr(o: Record<string, unknown>, key: string): unknown[] {
  const v = o[key];
  if (!Array.isArray(v)) throw new ManifestError(`manifest.${key}: expected an array`);
  return v;
}

function obj(v: unknown, where: string): Record<string, unknown> {
  if (!isObject(v)) throw new ManifestError(`${where}: expected an object`);
  return v;
}

function slot(v: unknown): ManifestSlot {
  const o = obj(v, "atlas glyph slot");
  return { x: num(o, "x"), y: num(o, "y"), w: num(o, "w"), h: num(o, "h") };
}

function glyph(v: unknown): ManifestGlyph {
  const o = obj(v, "atlas glyph");
  const origin = arr(o, "origin");
  if (origin.length !== 2 || typeof origin[0] !== "number" || typeof origin[1] !== "number") {
    throw new ManifestError("atlas glyph origin: expected [number, number]");
  }
  return {
    id: num(o, "id"),
    origin: [origin[0], origin[1]],
    texelsPerUnit: num(o, "texels_per_unit"),
    slots: arr(o, "slots").map(slot),
  };
}

function atlasPage(v: unknown): AtlasPage {
  const o = obj(v, "atlas page");
  return {
    page: num(o, "page"),
    path: str(o, "path"),
    width: num(o, "width"),
    height: num(o, "height"),
    coverage: num(o, "coverage"),
    glyphs: arr(o, "glyphs").map(glyph),
  };
}

function chunkRef(v: unknown): ChunkRef {
  const o = obj(v, "chunk entry");
  return {
    chapter: num(o, "chapter"),
    path: str(o, "path"),
    chunkIndex: num(o, "chunk_index"),
    variant: num(o, "variant"),
    flags: num(o, "flags"),
    plaintextSha256: str(o, "plaintext_sha256"),
    sealedLen: num(o, "sealed_len"),
  };
}

/**
 * Parses (and validates) manifest JSON — a string, or the raw UTF-8 bytes. The
 * snake_case wire fields become a typed, camelCase [`Manifest`]. Throws
 * [`ManifestError`] on a schema mismatch or any malformed field.
 */
export function parseManifest(input: string | Uint8Array): Manifest {
  let parsed: unknown;
  try {
    parsed = JSON.parse(typeof input === "string" ? input : dec.decode(input));
  } catch (e) {
    throw new ManifestError(`manifest is not valid JSON: ${String(e)}`);
  }
  const o = obj(parsed, "manifest");
  const schema = num(o, "schema");
  if (schema !== MANIFEST_SCHEMA) {
    throw new ManifestError(`manifest schema ${schema} is not supported (expected ${MANIFEST_SCHEMA})`);
  }
  const fonts = arr(o, "fonts").map((f, i) => {
    if (typeof f !== "string") throw new ManifestError(`manifest.fonts[${i}]: expected a string`);
    return f;
  });
  return {
    schema,
    editionId: str(o, "edition_id"),
    version: num(o, "version"),
    kind: str(o, "kind"),
    variants: num(o, "variants"),
    fonts,
    pxRange: num(o, "px_range"),
    chunks: arr(o, "chunks").map(chunkRef),
    atlas: arr(o, "atlas").map(atlasPage),
  };
}

/**
 * Verifies the manifest's detached Ed25519 signature over the exact bytes, with
 * the pinned 32-byte raw public key. Returns `false` on a bad signature;
 * resolves true only for an authentic manifest.
 */
export async function verifyManifestSignature(
  bytes: Uint8Array,
  signature: Uint8Array,
  publicKey: Uint8Array,
): Promise<boolean> {
  let key: CryptoKey;
  try {
    // `.slice()` yields an ArrayBuffer-backed view (a valid BufferSource).
    key = await crypto.subtle.importKey("raw", publicKey.slice(), { name: "Ed25519" }, false, ["verify"]);
  } catch (e) {
    throw new ManifestError(`manifest public key is not a valid Ed25519 key: ${String(e)}`);
  }
  return crypto.subtle.verify("Ed25519", key, signature.slice(), bytes.slice());
}

/**
 * Verifies and parses a manifest in one step: the only safe way to obtain a
 * [`Manifest`] from the network. Throws [`ManifestError`] if the signature does
 * not verify against the pinned key, before any field is read.
 */
export async function openManifest(bytes: Uint8Array, signature: Uint8Array, publicKey: Uint8Array): Promise<Manifest> {
  if (!(await verifyManifestSignature(bytes, signature, publicKey))) {
    throw new ManifestError("manifest signature does not verify against the pinned publisher key");
  }
  return parseManifest(bytes);
}

/** The chunk entry for a `(chunkIndex, variant)`, or `undefined` if absent. */
export function chunk(manifest: Manifest, chunkIndex: number, variant = 0): ChunkRef | undefined {
  return manifest.chunks.find((c) => c.chunkIndex === chunkIndex && c.variant === variant);
}

/** The atlas page by index, or `undefined`. */
export function page(manifest: Manifest, index: number): AtlasPage | undefined {
  return manifest.atlas.find((a) => a.page === index);
}
