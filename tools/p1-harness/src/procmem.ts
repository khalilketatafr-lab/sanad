/**
 * Layer "procmem": raw memory sweep of every browser process (Linux).
 *
 * This is the authoritative heap check. A V8 heap snapshot only shows *live*
 * strings, truncates long ones, and never shows ArrayBuffer or WASM linear
 * memory. Reading /proc/<pid>/mem sees everything the browser holds:
 *   - JS strings in every isolate (page, dedicated workers, service worker),
 *     both Latin-1 and UTF-16 representations;
 *   - ArrayBuffers, typed arrays, WASM linear memory (Folio decoder, Compositor);
 *   - Blink DOM/text storage, network buffers, shared memory (memfd, /dev/shm);
 *   - garbage that was freed but not yet overwritten, i.e. transient plaintext.
 * It must therefore run BEFORE anything that forces a GC (heap snapshots).
 *
 * Requirements: Linux, and permission to read the browser's memory (same uid
 * with yama.ptrace_scope ≤ 1 for descendants, or root/CAP_SYS_PTRACE). CI runs
 * this layer with `--proc-mem=require` so it can never be skipped silently.
 */
import { closeSync, openSync, readSync } from "node:fs";
import { readdir, readFile } from "node:fs/promises";
import type { NeedleSet } from "./canary.ts";
import { ByteMatcher, excerptBytes } from "./match.ts";
import type { Report } from "./report.ts";

const CHUNK = 8 * 1024 * 1024;
const PAGE = 4096;

interface Region {
  readonly start: number;
  readonly end: number;
  readonly perms: string;
  readonly path: string;
}

async function parentMap(): Promise<Map<number, number[]>> {
  const children = new Map<number, number[]>();
  for (const name of await readdir("/proc")) {
    if (!/^\d+$/u.test(name)) continue;
    try {
      const stat = await readFile(`/proc/${name}/stat`, "utf8");
      // comm (field 2) may contain spaces and parentheses: parse after the last ')'.
      const rest = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
      const ppid = Number(rest[1]);
      const pid = Number(name);
      const list = children.get(ppid) ?? [];
      list.push(pid);
      children.set(ppid, list);
    } catch {
      // process exited while listing
    }
  }
  return children;
}

/** All descendants of `root` whose executable looks like a Chromium build. */
export async function browserProcesses(root: number = process.pid): Promise<Array<{ pid: number; type: string }>> {
  const children = await parentMap();
  const out: Array<{ pid: number; type: string }> = [];
  const queue = [...(children.get(root) ?? [])];
  for (let i = 0; i < queue.length; i++) {
    const pid = queue[i];
    if (pid === undefined) break;
    queue.push(...(children.get(pid) ?? []));
    try {
      const cmd = (await readFile(`/proc/${pid}/cmdline`, "utf8")).split("\0");
      const exe = cmd[0] ?? "";
      if (!/chrom|headless_shell/iu.test(exe)) continue;
      const typeArg = cmd.find((a) => a.startsWith("--type="));
      out.push({ pid, type: typeArg?.slice("--type=".length) ?? "browser" });
    } catch {
      // exited
    }
  }
  return out;
}

function scannable(r: Region): boolean {
  if (r.perms[0] !== "r") return false;
  const p = r.path;
  if (p === "") return true; // anonymous: V8 heaps, partition alloc, WASM memory, ArrayBuffers
  if (p === "[vvar]" || p === "[vdso]" || p === "[vsyscall]" || p === "[vvar_vclock]") return false;
  if (p.startsWith("[")) return true; // [heap], [stack], [anon:…]
  if (p.startsWith("/memfd:") || p.startsWith("/dev/shm/")) return true; // shared memory (Mojo)
  if (p.endsWith("(deleted)")) return true; // unlinked shared-memory files
  return false; // file-backed code/data: libraries and resources
}

