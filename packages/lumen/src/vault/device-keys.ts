/**
 * Device identity for the Vault Worker (blueprint 01 §3.2):
 *
 *   DeviceKey-ECDH  P-256 ECDH   receives lease keys (wrapped by the Kernel)
 *   DeviceKey-Sign  P-256 ECDSA  signs every DPoP proof
 *
 * Both private keys are generated NON-EXTRACTABLE: no code on this page,
 * including ours, can ever read their bytes. Page code can still *use* them,
 * which is why sessions are additionally watched by Sentinel. The CryptoKey
 * objects persist in IndexedDB via structured clone, which keeps them
 * non-extractable across reloads.
 */

export interface DeviceKeys {
  readonly ecdh: CryptoKeyPair;
  readonly sign: CryptoKeyPair;
}

export interface KeyStore {
  load(): Promise<DeviceKeys | undefined>;
  save(keys: DeviceKeys): Promise<void>;
  clear(): Promise<void>;
}

export async function generateDeviceKeys(): Promise<DeviceKeys> {
  const ecdh = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
  const sign = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
  assertNonExtractable(ecdh);
  assertNonExtractable(sign);
  return { ecdh, sign };
}

function assertNonExtractable(pair: CryptoKeyPair): void {
  if (pair.privateKey.extractable) throw new Error("device key generated extractable");
}

export async function loadOrCreateDeviceKeys(store: KeyStore): Promise<{ keys: DeviceKeys; created: boolean }> {
  const existing = await store.load();
  if (existing !== undefined && !existing.sign.privateKey.extractable && !existing.ecdh.privateKey.extractable) {
    return { keys: existing, created: false };
  }
  const keys = await generateDeviceKeys();
  await store.save(keys);
  return { keys, created: true };
}

/** In-memory store (tests, and the fallback when storage is unavailable). */
export class MemoryKeyStore implements KeyStore {
  #keys: DeviceKeys | undefined;

  load(): Promise<DeviceKeys | undefined> {
    return Promise.resolve(this.#keys);
  }

  save(keys: DeviceKeys): Promise<void> {
    this.#keys = keys;
    return Promise.resolve();
  }

  clear(): Promise<void> {
    this.#keys = undefined;
    return Promise.resolve();
  }
}

const DB = "sanad-vault";
const STORE = "device";
const KEY = "keys/v1";

function request<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

/** IndexedDB store, available in workers. Holds CryptoKey handles, never key bytes. */
export class IndexedDbKeyStore implements KeyStore {
  #db: Promise<IDBDatabase> | undefined;

  #open(): Promise<IDBDatabase> {
    this.#db ??= new Promise((resolve, reject) => {
      const open = indexedDB.open(DB, 1);
      open.onupgradeneeded = () => open.result.createObjectStore(STORE);
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    });
    return this.#db;
  }

  async load(): Promise<DeviceKeys | undefined> {
    const db = await this.#open();
    const value: unknown = await request(db.transaction(STORE, "readonly").objectStore(STORE).get(KEY));
    return isDeviceKeys(value) ? value : undefined;
  }

  async save(keys: DeviceKeys): Promise<void> {
    const db = await this.#open();
    await request(db.transaction(STORE, "readwrite").objectStore(STORE).put(keys, KEY));
  }

  async clear(): Promise<void> {
    const db = await this.#open();
    await request(db.transaction(STORE, "readwrite").objectStore(STORE).delete(KEY));
  }
}

function isPair(v: unknown): v is CryptoKeyPair {
  return typeof v === "object" && v !== null && (v as CryptoKeyPair).privateKey instanceof CryptoKey && (v as CryptoKeyPair).publicKey instanceof CryptoKey;
}

function isDeviceKeys(v: unknown): v is DeviceKeys {
  return typeof v === "object" && v !== null && isPair((v as DeviceKeys).ecdh) && isPair((v as DeviceKeys).sign);
}
