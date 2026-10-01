/**
 * Producer side of the pointer-sample ring (main thread → workers).
 *
 * Wire layout and publication protocol are specified in
 * crates/compositor/src/ring.rs. The main thread writes every pointer event
 * here and never blocks, never allocates, never posts a message. The Render
 * Worker (gesture physics) and the Compositor (hit-testing) read it
 * independently through wasm (`PointerRingConsumer`, `Compositor.attach_pointer_ring`).
 *
 * Requires cross-origin isolation (COOP/COEP), which the reader origin has.
 */

export const RING_LAYOUT = {
  headerBytes: 16,
  recordF64s: 4,
  version: 1,
  slot: { writeSeq: 0, capacity: 1, stride: 2, version: 3 },
} as const;

export const PHASE = { move: 0, down: 1, up: 2, cancel: 3 } as const;
export const DEVICE = { mouse: 0, touch: 1, pen: 2 } as const;
export type PointerPhase = keyof typeof PHASE;
export type PointerDevice = keyof typeof DEVICE;

export interface PointerSampleInput {
  /** Epoch ms: `performance.timeOrigin + event.timeStamp` (same clock in every worker). */
  readonly t: number;
  /** CSS px relative to the reader surface. */
  readonly x: number;
  readonly y: number;
  readonly pointerId: number;
  readonly phase: PointerPhase;
  readonly device: PointerDevice;
  readonly buttons: number;
  readonly pressure: number;
}

/**
 * `meta` packs integer fields into one f64 (exact below 2^53). JS bitwise
 * operators are 32-bit, so the high fields are composed arithmetically.
 */
export function encodeMeta(s: Pick<PointerSampleInput, "pointerId" | "phase" | "device" | "buttons" | "pressure">): number {
  const id = s.pointerId >>> 0 & 0xffff;
  const pressure = Math.round(Math.min(1, Math.max(0, s.pressure)) * 255);
  return id + PHASE[s.phase] * 2 ** 16 + DEVICE[s.device] * 2 ** 18 + (s.buttons & 0xff) * 2 ** 20 + pressure * 2 ** 28;
}

export class PointerRingProducer {
  readonly buffer: SharedArrayBuffer;
  readonly capacity: number;
  readonly #header: Int32Array;
  readonly #records: Float64Array;
  readonly #mask: number;
  #seq: number;

  private constructor(buffer: SharedArrayBuffer, capacity: number) {
    this.buffer = buffer;
    this.capacity = capacity;
    this.#header = new Int32Array(buffer, 0, 4);
    this.#records = new Float64Array(buffer, RING_LAYOUT.headerBytes, capacity * RING_LAYOUT.recordF64s);
    this.#mask = capacity - 1;
    this.#seq = Atomics.load(this.#header, RING_LAYOUT.slot.writeSeq) >>> 0;
  }

  /** Allocates and initializes a ring. `capacity` must be a power of two. */
  static create(capacity = 256): PointerRingProducer {
    if (!Number.isInteger(capacity) || capacity < 2 || (capacity & (capacity - 1)) !== 0) {
      throw new RangeError("pointer ring capacity must be a power of two ≥ 2");
    }
    if (typeof SharedArrayBuffer === "undefined") {
      throw new Error("SharedArrayBuffer unavailable: the reader origin must be cross-origin isolated");
    }
    const buffer = new SharedArrayBuffer(RING_LAYOUT.headerBytes + capacity * RING_LAYOUT.recordF64s * 8);
    const header = new Int32Array(buffer, 0, 4);
    Atomics.store(header, RING_LAYOUT.slot.capacity, capacity);
    Atomics.store(header, RING_LAYOUT.slot.stride, RING_LAYOUT.recordF64s);
    Atomics.store(header, RING_LAYOUT.slot.writeSeq, 0);
    // Version last: consumers refuse a ring whose version is not yet set.
    Atomics.store(header, RING_LAYOUT.slot.version, RING_LAYOUT.version);
    return new PointerRingProducer(buffer, capacity);
  }

  /** Writes one sample, then publishes it with a sequentially consistent store. */
  push(s: PointerSampleInput): void {
    const seq = this.#seq;
    const base = (seq & this.#mask) * RING_LAYOUT.recordF64s;
    const r = this.#records;
    r[base] = s.t;
    r[base + 1] = s.x;
    r[base + 2] = s.y;
    r[base + 3] = encodeMeta(s);
    this.#seq = (seq + 1) >>> 0;
    Atomics.store(this.#header, RING_LAYOUT.slot.writeSeq, this.#seq | 0);
  }

  /** Convenience for DOM pointer events on the reader surface. */
  pushEvent(e: PointerEvent, phase: PointerPhase, surface: { readonly left: number; readonly top: number }): void {
    this.push({
      t: performance.timeOrigin + e.timeStamp,
      x: e.clientX - surface.left,
      y: e.clientY - surface.top,
      pointerId: e.pointerId,
      phase,
      device: e.pointerType === "touch" ? "touch" : e.pointerType === "pen" ? "pen" : "mouse",
      buttons: e.buttons,
      pressure: e.pressure,
    });
  }
}
