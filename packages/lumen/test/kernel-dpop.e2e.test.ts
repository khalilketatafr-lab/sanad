/**
 * End-to-end: the real `sanad-kernel` binary, driven by the real Vault
 * Worker code over HTTP, from Node (WebCrypto) and from Chromium on a
 * separate reader origin (CORS, IndexedDB-persisted non-extractable keys).
 *
 * Includes the stub lease: the Kernel wraps chunk keys to the device's ECDH
 * key; the Vault unwraps them with WebCrypto and decrypts chunks sealed by
 * Folio's own code (`seal_fixture`), served from the reader origin as a CDN.
 */
import assert from "node:assert/strict";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import type { AddressInfo } from "node:net";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";
import { MemoryKeyStore, loadOrCreateDeviceKeys } from "../src/vault/device-keys.ts";
import { createDpopProof } from "../src/vault/dpop.ts";
import { FolioError, openChunk } from "../src/vault/folio.ts";
import { KernelClient, KernelError } from "../src/vault/kernel-client.ts";
import { chunkKeyId, unwrapLease } from "../src/vault/lease.ts";

/** The development catalog's canary edition (crates/kernel/src/catalog.rs). */
const CANARY = "00000000-0000-7000-8000-00000000ca7a";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
let kernel: ChildProcess;
let kernelOrigin = "";
let reader: Server;
let readerOrigin = "";
let sealedDir = "";
let manifest: { chunk: number; variant: number; file: string; sha256: string }[] = [];

async function freePort(): Promise<number> {
  const s = createServer();
  await new Promise<void>((r) => s.listen(0, "127.0.0.1", r));
  const { port } = s.address() as AddressInfo;
  await new Promise<void>((r) => s.close(() => r()));
  return port;
}

