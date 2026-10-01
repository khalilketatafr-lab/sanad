/**
 * Runtime layers: drive the real reader in Chromium, under its production
 * headers, and look for canaries everywhere a client can hold data.
 *
 * Order matters: process memory is swept first (before anything forces a GC),
 * then semantic layers, then heap snapshots (which GC), then the HAR is
 * flushed on context close and scanned.
 */
import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { chromium, type BrowserContext, type Page } from "playwright";
import type { NeedleSet } from "./canary.ts";
import { scanHeaps } from "./heap.ts";
import { ByteMatcher, decodeEscapes, excerptBytes, matchText } from "./match.ts";
import { scanProcessMemory } from "./procmem.ts";
import type { Layer, Report } from "./report.ts";

export type ProcMemMode = "off" | "auto" | "require";

export interface RuntimeOptions {
  readonly readyMark: string;
  readonly readyTimeoutMs: number;
  readonly settleMs: number;
  readonly procMem: ProcMemMode;
  readonly outDir: string;
  /** Fail if the page is not cross-origin isolated (the reader must be). */
  readonly requireIsolation: boolean;
  /** Treat CSP / Trusted Types violations as integrity failures. */
  readonly enforceCsp: boolean;
}

interface StorageItem {
  readonly where: string;
  readonly text?: string;
  readonly b64?: string;
}

const CHROMIUM_ARGS = [
  // Deterministic software WebGL2 so Lumen initializes on GPU-less CI runners.
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
  "--disable-background-networking",
  "--disable-component-update",
  "--no-first-run",
];

export function scanTextInto(report: Report, layer: Layer, location: string, text: string, needles: NeedleSet): void {
  for (const hit of matchText(decodeEscapes(text), needles.text)) {
    report.add({ layer, canaryId: hit.needle.canaryId, needle: `${hit.needle.kind}:normalized`, location, excerpt: hit.excerpt });
  }
}

export function scanBytesInto(
  report: Report,
  layer: Layer,
  location: string,
  bytes: Uint8Array,
  matcher: ByteMatcher,
  needles: NeedleSet,
): void {
  for (const hit of matcher.scan(bytes)) {
    report.add({
      layer,
      canaryId: hit.needle.canaryId,
      needle: `${hit.needle.kind}:${hit.needle.label}`,
      location,
      offset: hit.offset,
      excerpt: excerptBytes(bytes, hit.offset, hit.needle.bytes.length, hit.needle.encoding),
    });
  }
  const text = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  scanTextInto(report, layer, location, text, needles);
}

/** Runs inside the page. Must be self-contained (serialized by Playwright). */
async function collectStorageInPage(): Promise<StorageItem[]> {
  const out: StorageItem[] = [];
  const toB64 = (u8: Uint8Array): string => {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const visit = async (v: unknown, where: string, depth: number): Promise<void> => {
    if (depth > 32) return;
    if (typeof v === "string") {
      out.push({ where, text: v });
    } else if (v instanceof ArrayBuffer) {
      out.push({ where, b64: toB64(new Uint8Array(v)) });
    } else if (ArrayBuffer.isView(v)) {
      out.push({ where, b64: toB64(new Uint8Array(v.buffer, v.byteOffset, v.byteLength)) });
    } else if (v instanceof Blob) {
      out.push({ where, b64: toB64(new Uint8Array(await v.arrayBuffer())) });
    } else if (v instanceof CryptoKey) {
      out.push({ where, text: `[CryptoKey ${v.type}]` });
    } else if (v instanceof Map) {
      for (const [k, val] of v) {
        await visit(k, `${where}<key>`, depth + 1);
        await visit(val, `${where}[${String(k)}]`, depth + 1);
      }
    } else if (v instanceof Set || Array.isArray(v)) {
      let i = 0;
      for (const val of v) await visit(val, `${where}[${i++}]`, depth + 1);
    } else if (v !== null && typeof v === "object") {
      for (const [k, val] of Object.entries(v)) {
        await visit(k, `${where}<key>`, depth + 1);
        await visit(val, `${where}.${k}`, depth + 1);
      }
    }
  };
  const request = <T,>(r: IDBRequest<T>): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error ?? new Error("IDB request failed"));
    });

  for (const [name, store] of [
    ["localStorage", localStorage],
    ["sessionStorage", sessionStorage],
  ] as const) {
    for (let i = 0; i < store.length; i++) {
      const key = store.key(i);
      if (key === null) continue;
      out.push({ where: `${name}<key>`, text: key });
      out.push({ where: `${name}.${key}`, text: store.getItem(key) ?? "" });
    }
  }

  for (const info of await indexedDB.databases()) {
    const name = info.name;
    if (name === undefined) continue;
    const db = await request(indexedDB.open(name));
    try {
      for (const storeName of Array.from(db.objectStoreNames)) {
        const store = db.transaction(storeName, "readonly").objectStore(storeName);
        const [keys, values] = await Promise.all([request(store.getAllKeys()), request(store.getAll())]);
        await visit(keys, `indexedDB:${name}/${storeName}<keys>`, 0);
        await visit(values, `indexedDB:${name}/${storeName}`, 0);
      }
    } finally {
      db.close();
    }
  }

  for (const cacheName of await caches.keys()) {
    const cache = await caches.open(cacheName);
    for (const req of await cache.keys()) {
      out.push({ where: `cache:${cacheName}<url>`, text: req.url });
      const res = await cache.match(req);
      if (res) out.push({ where: `cache:${cacheName} ${req.url}`, b64: toB64(new Uint8Array(await res.arrayBuffer())) });
    }
  }

  try {
    const walk = async (dir: FileSystemDirectoryHandle, path: string): Promise<void> => {
      for await (const [entryName, handle] of dir.entries()) {
        if (handle.kind === "file") {
          const file = await (handle as FileSystemFileHandle).getFile();
          out.push({ where: `opfs:${path}/${entryName}`, b64: toB64(new Uint8Array(await file.arrayBuffer())) });
        } else {
          await walk(handle as FileSystemDirectoryHandle, `${path}/${entryName}`);
        }
      }
    };
    await walk(await navigator.storage.getDirectory(), "");
  } catch {
    // OPFS unavailable in this context
  }
  return out;
}

