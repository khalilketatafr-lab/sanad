/**
 * Generates the roadmap §10.4 reflow fixture with Atelier (Node only).
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ReflowFixture } from "./reflow-layout.ts";

export * from "./reflow-layout.ts";

/** Runs Atelier's example (no timing rounds) and reads the fixture. */
export function generateReflowFixture(repo: string): ReflowFixture {
  const out = mkdtempSync(join(tmpdir(), "lumen-reflow-"));
  execFileSync("cargo", ["run", "-q", "-p", "sanad-atelier", "--example", "reflow_bench", "--", out, "--rounds", "0"], { cwd: repo, stdio: ["ignore", "ignore", "inherit"] });
  return JSON.parse(readFileSync(join(out, "reflow.json"), "utf8")) as ReflowFixture;
}
