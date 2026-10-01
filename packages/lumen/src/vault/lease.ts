/**
 * Lease unwrap in the Vault Worker (blueprint 01 §3.3):
 *
 *   Z   = ECDH(DeviceKey-ECDH.private, lease.ephemeral_public)
 *   KEK = HKDF-SHA256(Z, salt = lease_id bytes, info = "sanad/lease/v1")
 *   CK  = AES-KW-unwrap(KEK, wrapped)  → non-extractable AES-GCM, decrypt only
 *
 * Every intermediate is a non-extractable CryptoKey except Z, which WebCrypto
 * returns as bytes and we zero after use (hygiene, not a boundary). The chunk
 * keys can decrypt, and nothing else: no export, no encrypt, no wrap.
 */
import { uuidToBytes } from "./folio.ts";
import { b64urlDecode } from "./jose.ts";

export const LEASE_INFO = new TextEncoder().encode("sanad/lease/v1");

export interface LeaseKey {
  readonly chunk: number;
  readonly variant: number;
  /** AES-KW of the 256-bit chunk key (40 bytes), base64url. */
  readonly wrapped: string;
}

export interface Lease {
  readonly lease_id: string;
  readonly edition_id: string;
  /** `[start, end)` chunk window. */
  readonly window: readonly [number, number];
  readonly expires_in: number;
  readonly ephemeral_public_jwk: JsonWebKey;
  readonly keys: readonly LeaseKey[];
  /** Reading Capability Token for Oracle, quotes and accessibility. */
  readonly rct: string;
}

export const chunkKeyId = (chunk: number, variant: number): string => `${chunk}/${variant}`;

/** Unwraps every key of `lease` with the device's ECDH private key. */
export async function unwrapLease(lease: Lease, deviceEcdh: CryptoKey): Promise<Map<string, CryptoKey>> {
  const ephemeral = await crypto.subtle.importKey("jwk", lease.ephemeral_public_jwk, { name: "ECDH", namedCurve: "P-256" }, false, []);
  const z = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: ephemeral }, deviceEcdh, 256));
  let kek: CryptoKey;
  try {
    const ikm = await crypto.subtle.importKey("raw", z, "HKDF", false, ["deriveKey"]);
    kek = await crypto.subtle.deriveKey(
      { name: "HKDF", hash: "SHA-256", salt: uuidToBytes(lease.lease_id), info: LEASE_INFO },
      ikm,
      { name: "AES-KW", length: 256 },
      false,
      ["unwrapKey"],
    );
  } finally {
    z.fill(0);
  }
  const out = new Map<string, CryptoKey>();
  for (const k of lease.keys) {
    const ck = await crypto.subtle.unwrapKey("raw", b64urlDecode(k.wrapped), kek, "AES-KW", { name: "AES-GCM" }, false, ["decrypt"]);
    out.set(chunkKeyId(k.chunk, k.variant), ck);
  }
  return out;
}