async function regions(pid: number): Promise<Region[]> {
  const maps = await readFile(`/proc/${pid}/maps`, "utf8");
  const out: Region[] = [];
  for (const line of maps.split("\n")) {
    const m = /^([0-9a-f]+)-([0-9a-f]+)\s+(\S{4})\s+\S+\s+\S+\s+\d+\s*(.*)$/u.exec(line);
    if (m === null) continue;
    out.push({
      start: Number.parseInt(m[1] ?? "0", 16),
      end: Number.parseInt(m[2] ?? "0", 16),
      perms: m[3] ?? "----",
      path: (m[4] ?? "").trim(),
    });
  }
  return out;
}

export interface ProcMemOptions {
  readonly mode: "off" | "auto" | "require";
  readonly scope: string;
}

export async function scanProcessMemory(needles: NeedleSet, report: Report, opts: ProcMemOptions): Promise<void> {
  if (opts.mode === "off") {
    report.cover({ layer: "procmem", scope: opts.scope, status: "skipped", detail: "disabled (--proc-mem=off)" });
    return;
  }
  const skip = (detail: string): void => {
    report.cover({
      layer: "procmem",
      scope: opts.scope,
      status: opts.mode === "require" ? "error" : "skipped",
      detail,
    });
  };
  if (process.platform !== "linux") {
    skip(`unsupported platform ${process.platform}`);
    return;
  }

  const procs = await browserProcesses();
  if (procs.length === 0) {
    skip("no browser processes found under the harness process tree");
    return;
  }

  const matcher = new ByteMatcher(needles.bytes);
  const buf = new Uint8Array(CHUNK);
  let scannedBytes = 0;
  let unreadable = 0;
  let deniedProcs = 0;
  const started = performance.now();

  for (const proc of procs) {
    let fd: number;
    let regs: Region[];
    try {
      regs = (await regions(proc.pid)).filter(scannable);
      fd = openSync(`/proc/${proc.pid}/mem`, "r");
    } catch {
      deniedProcs++;
      continue;
    }
    try {
      for (const r of regs) {
        const stream = matcher.stream();
        for (let addr = r.start; addr < r.end; ) {
          const len = Math.min(CHUNK, r.end - addr);
          const view = buf.subarray(0, len);
          let got = 0;
          try {
            got = readSync(fd, view, 0, len, addr);
          } catch {
            got = 0;
          }
          if (got <= 0) {
            // Unreadable range (guard page, racing unmap): skip one page and resync.
            const skipLen = Math.min(PAGE, r.end - addr);
            unreadable += skipLen;
            stream.reset(skipLen);
            addr += skipLen;
            continue;
          }
          const data = view.subarray(0, got);
          const base = stream.position;
          for (const hit of stream.feed(data)) {
            const rel = hit.offset - base;
            report.add({
              layer: "procmem",
              canaryId: hit.needle.canaryId,
              needle: `${hit.needle.kind}:${hit.needle.label}`,
              location: `${opts.scope} pid ${proc.pid} (${proc.type}) region ${r.path || "[anon]"}`,
              offset: r.start + hit.offset,
              excerpt: rel >= 0 ? excerptBytes(data, rel, hit.needle.bytes.length, hit.needle.encoding) : "(spans chunk boundary)",
            });
          }
          scannedBytes += got;
          addr += got;
        }
      }
    } finally {
      closeSync(fd);
    }
  }

  if (deniedProcs === procs.length) {
    skip(`permission denied for all ${procs.length} browser processes (run as root or with CAP_SYS_PTRACE)`);
    return;
  }
  const secs = ((performance.now() - started) / 1000).toFixed(1);
  const mb = (scannedBytes / 1048576).toFixed(0);
  report.cover({
    layer: "procmem",
    scope: opts.scope,
    status: deniedProcs > 0 && opts.mode === "require" ? "error" : "scanned",
    detail:
      `${procs.length - deniedProcs}/${procs.length} processes, ${mb} MiB in ${secs}s` +
      (unreadable > 0 ? `, ${(unreadable / 1048576).toFixed(1)} MiB unreadable` : "") +
      (deniedProcs > 0 ? `, ${deniedProcs} denied` : ""),
  });
}
