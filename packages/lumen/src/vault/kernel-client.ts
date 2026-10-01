/**
 * Kernel client for the Vault Worker: DPoP on every request, transparent
 * nonce handling, and device registration.
 *
 * Nonce dance (RFC 9449 §8–9): the Kernel answers a proof without a valid
 * nonce with `use_dpop_nonce` and a `DPoP-Nonce` header. The client retries
 * once with that nonce and remembers the freshest nonce from every response,
 * so the steady state is one round trip per call.
 */
import { createDpopProof } from "./dpop.ts";
import type { DeviceKeys } from "./device-keys.ts";
import { publicJwk, thumbprint, type PublicP256Jwk } from "./jose.ts";

export interface Registration {
  readonly device_id: string;
  readonly access_token: string;
  readonly token_type: "DPoP";
  readonly expires_in: number;
  readonly dpop_jkt: string;
}

export class KernelError extends Error {
  override readonly name = "KernelError";
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, description: string) {
    super(`${status} ${code}: ${description}`);
    this.status = status;
    this.code = code;
  }
}

export class KernelClient {
  readonly #origin: string;
  readonly #keys: DeviceKeys;
  readonly #fetch: typeof fetch;
  #jwk: PublicP256Jwk | undefined;
  #nonce: string | undefined;
  #token: string | undefined;

  constructor(origin: string, keys: DeviceKeys, fetchImpl: typeof fetch = globalThis.fetch) {
    this.#origin = origin.replace(/\/$/u, "");
    this.#keys = keys;
    // Never call fetch with `this` = the client: browsers throw "Illegal
    // invocation" (Node tolerates it). Caught by the Chromium e2e test.
    this.#fetch = (input, init) => fetchImpl.call(globalThis, input, init);
  }

  async jkt(): Promise<string> {
    return thumbprint(await this.#signJwk());
  }

  async #signJwk(): Promise<PublicP256Jwk> {
    this.#jwk ??= await publicJwk(this.#keys.sign.publicKey);
    return this.#jwk;
  }

  async #send(method: string, path: string, body: unknown, withToken: boolean): Promise<Response> {
    const url = `${this.#origin}${path}`;
    const jwk = await this.#signJwk();
    for (let attempt = 0; attempt < 2; attempt++) {
      const headers: Record<string, string> = {
        DPoP: await createDpopProof(
          { key: this.#keys.sign, method, url, nonce: this.#nonce, accessToken: withToken ? this.#token : undefined },
          jwk,
        ),
      };
      if (withToken) {
        if (this.#token === undefined) throw new KernelError(0, "not_registered", "register the device first");
        headers["Authorization"] = `DPoP ${this.#token}`;
      }
      if (body !== undefined) headers["Content-Type"] = "application/json";
      const res = await this.#fetch(url, { method, headers, body: body === undefined ? null : JSON.stringify(body) });
      const nonce = res.headers.get("DPoP-Nonce");
      if (nonce !== null) this.#nonce = nonce;
      if (res.ok) return res;
      const err = (await res.json().catch(() => ({}))) as { error?: string; error_description?: string };
      if (err.error === "use_dpop_nonce" && attempt === 0 && nonce !== null) continue;
      throw new KernelError(res.status, err.error ?? "http_error", err.error_description ?? res.statusText);
    }
    throw new KernelError(0, "nonce_loop", "nonce challenge repeated");
  }

  /** Proves possession of DeviceKey-Sign, registers DeviceKey-ECDH, obtains a DPoP-bound token. */
  async register(platform: Record<string, string | number | boolean> = {}): Promise<Registration> {
    const ecdh = await publicJwk(this.#keys.ecdh.publicKey);
    const res = await this.#send("POST", "/kernel/v1/devices", { ecdh_public_jwk: ecdh, platform }, false);
    const reg = (await res.json()) as Registration;
    if (reg.dpop_jkt !== (await this.jkt())) throw new KernelError(0, "jkt_mismatch", "Kernel bound the token to another key");
    this.#token = reg.access_token;
    return reg;
  }

  async self(): Promise<{ id: string; dpop_jkt: string }> {
    const res = await this.#send("GET", "/kernel/v1/devices/self", undefined, true);
    return (await res.json()) as { id: string; dpop_jkt: string };
  }
}
