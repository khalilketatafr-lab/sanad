#!/usr/bin/env node
/**
 * Sanad P1 harness — "Protected text never exists as Unicode on the client."
 *
 *   p1-harness scan     [--config p1.config.json] [--dist DIR]... [--route PATH]...
 *                       [--proc-mem off|auto|require] [--out DIR] [--static-only]
 *   p1-harness selftest [--canaries FILE] [--proc-mem off|auto|require] [--out DIR]
 *
 * Exit codes: 0 clean · 1 P1 violation · 2 harness integrity failure.
 */
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { parseArgs } from "node:util";
import { buildNeedles, loadCanaries } from "./canary.ts";
import { Report } from "./report.ts";
import { scanRoute, type ProcMemMode } from "./runtime-scan.ts";
import { runSelftest } from "./selftest.ts";
import { serveStatic } from "./server.ts";
import { scanStatic } from "./static-scan.ts";

interface Config {
  readonly canaries: string;
  readonly dist: readonly string[];
  readonly routes: readonly string[];
  readonly readyMark: string;
  readonly readyTimeoutMs: number;
  readonly settleMs: number;
  readonly procMem: ProcMemMode;
  readonly out: string;
}

const DEFAULTS: Config = {
  canaries: "../../fixtures/canary/canaries.json",
  dist: ["../../apps/reader/dist"],
  routes: ["/"],
  readyMark: "lumen:ready",
  readyTimeoutMs: 20000,
  settleMs: 1500,
  procMem: "auto",
  out: "out",
};

function isProcMem(v: unknown): v is ProcMemMode {
  return v === "off" || v === "auto" || v === "require";
}

async function loadConfig(path: string | undefined): Promise<{ config: Config; base: string }> {
  if (path === undefined) return { config: DEFAULTS, base: process.cwd() };
  const raw = JSON.parse(await readFile(path, "utf8")) as Partial<Config>;
  const config: Config = { ...DEFAULTS, ...raw };
  if (!isProcMem(config.procMem)) throw new Error(`config.procMem must be off|auto|require`);
  return { config, base: dirname(resolve(path)) };
}

async function loadHeaders(dist: string): Promise<Record<string, string>> {
  const path = resolve(dist, ".security-headers.json");
  try {
    return JSON.parse(await readFile(path, "utf8")) as Record<string, string>;
  } catch {
    throw new Error(`${path} missing: the reader build must emit its production security headers`);
  }
}

async function main(): Promise<number> {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      config: { type: "string" },
      canaries: { type: "string" },
      dist: { type: "string", multiple: true },
      route: { type: "string", multiple: true },
      "proc-mem": { type: "string" },
      out: { type: "string" },
      "static-only": { type: "boolean", default: false },
    },
  });
  const command = positionals[0] ?? "scan";
  const { config, base } = await loadConfig(values.config);
  const procMem = values["proc-mem"] ?? config.procMem;
  if (!isProcMem(procMem)) throw new Error("--proc-mem must be off|auto|require");
  const out = resolve(values.out ?? resolve(base, config.out));
  const canaryPath = resolve(values.canaries ?? resolve(base, config.canaries));
  const canaries = await loadCanaries(canaryPath);
  const needles = buildNeedles(canaries);
  process.stdout.write(
    `P1 harness: ${canaries.phrases.length} canaries → ${needles.text.length} text needles, ` +
      `${needles.bytes.length} memory needles, ${needles.assetBytes.length} asset needles\n`,
  );

  if (command === "selftest") return runSelftest(canaries, needles, out, procMem);
  if (command !== "scan") throw new Error(`unknown command "${command}"`);

  const dists = (values.dist ?? config.dist).map((d) => resolve(values.dist ? process.cwd() : base, d));
  const routes = values.route ?? config.routes;
  const report = new Report();

  await scanStatic(dists, needles, report);

  if (!values["static-only"]) {
    const primary = dists[0];
    if (primary === undefined) throw new Error("no dist directory to serve");
    const origin = await serveStatic({ root: primary, headers: await loadHeaders(primary) });
    try {
      for (const route of routes) {
        await scanRoute(origin.url, route, needles, {
          readyMark: config.readyMark,
          readyTimeoutMs: config.readyTimeoutMs,
          settleMs: config.settleMs,
          procMem,
          outDir: out,
          requireIsolation: true,
          enforceCsp: true,
        }, report);
      }
    } finally {
      await origin.close();
    }
  }

  report.print("P1 enforcement");
  const path = await report.write(out, "p1-report");
  process.stdout.write(`\nReport: ${path}\n`);
  return report.exitCode();
}

main().then(
  (code) => process.exit(code),
  (err: unknown) => {
    process.stderr.write(`p1-harness: ${err instanceof Error ? (err.stack ?? err.message) : String(err)}\n`);
    process.exit(2);
  },
);
