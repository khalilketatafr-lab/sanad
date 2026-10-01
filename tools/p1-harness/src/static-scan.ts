/**
 * Layer "static": every file that would be deployed to the reader origin.
 *
 * Each file is checked twice:
 *  1. raw bytes against byte needles (UTF-8, UTF-16LE, base64 at 3 alignments),
 *     which covers binaries (WASM data segments, fonts, images) too;
 *  2. if it decodes as text: JS/JSON/HTML/percent escapes are undone, the result
 *     is normalized and matched against text needles. Bundlers commonly emit
 *     non-ASCII as `\uXXXX`, which a naive grep would miss.
 * Pre-compressed siblings (.gz, .br) are decompressed and scanned as well.
 */
import { readdir, readFile, stat } from "node:fs/promises";
import { join, relative } from "node:path";
import { brotliDecompressSync, gunzipSync } from "node:zlib";
import type { NeedleSet } from "./canary.ts";
import { ByteMatcher, decodeEscapes, excerptBytes, matchText } from "./match.ts";
import type { Report } from "./report.ts";

const MAX_FILE_BYTES = 256 * 1024 * 1024;

async function* walk(dir: string): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true });
  for (const e of entries) {
    const p = join(dir, e.name);
    if (e.isDirectory()) yield* walk(p);
    else if (e.isFile()) yield p;
    // Symlinks are deliberately not followed: deploy artifacts must be real files.
  }
}

function decodeUtf8Strict(buf: Uint8Array): string | undefined {
  if (buf.includes(0)) return undefined;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(buf);
  } catch {
    return undefined;
  }
}

function inflate(path: string, buf: Uint8Array): Uint8Array | undefined {
  try {
    if (path.endsWith(".gz")) return gunzipSync(buf);
    if (path.endsWith(".br")) return brotliDecompressSync(buf);
  } catch {
    return undefined;
  }
  return undefined;
}

export async function scanStatic(
  roots: readonly string[],
  needles: NeedleSet,
  report: Report,
): Promise<void> {
  const matcher = new ByteMatcher(needles.assetBytes);
  for (const root of roots) {
    let files = 0;
    let bytes = 0;
    try {
      const s = await stat(root);
      if (!s.isDirectory()) throw new Error("not a directory");
    } catch (err) {
      report.cover({ layer: "static", scope: root, status: "error", detail: `cannot read: ${String(err)}` });
      continue;
    }
    for await (const path of walk(root)) {
      const rel = relative(root, path);
      const size = (await stat(path)).size;
      if (size > MAX_FILE_BYTES) {
        report.fail(`static:${rel}`, `file exceeds ${MAX_FILE_BYTES} bytes; refusing to ship unscanned content`);
        continue;
      }
      const raw = new Uint8Array(await readFile(path));
      const views: Array<readonly [string, Uint8Array]> = [[rel, raw]];
      const inflated = inflate(path, raw);
      if (inflated !== undefined) views.push([`${rel} (decompressed)`, inflated]);

      for (const [location, buf] of views) {
        files++;
        bytes += buf.length;
        for (const hit of matcher.scan(buf)) {
          report.add({
            layer: "static",
            canaryId: hit.needle.canaryId,
            needle: `${hit.needle.kind}:${hit.needle.label}`,
            location,
            offset: hit.offset,
            excerpt: excerptBytes(buf, hit.offset, hit.needle.bytes.length, hit.needle.encoding),
          });
        }
        const text = decodeUtf8Strict(buf);
        if (text !== undefined) {
          for (const hit of matchText(decodeEscapes(text), needles.text)) {
            report.add({
              layer: "static",
              canaryId: hit.needle.canaryId,
              needle: `${hit.needle.kind}:normalized`,
              location,
              excerpt: hit.excerpt,
            });
          }
        }
      }
    }
    if (files === 0) {
      report.cover({ layer: "static", scope: root, status: "error", detail: "no files: was the reader built?" });
    } else {
      report.cover({ layer: "static", scope: root, status: "scanned", detail: `${files} files, ${bytes} bytes` });
    }
  }
}
