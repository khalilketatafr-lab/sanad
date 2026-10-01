/**
 * DPoP proofs (RFC 9449 §4.2), signed in the Vault Worker with the
 * non-extractable DeviceKey-Sign.
 *
 * WebCrypto's ECDSA signature is the IEEE P1363 fixed-width r‖s encoding,
 * which is exactly JWS ES256 (RFC 7518 §3.4). No DER conversion is needed.
 */
import { b64url, b64urlJson, publicJwk, sha256b64url, type PublicP256Jwk } from "./jose.ts";

const encoder = new TextEncoder();

export interface ProofInput {
  readonly key: CryptoKeyPair;
  readonly method: string;
  /** Request URL. Query and fragment are stripped for `htu`. */
  readonly url: string;
  readonly nonce?: string | undefined;
  /** Present on resource requests: adds `ath`. */
  readonly accessToken?: string | undefined;
  /** Seconds since the Unix epoch (injectable for tests). */
  readonly now?: number;
}

export function htuOf(url: string): string {
  const u = new URL(url);
  u.search = "";
  u.hash = "";
  return u.href;
}

export async function createDpopProof(input: ProofInput, jwk?: PublicP256Jwk): Promise<string> {
  const header = { typ: "dpop+jwt", alg: "ES256", jwk: jwk ?? (await publicJwk(input.key.publicKey)) };
  const jtiBytes = crypto.getRandomValues(new Uint8Array(16));
  const claims: Record<string, string | number> = {
    jti: b64url(jtiBytes),
    htm: input.method.toUpperCase(),
    htu: htuOf(input.url),
    iat: input.now ?? Math.floor(Date.now() / 1000),
  };
  if (input.nonce !== undefined) claims["nonce"] = input.nonce;
  if (input.accessToken !== undefined) claims["ath"] = await sha256b64url(input.accessToken);
  const signingInput = `${b64urlJson(header)}.${b64urlJson(claims)}`;
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, input.key.privateKey, encoder.encode(signingInput));
  if (signature.byteLength !== 64) throw new Error(`unexpected ECDSA signature length ${signature.byteLength}`);
  return `${signingInput}.${b64url(signature)}`;
}
