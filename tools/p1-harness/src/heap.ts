/**
 * Layer "heap": V8 heap snapshots via the Chrome DevTools Protocol.
 *
 * Covers the page's main isolate and, through Target auto-attach, every
 * dedicated worker (Vault Worker, Render Worker). Snapshots give precise
 * attribution ("a live JS string in the Vault Worker"), while the procmem
 * layer gives completeness, because snapshots:
 *   - force a GC first (transient plaintext disappears; procmem runs before this),
 *   - may truncate very long strings,
 *   - exclude ArrayBuffer and WASM memory contents.
 */
import type { BrowserContext, CDPSession, Page } from "playwright";
import type { NeedleSet } from "./canary.ts";
import { matchText } from "./match.ts";
import type { Report } from "./report.ts";

interface SnapshotJson {
  readonly strings: readonly string[];
}

const MIN_LEN = 12;

function scanSnapshot(json: string, scope: string, needles: NeedleSet, report: Report): number {
  const snap = JSON.parse(json) as SnapshotJson;
  // Join with a separator that normalizes to a word boundary, then match once.
  const candidates = snap.strings.filter((s) => s.length >= MIN_LEN);
  for (const hit of matchText(candidates.join("\n \n"), needles.text)) {
    report.add({
      layer: "heap",
      canaryId: hit.needle.canaryId,
      needle: `${hit.needle.kind}:normalized`,
      location: scope,
      excerpt: hit.excerpt,
    });
  }
  return snap.strings.length;
}

async function snapshotPage(session: CDPSession): Promise<string> {
  const chunks: string[] = [];
  const onChunk = (e: { chunk: string }): void => {
    chunks.push(e.chunk);
  };
  session.on("HeapProfiler.addHeapSnapshotChunk", onChunk);
  try {
    await session.send("HeapProfiler.enable");
    await session.send("HeapProfiler.takeHeapSnapshot", { reportProgress: false, captureNumericValue: false });
  } finally {
    session.off("HeapProfiler.addHeapSnapshotChunk", onChunk);
  }
  return chunks.join("");
}

interface ChildTarget {
  readonly sessionId: string;
  readonly type: string;
  readonly url: string;
}

/**
 * Talks to an auto-attached child target (worker) through the page session
 * using non-flattened Target.sendMessageToTarget, which Playwright's public
 * CDPSession API supports.
 */
class ChildChannel {
  #id = 0;
  readonly #pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  readonly #listeners = new Map<string, Array<(params: unknown) => void>>();

  readonly parent: CDPSession;
  readonly target: ChildTarget;

  constructor(parent: CDPSession, target: ChildTarget) {
    this.parent = parent;
    this.target = target;
  }

  dispatch(raw: string): void {
    const msg = JSON.parse(raw) as { id?: number; result?: unknown; error?: { message: string }; method?: string; params?: unknown };
    if (msg.id !== undefined) {
      const p = this.#pending.get(msg.id);
      if (p === undefined) return;
      this.#pending.delete(msg.id);
      if (msg.error) p.reject(new Error(msg.error.message));
      else p.resolve(msg.result);
    } else if (msg.method !== undefined) {
      for (const l of this.#listeners.get(msg.method) ?? []) l(msg.params);
    }
  }

  on(method: string, fn: (params: unknown) => void): void {
    const list = this.#listeners.get(method) ?? [];
    list.push(fn);
    this.#listeners.set(method, list);
  }

  async send(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
    const id = ++this.#id;
    const done = new Promise<unknown>((resolve, reject) => this.#pending.set(id, { resolve, reject }));
    await this.parent.send("Target.sendMessageToTarget", {
      sessionId: this.target.sessionId,
      message: JSON.stringify({ id, method, params }),
    });
    return done;
  }
}

async function snapshotChild(channel: ChildChannel): Promise<string> {
  const chunks: string[] = [];
  channel.on("HeapProfiler.addHeapSnapshotChunk", (p) => {
    chunks.push((p as { chunk: string }).chunk);
  });
  await channel.send("HeapProfiler.enable");
  await channel.send("HeapProfiler.takeHeapSnapshot", { reportProgress: false });
  return chunks.join("");
}

export async function scanHeaps(
  context: BrowserContext,
  page: Page,
  scope: string,
  needles: NeedleSet,
  report: Report,
): Promise<void> {
  const session = await context.newCDPSession(page);
  try {
    const children = new Map<string, ChildChannel>();
    session.on("Target.attachedToTarget", (e) => {
      const info = e.targetInfo;
      children.set(e.sessionId, new ChildChannel(session, { sessionId: e.sessionId, type: info.type, url: info.url }));
    });
    session.on("Target.receivedMessageFromTarget", (e) => {
      children.get(e.sessionId)?.dispatch(e.message);
    });
    // Workers that already exist are reported immediately on enabling auto-attach.
    await session.send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: false });

    const pageStrings = scanSnapshot(await snapshotPage(session), `${scope} main thread`, needles, report);
    report.cover({ layer: "heap", scope: `${scope} main thread`, status: "scanned", detail: `${pageStrings} strings` });

    for (const child of children.values()) {
      const label = `${scope} ${child.target.type} ${new URL(child.target.url).pathname}`;
      try {
        const n = scanSnapshot(await snapshotChild(child), label, needles, report);
        report.cover({ layer: "heap", scope: label, status: "scanned", detail: `${n} strings` });
      } catch (err) {
        report.cover({ layer: "heap", scope: label, status: "error", detail: String(err) });
      }
    }
    await session.send("Target.setAutoAttach", { autoAttach: false, waitForDebuggerOnStart: false, flatten: false });
  } catch (err) {
    report.cover({ layer: "heap", scope, status: "error", detail: String(err) });
  } finally {
    await session.detach().catch(() => undefined);
  }
}
