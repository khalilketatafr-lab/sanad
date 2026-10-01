/**
 * End-to-end: the real `sanad-kernel` binary, driven by the real Vault
 * Worker code over HTTP, from Node (WebCrypto) and from Chromium on a
 * separate reader origin (CORS, IndexedDB-persisted non-extractable keys).
 */
import assert from "node:assert/strict";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";
import { MemoryKeyStore, loadOrCreateDeviceKeys } from "../src/vault/device-keys.ts";
import { createDpopProof } from "../src/vault/dpop.ts";
import { KernelClient, KernelError } from "../src/vault/kernel-client.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
let kernel: ChildProcess;
let kernelOrigin = "";
let reader: Server;
let readerOrigin = "";

async function freePort(): Promise<number> {
  const s = createServer();
  await new Promise<void>((r) => s.listen(0, "127.0.0.1", r));
  const { port } = s.address() as AddressInfo;
  await new Promise<void>((r) => s.close(() => r()));
  return port;
}

before(async () => {
  execFileSync("cargo", ["build", "-q", "-p", "sanad-kernel"], { cwd: repo, stdio: "inherit" });
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
    if (req.url === "/vault.js") res.writeHead(200, { "content-type": "text/javascript" }).end(js);
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