before(async () => {
  execFileSync("cargo", ["build", "-q", "-p", "sanad-kernel"], { cwd: repo, stdio: "inherit" });
  sealedDir = mkdtempSync(join(tmpdir(), "folio-sealed-"));
  execFileSync("cargo", ["run", "-q", "-p", "sanad-folio", "--features", "seal", "--example", "seal_fixture", "--", sealedDir], { cwd: repo, stdio: "inherit" });
  manifest = JSON.parse(readFileSync(join(sealedDir, "manifest.json"), "utf8")) as typeof manifest;
  const readerPort = await freePort();
  readerOrigin = `http://localhost:${readerPort}`;
  const port = await freePort();
  kernelOrigin = `http://127.0.0.1:${port}`;
  kernel = spawn(join(repo, "target/debug/sanad-kernel"), [], {
    env: {
      ...process.env,
      SANAD_KERNEL_BIND: `127.0.0.1:${port}`,
      SANAD_KERNEL_PUBLIC_ORIGIN: kernelOrigin,
      SANAD_READER_ORIGINS: readerOrigin,
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise<void>((ok, fail) => {
    const timer = setTimeout(() => fail(new Error("kernel did not start")), 30_000);
    kernel.stdout?.on("data", (d: Buffer) => {
      if (d.toString().includes("listening on")) {
        clearTimeout(timer);
        ok();
      }
    });
    kernel.on("exit", (code) => fail(new Error(`kernel exited ${code}`)));
  });

  // Reader origin: serves the bundled Vault harness.
  const bundle = await build({ entryPoints: [join(here, "fixtures/vault-browser.ts")], bundle: true, write: false, format: "iife", globalName: "Vault", target: "es2022" });
  const js = bundle.outputFiles[0]?.text ?? "";
  reader = createServer((req, res) => {
    const chunk = /^\/chunks\/(chunk-\d+-v\d\.folio)$/u.exec(req.url ?? "");
    if (req.url === "/vault.js") res.writeHead(200, { "content-type": "text/javascript" }).end(js);
    else if (chunk?.[1] !== undefined) res.writeHead(200, { "content-type": "application/octet-stream" }).end(readFileSync(join(sealedDir, chunk[1])));
    else res.writeHead(200, { "content-type": "text/html" }).end('<!doctype html><script src="/vault.js"></script>');
  });
  await new Promise<void>((r) => reader.listen(readerPort, "127.0.0.1", r));
});

after(async () => {
  kernel.kill("SIGTERM");
  await new Promise<void>((r) => reader.close(() => r()));
});

test("Node WebCrypto client registers and calls a DPoP-protected route", async () => {
  const { keys } = await loadOrCreateDeviceKeys(new MemoryKeyStore());
  await assert.rejects(crypto.subtle.exportKey("jwk", keys.sign.privateKey), "private key must be non-extractable");
  const client = new KernelClient(kernelOrigin, keys);
  const reg = await client.register({ tier: "B" });
  // Cross-language RFC 7638: TS thumbprint == Rust thumbprint the token is bound to.
  assert.equal(reg.dpop_jkt, await client.jkt());
  assert.match(reg.access_token, /^v4\.public\./u);
  const self = await client.self();
  assert.equal(self.id, reg.device_id);
});

test("a replayed proof is refused by the running Kernel", async () => {
  const { keys } = await loadOrCreateDeviceKeys(new MemoryKeyStore());
  const client = new KernelClient(kernelOrigin, keys);
  const reg = await client.register();
  // Obtain a valid nonce, then send the very same proof twice.
  const probe = await fetch(`${kernelOrigin}/kernel/v1/devices/self`, { headers: { Authorization: `DPoP ${reg.access_token}`, DPoP: "x" } });
  const nonce = probe.headers.get("DPoP-Nonce") ?? undefined;
  const url = `${kernelOrigin}/kernel/v1/devices/self`;
  const proof = await createDpopProof({ key: keys.sign, method: "GET", url, nonce, accessToken: reg.access_token });
  const send = () => fetch(url, { headers: { Authorization: `DPoP ${reg.access_token}`, DPoP: proof } });
  assert.equal((await send()).status, 200);
  const replay = await send();
  assert.equal(replay.status, 401);
  assert.match(replay.headers.get("WWW-Authenticate") ?? "", /invalid_dpop_proof/u);
});

test("a stolen token is useless without the device's private key", async () => {
  const alice = new KernelClient(kernelOrigin, (await loadOrCreateDeviceKeys(new MemoryKeyStore())).keys);
  const aliceReg = await alice.register();
  const mallory = (await loadOrCreateDeviceKeys(new MemoryKeyStore())).keys;
  const url = `${kernelOrigin}/kernel/v1/devices/self`;
  const probe = await fetch(url, { headers: { DPoP: "x" } });
  const nonce = probe.headers.get("DPoP-Nonce") ?? undefined;
  const proof = await createDpopProof({ key: mallory.sign, method: "GET", url, nonce, accessToken: aliceReg.access_token });
  const res = await fetch(url, { headers: { Authorization: `DPoP ${aliceReg.access_token}`, DPoP: proof } });
  assert.equal(res.status, 401);
  assert.deepEqual(await res.json(), { error: "invalid_token", error_description: "access token is bound to a different key" });
});

test("client surfaces Kernel errors with RFC 9449 codes", async () => {
  const client = new KernelClient(kernelOrigin, (await loadOrCreateDeviceKeys(new MemoryKeyStore())).keys);
  await assert.rejects(client.self(), (e: unknown) => e instanceof KernelError && e.code === "not_registered");
});

const sha256hex = async (b: ArrayBuffer): Promise<string> =>
  Buffer.from(await crypto.subtle.digest("SHA-256", b)).toString("hex");

test("lease: wrapped chunk keys unwrap into decrypt-only keys that open Folio chunks", async () => {
  const { keys } = await loadOrCreateDeviceKeys(new MemoryKeyStore());
  const client = new KernelClient(kernelOrigin, keys);
  await client.register();
  const lease = await client.openEdition(CANARY);
  assert.deepEqual(lease.window, [0, 3]);
  assert.match(lease.rct, /^v4\.public\./u);
  const cks = await unwrapLease(lease, keys.ecdh.privateKey);
  assert.equal(cks.size, 3);
  for (const entry of manifest.filter((m) => m.chunk < 3)) {
    const key = cks.get(chunkKeyId(entry.chunk, 0));
    assert.ok(key);
    assert.equal(key.extractable, false);
    assert.deepEqual([...key.usages], ["decrypt"]);
    const sealed = new Uint8Array(readFileSync(join(sealedDir, entry.file)));
    const plain = await openChunk(key, sealed, { kind: "flow", editionId: CANARY, chunkIndex: entry.chunk, variant: 0 });
    assert.equal(await sha256hex(plain), entry.sha256, `chunk ${entry.chunk}`);

    // A flipped ciphertext bit fails authentication; a swapped chunk fails identity.
    const tampered = sealed.slice();
    tampered[100] = (tampered[100] ?? 0) ^ 1;
    await assert.rejects(openChunk(key, tampered, { kind: "flow", editionId: CANARY, chunkIndex: entry.chunk, variant: 0 }), FolioError);
    const other = new Uint8Array(readFileSync(join(sealedDir, `chunk-${(entry.chunk + 1) % 3}-v0.folio`)));
    await assert.rejects(openChunk(key, other, { kind: "flow", editionId: CANARY, chunkIndex: entry.chunk, variant: 0 }), /identity mismatch: chunkIndex/u);
  }
  assert.equal(cks.get(chunkKeyId(3, 0)), undefined, "no key outside the window");

  // Another device's ECDH key cannot unwrap this lease (AES-KW integrity check).
  const { keys: other } = await loadOrCreateDeviceKeys(new MemoryKeyStore());
  await assert.rejects(unwrapLease(lease, other.ecdh.privateKey));
});

test("browser on the reader origin: CORS, IndexedDB-persisted non-extractable keys", async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    const errors: string[] = [];
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(m.text());
    });
    await page.goto(readerOrigin);
    type Result = { created: boolean; exportBlocked: boolean; jkt: string; deviceId: string; selfId: string; selfJkt: string };
    const run = () => page.evaluate((o) => (globalThis as unknown as { Vault: { run: (o: string) => Promise<Result> } }).Vault.run(o), kernelOrigin);
    const first = await run();
    assert.equal(first.created, true);
    assert.equal(first.exportBlocked, true, "private key exportable in the browser");
    assert.equal(first.selfJkt, first.jkt);
    assert.equal(first.selfId, first.deviceId);

    await page.reload();
    const second = await run();
    assert.equal(second.created, false, "keys must survive a reload via IndexedDB");
    assert.equal(second.jkt, first.jkt, "same device key after reload");
    assert.equal(second.deviceId, first.deviceId, "re-registration is idempotent");
    // Chrome logs every 4xx as "Failed to load resource". The only expected
    // ones are the two use_dpop_nonce challenges (one per registration).
    assert.deepEqual(errors.filter((e) => /CORS|Access-Control|blocked/iu.test(e)), [], "CORS errors");
    assert.equal(errors.filter((e) => e.includes("status of 400")).length, 2, errors.join("\n"));
    assert.equal(errors.length, 2, errors.join("\n"));
  } finally {
    await browser.close();
  }
});

