/** Runs in Chromium on the reader origin: real IndexedDB, real CORS, real WebCrypto. */
import { IndexedDbKeyStore, loadOrCreateDeviceKeys } from "../../src/vault/device-keys.ts";
import { KernelClient } from "../../src/vault/kernel-client.ts";

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
