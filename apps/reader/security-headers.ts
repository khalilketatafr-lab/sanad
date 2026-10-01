/**
 * Security headers for the clean-room reader origin (read.sanad.app).
 *
 * Single source of truth: the same map is
 *   - served by `vite preview` (local production preview),
 *   - emitted as `dist/_headers` (Cloudflare Pages / edge config),
 *   - emitted as `dist/.security-headers.json`, which the P1 harness serves
 *     verbatim so CI tests exactly what production ships.
 *
 * Blueprint: docs/blueprint/05-security-and-drm.md §6.
 */

export interface ReaderOrigins {
  /** Kernel API origin, e.g. https://kernel.sanad.app. Omit for same-origin (local/CI). */
  readonly kernel?: string;
  /** Ciphertext CDN origin, e.g. https://cdn.sanad.app. Omit for same-origin (local/CI). */
  readonly cdn?: string;
}

const ORIGIN_RE = /^https:\/\/[a-z0-9.-]+(?::\d{1,5})?$/;

function assertOrigin(name: string, value: string | undefined): void {
  if (value !== undefined && !ORIGIN_RE.test(value)) {
    throw new Error(`security-headers: ${name} must be a bare https origin, got "${value}"`);
  }
}

export function contentSecurityPolicy(origins: ReaderOrigins = {}): string {
  assertOrigin("kernel", origins.kernel);
  assertOrigin("cdn", origins.cdn);
  const connect = ["'self'", origins.kernel, origins.cdn].filter((v): v is string => v !== undefined);
  const directives: ReadonlyArray<readonly [string, ...string[]]> = [
    ["default-src", "'none'"],
    ["script-src", "'self'", "'wasm-unsafe-eval'"],
    ["worker-src", "'self'"],
    ["connect-src", ...(connect as [string, ...string[]])],
    ["img-src", "'self'", "blob:", "data:"],
    ["style-src", "'self'"],
    ["font-src", "'self'"],
    ["manifest-src", "'self'"],
    ["base-uri", "'none'"],
    ["form-action", "'none'"],
    ["frame-ancestors", "'none'"],
    ["require-trusted-types-for", "'script'"],
    ["trusted-types", "'none'"],
  ];
  return directives.map((d) => d.join(" ")).join("; ");
}

export function securityHeaders(origins: ReaderOrigins = {}): Readonly<Record<string, string>> {
  return {
    "Content-Security-Policy": contentSecurityPolicy(origins),
    // Cross-origin isolation: unlocks SharedArrayBuffer (input ring) and WASM threads.
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "same-origin",
    "Permissions-Policy": [
      "accelerometer=()",
      "camera=()",
      "display-capture=()",
      "geolocation=()",
      "gyroscope=()",
      "hid=()",
      "magnetometer=()",
      "microphone=()",
      "midi=()",
      "payment=()",
      "serial=()",
      "usb=()",
      "xr-spatial-tracking=()",
    ].join(", "),
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    "Strict-Transport-Security": "max-age=63072000; includeSubDomains; preload",
  };
}

/** Headers safe for the Vite dev server (HMR needs inline scripts, so no CSP there). */
export function devHeaders(): Readonly<Record<string, string>> {
  return {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "same-origin",
  };
}
