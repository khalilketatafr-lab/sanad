import { defineConfig, type Plugin, type UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import { devHeaders, securityHeaders, type ReaderOrigins } from "./security-headers.ts";

function originsFromEnv(): ReaderOrigins {
  const kernel = process.env["SANAD_KERNEL_ORIGIN"];
  const cdn = process.env["SANAD_CDN_ORIGIN"];
  return { ...(kernel ? { kernel } : {}), ...(cdn ? { cdn } : {}) };
}

/** Emits `_headers` (edge) and `.security-headers.json` (P1 harness) into dist. */
function emitSecurityHeaders(origins: ReaderOrigins): Plugin {
  return {
    name: "sanad:security-headers",
    apply: "build",
    generateBundle() {
      const headers = securityHeaders(origins);
      const edge = ["/*", ...Object.entries(headers).map(([k, v]) => `  ${k}: ${v}`)].join("\n");
      this.emitFile({ type: "asset", fileName: "_headers", source: `${edge}\n` });
      this.emitFile({
        type: "asset",
        fileName: ".security-headers.json",
        source: `${JSON.stringify(headers, null, 2)}\n`,
      });
    },
  };
}

export default defineConfig((): UserConfig => {
  const origins = originsFromEnv();
  return {
    plugins: [react(), emitSecurityHeaders(origins)],
    build: {
      target: "es2023",
      // No source maps in shipped output: they would re-expose sources and are
      // scanned by the P1 harness anyway. Upload hidden maps to error tracking later.
      sourcemap: false,
      // Never inline assets as data: URLs (WASM via data: would violate connect-src).
      assetsInlineLimit: 0,
      modulePreload: { polyfill: false },
      reportCompressedSize: false,
    },
    worker: { format: "es" },
    server: { headers: devHeaders() },
    preview: { headers: securityHeaders(origins) },
  };
});
