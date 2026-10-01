import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export type Layer =
  | "static" // built assets on disk (apps/reader/dist)
  | "dom" // serialized DOM, innerText, open shadow roots, all frames
  | "aria" // accessibility tree as exposed to assistive technology
  | "storage" // localStorage, sessionStorage, IndexedDB, Cache Storage, OPFS, cookies
  | "network" // every request/response body, URL and header (HAR + live capture)
  | "console" // console output of the page and its workers
  | "heap" // V8 heap snapshot strings (main thread, and workers when attachable)
  | "procmem"; // raw memory of every browser process: catches ArrayBuffers, WASM memory, freed strings

export interface Finding {
  readonly layer: Layer;
  readonly canaryId: string;
  readonly needle: string;
  readonly location: string;
  readonly excerpt: string;
  readonly offset?: number;
}

export type CoverageStatus = "scanned" | "skipped" | "error";

export interface LayerCoverage {
  readonly layer: Layer;
  readonly scope: string;
  readonly status: CoverageStatus;
  readonly detail: string;
}

export interface IntegrityIssue {
  readonly scope: string;
  readonly message: string;
}

export class Report {
  readonly findings: Finding[] = [];
  readonly coverage: LayerCoverage[] = [];
  readonly integrity: IntegrityIssue[] = [];
  readonly #seen = new Set<string>();

  add(f: Finding): void {
    const key = `${f.layer}|${f.canaryId}|${f.needle}|${f.location}|${f.offset ?? ""}`;
    if (this.#seen.has(key)) return;
    this.#seen.add(key);
    this.findings.push(f);
  }

  cover(c: LayerCoverage): void {
    this.coverage.push(c);
  }

  fail(scope: string, message: string): void {
    this.integrity.push({ scope, message });
  }

  merge(other: Report): void {
    for (const f of other.findings) this.add(f);
    this.coverage.push(...other.coverage);
    this.integrity.push(...other.integrity);
  }

  layersHit(): Set<Layer> {
    return new Set(this.findings.map((f) => f.layer));
  }

  /** 0 = clean, 1 = P1 violation, 2 = harness integrity failure (cannot vouch for P1). */
  exitCode(): 0 | 1 | 2 {
    if (this.findings.length > 0) return 1;
    if (this.integrity.length > 0 || this.coverage.some((c) => c.status === "error")) return 2;
    return 0;
  }

  async write(outDir: string, name: string): Promise<string> {
    await mkdir(outDir, { recursive: true });
    const path = join(outDir, `${name}.json`);
    const body = {
      result: ["pass", "P1-violation", "integrity-failure"][this.exitCode()],
      findings: this.findings,
      integrity: this.integrity,
      coverage: this.coverage,
    };
    await writeFile(path, `${JSON.stringify(body, null, 2)}\n`);
    return path;
  }

  print(title: string): void {
    const out = process.stdout;
    out.write(`\n━━ ${title} ━━\n`);
    for (const c of this.coverage) {
      const mark = c.status === "scanned" ? "✓" : c.status === "skipped" ? "–" : "✗";
      out.write(`  ${mark} ${c.layer.padEnd(8)} ${c.scope}: ${c.detail}\n`);
    }
    for (const i of this.integrity) out.write(`  ✗ integrity ${i.scope}: ${i.message}\n`);
    if (this.findings.length === 0) {
      out.write("  No canary observed.\n");
      return;
    }
    out.write(`\n  ✗ P1 VIOLATION — ${this.findings.length} canary observation(s):\n`);
    for (const f of this.findings.slice(0, 50)) {
      const at = f.offset === undefined ? "" : ` @0x${f.offset.toString(16)}`;
      out.write(`    [${f.layer}] ${f.canaryId} (${f.needle}) in ${f.location}${at}\n      … ${f.excerpt} …\n`);
    }
    if (this.findings.length > 50) out.write(`    … and ${this.findings.length - 50} more (see JSON report)\n`);
  }
}
