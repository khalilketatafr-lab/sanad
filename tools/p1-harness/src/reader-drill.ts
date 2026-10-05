/**
 * P1 reader drill — a real, adversarial protection test against the running
 * reader (not a unit test). The reader-demo renders a real book (Austen +
 * Kalila wa-Dimna) fully on screen; this drill proves that exact text exists
 * NOWHERE a client-side attacker can read it — DOM (incl. pierced shadow roots
 * and the accessibility tree), storage, cookies, the served bytes, and the JS
 * heap — using the same scanner CI runs. Base64 and UTF-16 alignments are
 * checked too, so inlined ciphertext cannot hide plaintext.
 *
 *   node src/reader-drill.ts        # builds reader-demo if needed, then scans
 *
 * Two checks, both must pass:
 *   1. the untouched reader is clean (the on-screen book text is absent); and
 *   2. a negative control — one phrase planted in the DOM — IS caught, so the
 *      clean result above is meaningful and not a blind scanner.
 *
 * Exit 0 only if the reader is clean AND the planted leak is caught.
 */
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildNeedles, loadCanaries } from "./canary.ts";
import { Report } from "./report.ts";
import { scanRoute, type RuntimeOptions } from "./runtime-scan.ts";
import { serveStatic } from "./server.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const dist = resolve(repo, "tools/reader-demo/dist");
const page = "reader-demo.standalone.html";
const outDir = resolve(here, "../out");

function options(): RuntimeOptions {
  return {
    readyMark: "lumen:ready",
    readyTimeoutMs: 20_000,
    settleMs: 1500,
    // Process-memory scanning needs ptrace privileges unavailable in most
    // sandboxes; the DOM/storage/network/heap surfaces are the client-attacker
    // surfaces and are always scanned.
    procMem: "off",
    outDir,
    requireIsolation: false,
    enforceCsp: false,
  };
}

async function main(): Promise<void> {
  if (!existsSync(join(dist, page))) {
    console.log("building reader-demo (self-contained) …");
    execFileSync("node", ["build.ts"], { cwd: resolve(repo, "tools/reader-demo"), stdio: "inherit" });
  }

  const canaries = await loadCanaries(resolve(here, "../fixtures/reader-content.json"));
  const needles = buildNeedles(canaries);

  // 1. The real reader, untouched. Expect zero canaries.
  const origin = await serveStatic({ root: dist, headers: {} });
  const real = new Report();
  try {
    await scanRoute(origin.url, `/${page}`, needles, options(), real);
  } finally {
    await origin.close();
  }
  real.print("Real reader — the book is on screen; is any of its text on the client?");
  const readerClean = real.exitCode() === 0;

  // 2. Negative control: plant one phrase in the DOM; the scan MUST catch it.
  const tmp = await mkdtemp(join(tmpdir(), "p1-neg-"));
  const planted = canaries.phrases[0]?.text ?? "";
  const html = (await readFile(join(dist, page), "utf8")).replace(
    "</body>",
    `<div id="planted">${planted}</div></body>`,
  );
  await writeFile(join(tmp, "planted.html"), html);
  const negOrigin = await serveStatic({ root: tmp, headers: {} });
  const neg = new Report();
  try {
    await scanRoute(negOrigin.url, "/planted.html", needles, options(), neg);
  } finally {
    await negOrigin.close();
    await rm(tmp, { recursive: true, force: true });
  }
  neg.print("Negative control — one phrase planted in the DOM");
  const leakCaught = neg.exitCode() === 1;

  console.log(
    `\nRESULT: reader clean = ${readerClean ? "YES" : "NO"} · planted leak caught = ${leakCaught ? "YES" : "NO"}`,
  );
  process.exit(readerClean && leakCaught ? 0 : 1);
}

void main();
