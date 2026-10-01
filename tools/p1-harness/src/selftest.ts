/**
 * Negative controls. A P1 harness that cannot see a leak is worse than none,
 * because it certifies violations as clean. Before every scan, CI proves that
 * each layer still detects a deliberately planted canary, and that a clean
 * page produces zero findings (no false positives).
 *
 * The leaky fixture site never contains a canary in plain form. Canaries are
 * shipped XOR-encoded and decoded at runtime, so each case exercises exactly
 * the layer it targets (e.g. the WASM case writes decoded bytes straight into
 * linear memory and never creates a JS string).
 */
import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import type { CanaryFile, NeedleSet } from "./canary.ts";
import { base64Alignments } from "./canary.ts";
import { Report, type Layer } from "./report.ts";
import { scanRoute, type ProcMemMode } from "./runtime-scan.ts";
import { serveStatic } from "./server.ts";
import { scanStatic } from "./static-scan.ts";

const XOR_KEY = 0xa5;

interface RuntimeCase {
  readonly mode: string;
  readonly canary: string;
  /** Every listed layer must report the planted canary. */
  readonly expect: readonly Layer[];
  /** If set, at least one finding's needle label must contain this (proves a specific encoding). */
  readonly expectNeedle?: string;
}

const RUNTIME_CASES: readonly RuntimeCase[] = [
  { mode: "clean", canary: "latin-en", expect: [] },
  { mode: "dom-text", canary: "arabic", expect: ["dom"] },
  { mode: "dom-attr", canary: "latin-fr", expect: ["dom"] },
  { mode: "shadow-closed", canary: "mixed-bidi", expect: ["dom"] },
  { mode: "aria", canary: "mixed-bidi", expect: ["aria"] },
  { mode: "console", canary: "latin-en", expect: ["console"] },
  { mode: "local-storage", canary: "arabic", expect: ["storage"] },
  { mode: "indexeddb", canary: "latin-fr", expect: ["storage"] },
  { mode: "indexeddb-bytes", canary: "arabic", expect: ["storage"] },
  { mode: "cache", canary: "latin-en", expect: ["storage"] },
  { mode: "network-response", canary: "arabic", expect: ["network"] },
  { mode: "network-request", canary: "latin-en", expect: ["network"] },
  { mode: "js-heap", canary: "latin-fr", expect: ["heap", "procmem"] },
  { mode: "js-heap-long", canary: "arabic", expect: ["procmem"] },
  { mode: "latin1-heap", canary: "latin-fr", expect: ["procmem"], expectNeedle: "latin1" },
  { mode: "worker-heap", canary: "mixed-bidi", expect: ["heap", "procmem"] },
  { mode: "wasm-memory", canary: "latin-en", expect: ["procmem"] },
  { mode: "arraybuffer-utf16", canary: "arabic", expect: ["procmem"], expectNeedle: "utf16le" },
];

// (module (memory (export "mem") 1))
const WASM_MEMORY_MODULE = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x07, 0x07, 0x01, 0x03, 0x6d, 0x65, 0x6d, 0x02, 0x00];