/** Runs inside each frame: DOM, rendered text and open shadow roots. */
function serializeDomInPage(): string {
  const parts: string[] = [document.title, document.documentElement.outerHTML, document.body.innerText];
  const stack: Array<Document | ShadowRoot> = [document];
  while (stack.length > 0) {
    const root = stack.pop();
    if (root === undefined) break;
    for (const el of Array.from(root.querySelectorAll("*"))) {
      if (el.shadowRoot !== null) {
        parts.push(el.shadowRoot.innerHTML);
        stack.push(el.shadowRoot);
      }
    }
  }
  return parts.join("\n");
}

interface CdpDomNode {
  readonly nodeValue?: string;
  readonly attributes?: readonly string[];
  readonly children?: readonly CdpDomNode[];
  readonly shadowRoots?: readonly CdpDomNode[];
  readonly contentDocument?: CdpDomNode;
  readonly pseudoElements?: readonly CdpDomNode[];
}

function flattenCdpDom(node: CdpDomNode, out: string[]): void {
  if (node.nodeValue) out.push(node.nodeValue);
  if (node.attributes) out.push(...node.attributes);
  for (const list of [node.children, node.shadowRoots, node.pseudoElements]) for (const c of list ?? []) flattenCdpDom(c, out);
  if (node.contentDocument) flattenCdpDom(node.contentDocument, out);
}

async function scanDom(context: BrowserContext, page: Page, scope: string, needles: NeedleSet, report: Report): Promise<void> {
  let frames = 0;
  for (const frame of page.frames()) {
    frames++;
    scanTextInto(report, "dom", `${scope} frame ${frame.url()}`, await frame.evaluate(serializeDomInPage), needles);
  }
  // CDP pierce walk: reaches closed shadow roots and cross-frame documents.
  const cdp = await context.newCDPSession(page);
  try {
    const { root } = (await cdp.send("DOM.getDocument", { depth: -1, pierce: true })) as { root: CdpDomNode };
    const values: string[] = [];
    flattenCdpDom(root, values);
    scanTextInto(report, "dom", `${scope} CDP DOM (pierce)`, values.join("\n"), needles);

    const ax = (await cdp.send("Accessibility.getFullAXTree")) as {
      nodes: ReadonlyArray<{ name?: { value?: unknown }; description?: { value?: unknown }; value?: { value?: unknown } }>;
    };
    const axText = ax.nodes
      .flatMap((n) => [n.name?.value, n.description?.value, n.value?.value])
      .filter((v): v is string => typeof v === "string")
      .join("\n");
    scanTextInto(report, "aria", `${scope} accessibility tree`, axText, needles);
    scanTextInto(report, "aria", `${scope} aria snapshot`, await page.locator(":root").ariaSnapshot(), needles);
    report.cover({ layer: "aria", scope, status: "scanned", detail: `${ax.nodes.length} AX nodes` });
  } finally {
    await cdp.detach().catch(() => undefined);
  }
  report.cover({ layer: "dom", scope, status: "scanned", detail: `${frames} frame(s) + pierced CDP DOM` });
}

async function scanStorage(context: BrowserContext, page: Page, scope: string, needles: NeedleSet, matcher: ByteMatcher, report: Report): Promise<void> {
  const items = await page.evaluate(collectStorageInPage);
  for (const item of items) {
    const location = `${scope} ${item.where}`;
    if (item.text !== undefined) scanTextInto(report, "storage", location, item.text, needles);
    if (item.b64 !== undefined) scanBytesInto(report, "storage", location, Buffer.from(item.b64, "base64"), matcher, needles);
  }
  const cookies = await context.cookies();
  for (const c of cookies) scanTextInto(report, "storage", `${scope} cookie ${c.name}`, `${c.name}=${c.value}`, needles);
  report.cover({ layer: "storage", scope, status: "scanned", detail: `${items.length} items, ${cookies.length} cookies` });
}

