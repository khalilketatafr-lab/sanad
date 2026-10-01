/**
 * Minimal static origin used by the runtime scan. It serves a built dist with
 * the exact production security headers (dist/.security-headers.json) so the
 * reader runs under its real CSP and cross-origin isolation.
 */
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { extname, join, normalize, sep } from "node:path";

const MIME: Readonly<Record<string, string>> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".avif": "image/avif",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
  ".bin": "application/octet-stream",
  ".folio": "application/octet-stream",
  ".webmanifest": "application/manifest+json",
};

export interface StaticOrigin {
  readonly url: string;
  close(): Promise<void>;
}

export interface ServeOptions {
  readonly root: string;
  readonly headers: Readonly<Record<string, string>>;
  /** Selftest only: accept `POST /__echo` (204) so request-body leaks can be generated. */
  readonly echo?: boolean;
}

export async function serveStatic(opts: ServeOptions): Promise<StaticOrigin> {
  const root = normalize(opts.root);

  async function resolveFile(urlPath: string): Promise<string | undefined> {
    let decoded: string;
    try {
      decoded = decodeURIComponent(urlPath);
    } catch {
      return undefined;
    }
    const candidate = normalize(join(root, decoded));
    if (candidate !== root && !candidate.startsWith(root + sep)) return undefined; // traversal
    try {
      const s = await stat(candidate);
      if (s.isFile()) return candidate;
      if (s.isDirectory()) {
        const index = join(candidate, "index.html");
        if ((await stat(index)).isFile()) return index;
      }
    } catch {
      // fall through to SPA fallback
    }
    // SPA fallback for extension-less routes (/read/:edition).
    if (extname(decoded) === "") return join(root, "index.html");
    return undefined;
  }

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    for (const [k, v] of Object.entries(opts.headers)) res.setHeader(k, v);
    res.setHeader("Cache-Control", "no-store");
    const url = new URL(req.url ?? "/", "http://localhost");

    if (opts.echo === true && req.method === "POST" && url.pathname === "/__echo") {
      req.resume();
      req.on("end", () => {
        res.writeHead(204).end();
      });
      return;
    }
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.writeHead(405).end();
      return;
    }
    const file = await resolveFile(url.pathname);
    if (file === undefined) {
      res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" }).end("not found");
      return;
    }
    res.setHeader("Content-Type", MIME[extname(file)] ?? "application/octet-stream");
    res.writeHead(200);
    if (req.method === "HEAD") {
      res.end();
      return;
    }
    createReadStream(file).pipe(res);
  }

  const server: Server = createServer((req, res) => {
    handle(req, res).catch((err: unknown) => {
      res.writeHead(500).end(String(err));
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    // "localhost" (not 127.0.0.1) is a potentially-trustworthy origin, so
    // crossOriginIsolated, WebCrypto and service workers behave as on https.
    url: `http://localhost:${port}`,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}
