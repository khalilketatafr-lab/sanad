/** Minimal JOSE helpers for the Vault Worker (WebCrypto only, no dependencies). */

const encoder = new TextEncoder();

export function b64url(bytes: ArrayBuffer | Uint8Array): string {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(s).replace(/\+/gu, "-").replace(/\//gu, "_").replace(/=+$/u, "");
}

export function b64urlJson(value: unknown): string {
  return b64url(encoder.encode(JSON.stringify(value)));
}

/** Public EC P-256 JWK with only the members RFC 7638 hashes. */
export interface PublicP256Jwk {
  readonly kty: "EC";
  readonly crv: "P-256";
  readonly x: string;
  readonly y: string;
}

/**
 * Exports a public key as a minimal JWK. WebCrypto adds `ext` and `key_ops`;
 * they are dropped so the JWK we send is exactly what gets thumbprinted.
 */
export async function publicJwk(key: CryptoKey): Promise<PublicP256Jwk> {
  if (key.type !== "public") throw new TypeError("publicJwk: expected a public key");
  const jwk = await crypto.subtle.exportKey("jwk", key);
  if (jwk.kty !== "EC" || jwk.crv !== "P-256" || jwk.x === undefined || jwk.y === undefined) {
    throw new TypeError("publicJwk: not an EC P-256 key");
  }
  return { kty: "EC", crv: "P-256", x: jwk.x, y: jwk.y };
}

/** RFC 7638 thumbprint: SHA-256 over the required members in lexicographic order. */
export async function thumbprint(jwk: PublicP256Jwk): Promise<string> {
  const canonical = `{"crv":"P-256","kty":"EC","x":"${jwk.x}","y":"${jwk.y}"}`;
  return b64url(await crypto.subtle.digest("SHA-256", encoder.encode(canonical)));
}

export async function sha256b64url(text: string): Promise<string> {
  return b64url(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}
