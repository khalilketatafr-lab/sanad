/**
 * Builds every token output. `--check` rebuilds the committed Rust uniforms
 * into a temporary directory and fails if the committed file differs (CI).
 */
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import StyleDictionary from "style-dictionary";
import config, { createConfig } from "./sd-config.js";

const committedRust = fileURLToPath(new URL("../../crates/tokens/src/theme_uniforms.rs", import.meta.url));

if (process.argv.includes("--check")) {
  const dir = mkdtempSync(join(tmpdir(), "sanad-tokens-"));
  try {
    await new StyleDictionary(createConfig({ buildDir: join(dir, "build"), rustDir: dir })).buildAllPlatforms();
    if (readFileSync(join(dir, "theme_uniforms.rs"), "utf8") !== readFileSync(committedRust, "utf8")) {
      console.error(`stale: ${committedRust}\nrun \`pnpm --filter @sanad/tokens build\` and commit the result`);
      process.exitCode = 1;
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
} else {
  await new StyleDictionary(config).buildAllPlatforms();
}
