/**
 * worker_threads consumer for the ring stress test: runs the real wasm
 * PointerRingConsumer against a SharedArrayBuffer the main thread is writing
 * to concurrently, and checks every delivered sample is internally consistent.
 */
import { readFileSync } from "node:fs";
import { parentPort, workerData } from "node:worker_threads";
import { PointerRingConsumer, initSync } from "@sanad/lumen-wasm";

const { sab, wasmPath, total } = workerData as { sab: SharedArrayBuffer; wasmPath: string; total: number };
initSync({ module: readFileSync(wasmPath) });
const consumer = new PointerRingConsumer(sab);
const FIELDS = 9;
let delivered = 0;
let torn = 0;
let nonMonotonic = 0;
let lastSeq = -1;
let lastX = -1;
const deadline = Date.now() + 20_000;
parentPort?.postMessage({ ready: true });

while (Date.now() < deadline) {
  const batch = consumer.poll();
  for (let i = 0; i < batch.length; i += FIELDS) {
    const seq = batch[i] ?? -1;
    const t = batch[i + 1] ?? -1;
    const x = batch[i + 2] ?? -1;
    const y = batch[i + 3] ?? -1;
    const id = batch[i + 4] ?? -1;
    // Producer writes t = 2x, y = x, pointerId = x mod 65536 for sample number x.
    if (y !== x || t !== 2 * x || id !== x % 65536) torn++;
    if (seq <= lastSeq || x <= lastX) nonMonotonic++;
    lastSeq = seq;
    lastX = x;
    delivered++;
  }
  if (lastX === total - 1) break;
}
parentPort?.postMessage({ delivered, dropped: consumer.dropped(), torn, nonMonotonic, lastX });
