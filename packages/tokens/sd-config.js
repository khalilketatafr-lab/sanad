// @ts-check
/**
 * Style Dictionary 5 configuration for @sanad/tokens.
 *
 *   tokens.json (W3C DTCG 2025.10)
 *     ├─ css   → build/css/themes.css                  reader chrome (custom properties)
 *     ├─ ts    → build/ts/tokens.ts                    app logic + Lumen uniforms
 *     └─ rust  → crates/tokens/src/theme_uniforms.rs   shader uniforms for Rust (committed)
 *
 * Transforms normalize and VALIDATE each token (an OKLCH value whose hex is
 * not its exact 8-bit rounding fails the build); formats print the validated
 * model from src/model.ts. Warnings are errors.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { formatCss, formatRust, formatTs } from "./src/formats.ts";
import { buildModel, readMeta, resolveColor, resolveDimensionPx, resolveShadow } from "./src/model.ts";

/** @typedef {import("style-dictionary/types").Config} Config */
/** @typedef {import("style-dictionary/types").TransformedToken} TransformedToken */
/** @typedef {import("style-dictionary/types").FormatFnArguments} FormatFnArguments */
/** @typedef {import("./src/model.ts").TokenMeta} TokenMeta */

const here = (/** @type {string} */ rel) => fileURLToPath(new URL(rel, import.meta.url));

export const TRANSFORM_GROUP = "sanad/themes";

/** @param {TransformedToken} t */
const where = (t) => t.path.join(".");

/**
 * @param {FormatFnArguments} args
 * @returns {{ models: import("./src/model.ts").ThemeModel[], meta: TokenMeta }}
 */
function model({ dictionary, options }) {
  const meta = /** @type {TokenMeta} */ (options["meta"]);
  return { models: buildModel(dictionary.allTokens, meta), meta };
}

/**
 * @param {{ source?: string, tokens?: Record<string, unknown>, buildDir?: string, rustDir?: string }} [opts]
 *   Overrides for tests: an alternative token document and output directories.
 * @returns {Config}
 */
export function createConfig(opts = {}) {
  const sourcePath = opts.source ?? here("./tokens.json");
  const document = opts.tokens ?? JSON.parse(readFileSync(sourcePath, "utf8"));
  const meta = readMeta(document);
  const buildDir = (opts.buildDir ?? here("./build")).replace(/\/?$/u, "/");
  const rustDir = (opts.rustDir ?? here("../../crates/tokens/src")).replace(/\/?$/u, "/");

  return {
    ...(opts.tokens === undefined ? { source: [sourcePath] } : { tokens: document }),
    usesDtcg: true,
    // Verbose so a failed transform reports its token and cause, not just a count.
    log: { warnings: "error", verbosity: "verbose" },
    hooks: {
      transforms: {
        // Full path: theme groups reuse leaf names, so leaf names would collide.
        "sanad/name/path": { type: "name", transform: (t) => where(t) },
        "sanad/color/resolve": {
          type: "value",
          filter: (t) => t.$type === "color",
          transform: (t) => resolveColor(t.$value, where(t)),
        },
        "sanad/shadow/resolve": {
          type: "value",
          filter: (t) => t.$type === "shadow",
          transform: (t) => resolveShadow(t.$value, where(t)),
        },
        "sanad/dimension/px": {
          type: "value",
          filter: (t) => t.$type === "dimension",
          transform: (t) => resolveDimensionPx(t.$value, where(t)),
        },
      },
      transformGroups: {
        [TRANSFORM_GROUP]: ["sanad/name/path", "sanad/color/resolve", "sanad/shadow/resolve", "sanad/dimension/px"],
      },
      formats: {
        "sanad/css-themes": (args) => {
          const { models, meta: m } = model(args);
          return formatCss(models, m);
        },
        "sanad/ts-themes": (args) => {
          const { models, meta: m } = model(args);
          return formatTs(models, m);
        },
        "sanad/rust-uniforms": (args) => formatRust(model(args).models),
      },
    },
    platforms: {
      css: {
        transformGroup: TRANSFORM_GROUP,
        buildPath: `${buildDir}css/`,
        files: [{ destination: "themes.css", format: "sanad/css-themes", options: { meta } }],
      },
      ts: {
        transformGroup: TRANSFORM_GROUP,
        buildPath: `${buildDir}ts/`,
        files: [{ destination: "tokens.ts", format: "sanad/ts-themes", options: { meta } }],
      },
      rust: {
        transformGroup: TRANSFORM_GROUP,
        buildPath: rustDir,
        files: [{ destination: "theme_uniforms.rs", format: "sanad/rust-uniforms", options: { meta } }],
      },
    },
  };
}

export default createConfig();