test("browser: lease unwrap and chunk decryption with WebCrypto, chunks from the reader origin", async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.goto(readerOrigin);
    type Result = { window: [number, number]; digests: Record<string, string>; keyProps: { extractable: boolean; usages: string[] }[]; rctPrefix: string };
    const r = (await page.evaluate(
      ([o, e]) => (globalThis as unknown as { Vault: { lease: (o: string, e: string) => Promise<unknown> } }).Vault.lease(o, e),
      [kernelOrigin, CANARY] as const,
    )) as Result;
    assert.deepEqual(r.window, [0, 3]);
    assert.equal(r.rctPrefix, "v4.public.");
    for (const p of r.keyProps) assert.deepEqual(p, { extractable: false, usages: ["decrypt"] });
    for (const m of manifest.filter((x) => x.chunk < 3)) assert.equal(r.digests[String(m.chunk)], m.sha256, `chunk ${m.chunk}`);
  } finally {
    await browser.close();
  }
});

test("passkeys: sign up on one device, sign in on another with the synced passkey", async () => {
  // Chromium's virtual authenticator: a real CTAP2 platform authenticator in
  // software, with resident keys and user verification. Each browser context
  // is a separate device (its own IndexedDB device keys).
  const browser = await chromium.launch();
  const authenticator = {
    protocol: "ctap2",
    transport: "internal",
    hasResidentKey: true,
    hasUserVerification: true,
    isUserVerified: true,
    automaticPresenceSimulation: true,
  } as const;
  type Result = { deviceId: string; userId: string; selfUser: string | null; credentialId: string };
  try {
    const ctxA = await browser.newContext();
    const pageA = await ctxA.newPage();
    await pageA.goto(readerOrigin);
    const cdpA = await ctxA.newCDPSession(pageA);
    await cdpA.send("WebAuthn.enable");
    const { authenticatorId: authA } = await cdpA.send("WebAuthn.addVirtualAuthenticator", { options: authenticator });
    const up = (await pageA.evaluate(
      (o) => (globalThis as unknown as { Vault: { passkey: (o: string, m: string) => Promise<unknown> } }).Vault.passkey(o, "up"),
      kernelOrigin,
    )) as Result;
    assert.equal(up.selfUser, up.userId, "device A is signed in to the new account");
    const { credentials } = await cdpA.send("WebAuthn.getCredentials", { authenticatorId: authA });
    assert.equal(credentials.length, 1);
    assert.equal(credentials[0]?.isResidentCredential, true, "a discoverable credential (passkey)");

    // Device B: another profile to which the passkey manager synced the credential.
    const ctxB = await browser.newContext();
    const pageB = await ctxB.newPage();
    await pageB.goto(readerOrigin);
    const cdpB = await ctxB.newCDPSession(pageB);
    await cdpB.send("WebAuthn.enable");
    const { authenticatorId: authB } = await cdpB.send("WebAuthn.addVirtualAuthenticator", { options: authenticator });
    const synced = credentials[0];
    assert.ok(synced);
    await cdpB.send("WebAuthn.addCredential", { authenticatorId: authB, credential: synced });
    const signedIn = (await pageB.evaluate(
      (o) => (globalThis as unknown as { Vault: { passkey: (o: string, m: string) => Promise<unknown> } }).Vault.passkey(o, "in"),
      kernelOrigin,
    )) as Result;
    assert.notEqual(signedIn.deviceId, up.deviceId, "a different device");
    assert.equal(signedIn.userId, up.userId, "the same account, without a username");
    assert.equal(signedIn.selfUser, up.userId);
    assert.equal(signedIn.credentialId, up.credentialId);
  } finally {
    await browser.close();
  }
});

test("an origin outside SANAD_READER_ORIGINS is blocked by CORS", async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    // Same server, different origin: 127.0.0.1 is not the allowed localhost origin.
    await page.goto(readerOrigin.replace("localhost", "127.0.0.1"));
    const outcome = await page.evaluate(async (o) => {
      try {
        await (globalThis as unknown as { Vault: { run: (o: string) => Promise<unknown> } }).Vault.run(o);
        return "allowed";
      } catch (e) {
        return String(e);
      }
    }, kernelOrigin);
    assert.match(outcome, /Failed to fetch/u, `foreign origin must be refused, got: ${outcome}`);
  } finally {
    await browser.close();
  }
});
