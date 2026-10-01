/** Runs in Chromium on the reader origin: real IndexedDB, real CORS, real WebCrypto. */
import { IndexedDbKeyStore, loadOrCreateDeviceKeys } from "../../src/vault/device-keys.ts";
import { openChunk } from "../../src/vault/folio.ts";
import { KernelClient } from "../../src/vault/kernel-client.ts";
import { chunkKeyId, unwrapLease } from "../../src/vault/lease.ts";

export async function run(kernelOrigin: string): Promise<Record<string, unknown>> {
  const store = new IndexedDbKeyStore();
  const { keys, created } = await loadOrCreateDeviceKeys(store);
  let exportBlocked = false;
  try {
    await crypto.subtle.exportKey("jwk", keys.sign.privateKey);
  } catch {
    exportBlocked = true;
  }
  const client = new KernelClient(kernelOrigin, keys);
  const reg = await client.register({ tier: "B" });
  const self = await client.self();
  return { created, exportBlocked, jkt: await client.jkt(), deviceId: reg.device_id, selfId: self.id, selfJkt: self.dpop_jkt };
}

/**
 * Opens the canary edition, unwraps the lease with the IndexedDB device key and
 * decrypts the window's chunks, served by the reader origin as a CDN would.
 */
export async function lease(kernelOrigin: string, editionId: string): Promise<Record<string, unknown>> {
  const { keys } = await loadOrCreateDeviceKeys(new IndexedDbKeyStore());
  const client = new KernelClient(kernelOrigin, keys);
  await client.register({ tier: "B" });
  const l = await client.openEdition(editionId);
  const cks = await unwrapLease(l, keys.ecdh.privateKey);
  const digests: Record<number, string> = {};
  const keyProps: { extractable: boolean; usages: string[] }[] = [];
  for (let chunk = l.window[0]; chunk < l.window[1]; chunk++) {
    const key = cks.get(chunkKeyId(chunk, 0));
    if (key === undefined) throw new Error(`no key for chunk ${chunk}`);
    keyProps.push({ extractable: key.extractable, usages: [...key.usages] });
    const sealed = new Uint8Array(await (await fetch(`/chunks/chunk-${chunk}-v0.folio`)).arrayBuffer());
    const plain = await openChunk(key, sealed, { kind: "flow", editionId, chunkIndex: chunk, variant: 0 });
    digests[chunk] = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", plain)), (b) => b.toString(16).padStart(2, "0")).join("");
  }
  return { window: l.window, digests, keyProps, rctPrefix: l.rct.slice(0, 10) };
}