const LEAK_JS = `
const params = new URLSearchParams(location.search);
const mode = params.get("leak") ?? "clean";
const canary = params.get("canary") ?? "latin-en";
const KEY = ${XOR_KEY};
const keep = [];
async function bytes(kind) {
  const res = await fetch("/canary-" + canary + "." + kind + ".bin");
  const b = new Uint8Array(await res.arrayBuffer());
  for (let i = 0; i < b.length; i++) b[i] ^= KEY;
  return b;
}
async function text() { return new TextDecoder().decode(await bytes("u8")); }
function idb(value) {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open("p1-selftest", 1);
    open.onupgradeneeded = () => open.result.createObjectStore("s");
    open.onsuccess = () => {
      const tx = open.result.transaction("s", "readwrite");
      tx.objectStore("s").put(value, "k");
      tx.oncomplete = () => { open.result.close(); resolve(); };
      tx.onerror = () => reject(tx.error);
    };
    open.onerror = () => reject(open.error);
  });
}
const leaks = {
  "clean": async () => {},
  "dom-text": async () => { const d = document.createElement("p"); d.textContent = await text(); document.body.append(d); },
  "dom-attr": async () => { document.body.dataset.note = await text(); },
  "shadow-closed": async () => {
    const host = document.createElement("div"); document.body.append(host);
    host.attachShadow({ mode: "closed" }).textContent = await text();
  },
  "aria": async () => { const b = document.createElement("button"); b.setAttribute("aria-label", await text()); document.body.append(b); },
  "console": async () => { console.log(await text()); },
  "local-storage": async () => { localStorage.setItem("k", await text()); },
  "indexeddb": async () => { await idb({ note: await text() }); },
  "indexeddb-bytes": async () => { await idb(await bytes("u8")); },
  "cache": async () => { const c = await caches.open("p1"); await c.put("/cached", new Response(await text())); },
  "network-response": async () => { keep.push(await (await fetch("/leak-" + canary + ".txt")).arrayBuffer()); },
  "network-request": async () => { await fetch("/__echo", { method: "POST", body: await bytes("u8") }); },
  "js-heap": async () => { globalThis.__p1 = await text(); },
  "js-heap-long": async () => {
    const t = await text();
    globalThis.__p1 = JSON.parse(JSON.stringify("x".repeat(6000) + t + "y".repeat(6000)));
  },
  "latin1-heap": async () => {
    // Decode only the bytes before the U+2019 apostrophe (UTF-8 E2 80 99), so the
    // resulting string is Latin-1-representable and V8 stores it one byte per
    // char: neither UTF-8 nor UTF-16. The UTF-8 source bytes are wiped.
    const b = await bytes("u8");
    let cut = 0;
    while (cut + 2 < b.length && !(b[cut] === 0xe2 && b[cut + 1] === 0x80 && b[cut + 2] === 0x99)) cut++;
    globalThis.__p1 = JSON.parse(JSON.stringify(new TextDecoder().decode(b.subarray(0, cut - 2))));
    b.fill(0);
  },
  "worker-heap": async () => {
    const w = new Worker("/leak-worker.js", { type: "module" });
    const b = await bytes("u8");
    await new Promise((resolve) => { w.onmessage = resolve; w.postMessage(b, [b.buffer]); });
    keep.push(w);
  },
  "wasm-memory": async () => {
    const inst = new WebAssembly.Instance(new WebAssembly.Module(new Uint8Array(${JSON.stringify(WASM_MEMORY_MODULE)})));
    const b = await bytes("u8");
    new Uint8Array(inst.exports.mem.buffer).set(b, 4096);
    b.fill(0);
    keep.push(inst);
  },
  "arraybuffer-utf16": async () => { keep.push(await bytes("u16")); },
};
globalThis.__keep = keep;
await leaks[mode]();
performance.mark("lumen:ready");
`;

const LEAK_WORKER_JS = `
self.onmessage = (e) => {
  self.__p1 = new TextDecoder().decode(e.data);
  self.postMessage("ok");
};
`;

const INDEX_HTML = `<!doctype html><html><head><meta charset="utf-8"><title>P1 selftest</title>
<script type="module" src="/leak.js"></script></head><body><main>clean fixture</main></body></html>`;

function utf16le(s: string): Uint8Array {
  const out = new Uint8Array(s.length * 2);
  for (let i = 0; i < s.length; i++) {
    out[i * 2] = s.charCodeAt(i) & 0xff;
    out[i * 2 + 1] = s.charCodeAt(i) >>> 8;
  }
  return out;
}

function xor(data: Uint8Array): Uint8Array {
  return data.map((b) => b ^ XOR_KEY);
}

async function buildLeakySite(dir: string, canaries: CanaryFile): Promise<void> {
  await rm(dir, { recursive: true, force: true });
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, "index.html"), INDEX_HTML);
  await writeFile(join(dir, "leak.js"), LEAK_JS);
  await writeFile(join(dir, "leak-worker.js"), LEAK_WORKER_JS);
  const enc = new TextEncoder();
  for (const p of canaries.phrases) {
    await writeFile(join(dir, `canary-${p.id}.u8.bin`), xor(enc.encode(p.text)));
    await writeFile(join(dir, `canary-${p.id}.u16.bin`), xor(utf16le(p.text)));
    await writeFile(join(dir, `leak-${p.id}.txt`), p.text); // only fetched by network-response
  }
}

function phrase(canaries: CanaryFile, id: string): string {
  const p = canaries.phrases.find((x) => x.id === id);
  if (p === undefined) throw new Error(`selftest: canary "${id}" missing from canary file`);
  return p.text;
}