interface HarEntry {
  readonly request: { url: string; headers: ReadonlyArray<{ name: string; value: string }>; postData?: { text?: string } };
  readonly response: {
    headers: ReadonlyArray<{ name: string; value: string }>;
    content?: { text?: string; encoding?: string };
  };
}

async function scanHar(path: string, scope: string, needles: NeedleSet, matcher: ByteMatcher, report: Report): Promise<number> {
  const har = JSON.parse(await readFile(path, "utf8")) as { log: { entries: readonly HarEntry[] } };
  for (const e of har.log.entries) {
    const loc = `${scope} HAR ${e.request.url}`;
    const headerText = [...e.request.headers, ...e.response.headers].map((h) => `${h.name}: ${h.value}`).join("\n");
    scanTextInto(report, "network", loc, `${e.request.url}\n${headerText}`, needles);
    if (e.request.postData?.text !== undefined) scanTextInto(report, "network", `${loc} (request body)`, e.request.postData.text, needles);
    const content = e.response.content;
    if (content?.text !== undefined) {
      const body = content.encoding === "base64" ? Buffer.from(content.text, "base64") : Buffer.from(content.text, "utf8");
      scanBytesInto(report, "network", `${loc} (response body)`, body, matcher, needles);
    }
  }
  return har.log.entries.length;
}

export async function scanRoute(
  originUrl: string,
  route: string,
  needles: NeedleSet,
  opts: RuntimeOptions,
  report: Report,
): Promise<void> {
  const scope = route;
  const assetMatcher = new ByteMatcher(needles.assetBytes);
  await mkdir(opts.outDir, { recursive: true });
  const harPath = join(opts.outDir, `${route.replace(/[^a-z0-9]+/giu, "_") || "root"}.har`);

  const browser = await chromium.launch({ headless: true, args: CHROMIUM_ARGS });
  try {
    const context = await browser.newContext({
      recordHar: { path: harPath, content: "embed", mode: "full" },
      serviceWorkers: "allow",
      bypassCSP: false,
    });
    const page = await context.newPage();
    const consoleLines: string[] = [];
    const pending: Array<Promise<void>> = [];
    let liveResponses = 0;

    page.on("console", (msg) => {
      const text = msg.text();
      consoleLines.push(text);
      if (opts.enforceCsp && msg.type() === "error" && /Content Security Policy|Trusted Type/iu.test(text)) {
        report.fail(scope, `CSP/Trusted Types violation: ${text}`);
      }
    });
    page.on("pageerror", (err) => report.fail(scope, `uncaught page error: ${err.message}`));
    context.on("response", (res) => {
      pending.push(
        (async () => {
          const req = res.request();
          const loc = `${scope} live ${req.method()} ${res.url()}`;
          const post = req.postDataBuffer();
          if (post !== null) scanBytesInto(report, "network", `${loc} (request body)`, post, assetMatcher, needles);
          try {
            scanBytesInto(report, "network", `${loc} (response body)`, await res.body(), assetMatcher, needles);
            liveResponses++;
          } catch {
            // redirects and aborted requests have no body
          }
        })(),
      );
    });

    await page.goto(new URL(route, originUrl).href, { waitUntil: "load" });
    try {
      await page.waitForFunction((mark) => performance.getEntriesByName(mark).length > 0, opts.readyMark, {
        timeout: opts.readyTimeoutMs,
      });
    } catch {
      report.fail(scope, `reader never emitted performance mark "${opts.readyMark}" within ${opts.readyTimeoutMs}ms`);
    }
    await page.waitForTimeout(opts.settleMs);
    if (opts.requireIsolation && !(await page.evaluate(() => crossOriginIsolated))) {
      report.fail(scope, "page is not cross-origin isolated (COOP/COEP missing or broken)");
    }

    // 1. Raw memory first: nothing has forced a GC yet.
    await scanProcessMemory(needles, report, { mode: opts.procMem, scope });
    // 2. Semantic layers.
    await scanDom(context, page, scope, needles, report);
    await scanStorage(context, page, scope, needles, assetMatcher, report);
    // 3. Heap snapshots (these GC).
    await scanHeaps(context, page, scope, needles, report);

    await Promise.allSettled(pending);
    for (const [i, line] of consoleLines.entries()) scanTextInto(report, "console", `${scope} console[${i}]`, line, needles);
    report.cover({ layer: "console", scope, status: "scanned", detail: `${consoleLines.length} messages` });

    await context.close(); // flushes the HAR
    const harEntries = await scanHar(harPath, scope, needles, assetMatcher, report);
    report.cover({ layer: "network", scope, status: "scanned", detail: `${harEntries} HAR entries, ${liveResponses} live bodies` });
  } finally {
    await browser.close();
  }
}