async function staticSelftest(dir: string, canaries: CanaryFile, needles: NeedleSet): Promise<string[]> {
  await rm(dir, { recursive: true, force: true });
  await mkdir(dir, { recursive: true });
  const enc = new TextEncoder();
  const escaped = [...phrase(canaries, "arabic")]
    .map((c) => (c.charCodeAt(0) > 0x7e ? `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}` : c))
    .join("");
  const b64 = base64Alignments(enc.encode(phrase(canaries, "latin-en")))[1] ?? "";
  const entities = [...phrase(canaries, "mixed-bidi")].map((c) => `&#x${(c.codePointAt(0) ?? 0).toString(16)};`).join("");
  const words = phrase(canaries, "latin-en").split(" ");
  const files: ReadonlyArray<readonly [string, Uint8Array | string, boolean]> = [
    ["clean.js", "export const greeting = \"مرحبا بالقارئ\";\n", false],
    ["escaped.js", `export const s = "${escaped}";\n`, true],
    ["embedded-b64.js", `const blob = "QUJD${b64}WFla";\n`, true],
    ["data.wasm", new Uint8Array([0, 0x61, 0x73, 0x6d, ...utf16le(phrase(canaries, "latin-fr").normalize("NFD"))]), true],
    ["chunk.js.map", JSON.stringify({ sourcesContent: [`// ${words.slice(2, 7).join(" ")}`] }), true],
    ["index.html.gz", gzipSync(Buffer.from(`<p>${entities}</p>`)), true],
  ];
  for (const [name, body] of files) await writeFile(join(dir, name), body);
  const report = new Report();
  await scanStatic([dir], needles, report);
  const failures: string[] = [];
  const hitFiles = new Set(report.findings.map((f) => f.location.replace(" (decompressed)", "")));
  for (const [name, , leaky] of files) {
    if (leaky && !hitFiles.has(name)) failures.push(`static: planted canary in ${name} was NOT detected`);
    if (!leaky && hitFiles.has(name)) failures.push(`static: false positive in clean file ${name}`);
  }
  return failures;
}

export async function runSelftest(
  canaries: CanaryFile,
  needles: NeedleSet,
  outDir: string,
  procMem: ProcMemMode,
): Promise<number> {
  const failures = await staticSelftest(join(outDir, "selftest-static"), canaries, needles);
  process.stdout.write(`static selftest: ${failures.length === 0 ? "ok" : "FAILED"}\n`);

  const siteDir = join(outDir, "selftest-site");
  await buildLeakySite(siteDir, canaries);
  const origin = await serveStatic({
    root: siteDir,
    headers: { "Cross-Origin-Opener-Policy": "same-origin", "Cross-Origin-Embedder-Policy": "require-corp" },
    echo: true,
  });
  const procMemActive = procMem !== "off";
  try {
    for (const c of RUNTIME_CASES) {
      const report = new Report();
      const route = `/?leak=${c.mode}&canary=${c.canary}`;
      await scanRoute(origin.url, route, needles, {
        readyMark: "lumen:ready",
        readyTimeoutMs: 15000,
        settleMs: 300,
        procMem,
        outDir: join(outDir, "selftest-har"),
        requireIsolation: true,
        enforceCsp: false,
      }, report);

      const hit = report.layersHit();
      const wrongCanary = report.findings.filter((f) => f.canaryId !== c.canary);
      const missing = c.expect.filter((l) => !(l === "procmem" && !procMemActive) && !hit.has(l));
      const problems: string[] = [];
      if (report.integrity.length > 0) problems.push(`integrity: ${report.integrity.map((i) => i.message).join("; ")}`);
      const errors = report.coverage.filter((cv) => cv.status === "error");
      if (errors.length > 0) problems.push(`layer errors: ${errors.map((e) => `${e.layer} ${e.detail}`).join("; ")}`);
      if (c.expect.length === 0 && report.findings.length > 0) {
        problems.push(`false positive(s) on clean page: ${[...hit].join(", ")}`);
      }
      if (missing.length > 0) problems.push(`undetected in layer(s): ${missing.join(", ")}`);
      if (wrongCanary.length > 0) problems.push(`attributed to wrong canary: ${wrongCanary[0]?.canaryId}`);
      if (c.expectNeedle !== undefined && procMemActive && !report.findings.some((f) => f.needle.includes(c.expectNeedle ?? ""))) {
        problems.push(`no finding via a "${c.expectNeedle}" needle`);
      }

      const extra = [...hit].filter((l) => !c.expect.includes(l));
      const status = problems.length === 0 ? "ok  " : "FAIL";
      process.stdout.write(
        `runtime selftest ${status} ${c.mode.padEnd(18)} expect [${c.expect.join(",")}]` +
          ` got [${[...hit].join(",")}]${extra.length > 0 ? " (extra layers fine)" : ""}\n`,
      );
      for (const p of problems) failures.push(`${c.mode}: ${p}`);
    }
  } finally {
    await origin.close();
  }

  if (failures.length > 0) {
    process.stdout.write(`\n✗ P1 harness selftest FAILED (${failures.length}):\n${failures.map((f) => `  - ${f}`).join("\n")}\n`);
    return 2;
  }
  process.stdout.write(`\n✓ P1 harness selftest passed: every layer detects planted canaries; clean page has zero findings.\n`);
  return 0;
}
