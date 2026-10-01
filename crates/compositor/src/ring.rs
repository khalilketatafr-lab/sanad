//! Pointer-sample ring over a `SharedArrayBuffer` (main thread → workers).
//!
//! The main thread is the only producer. It writes pointer samples into a
//! fixed ring and never waits. Consumers (Render Worker physics, Compositor
//! hit-testing) read independently with their own cursor, so one slow
//! consumer never stalls input or another consumer. Requires cross-origin
//! isolation (COOP/COEP), which the reader origin has.
//!
//! ```text
//! byte 0   Int32Array header[4]:
//!            [0] WRITE_SEQ   total records ever written (wrapping u32), Atomics.store'd last
//!            [1] CAPACITY    records in the ring (power of two)
//!            [2] STRIDE      f64 slots per record (= 4)
//!            [3] VERSION     layout version (= 1)
//! byte 16  Float64Array records[CAPACITY × 4]:
//!            [0] t      epoch ms (performance.timeOrigin + now(): same clock in every worker)
//!            [1] x      CSS px, relative to the reader surface
//!            [2] y      CSS px
//!            [3] meta   pointerId (bits 0–15) | phase (16–17) | device (18–19)
//!                       | buttons (20–27) | pressure·255 (28–35), an exact integer in f64
//! ```
//!
//! Publication protocol (seqlock style): the producer writes record `s` into
//! slot `s mod CAPACITY`, then `Atomics.store(WRITE_SEQ, s+1)`. A consumer
//! loads `WRITE_SEQ` (acquire), copies the new records, then reloads it. Any
//! record whose slot the producer may have started overwriting during the
//! copy (`seq ≤ WRITE_SEQ′ − CAPACITY`) is discarded as possibly torn.

pub const HEADER_BYTES: u32 = 16;
pub const RECORD_F64S: u32 = 4;
pub const LAYOUT_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Move = 0,
    Down = 1,
    Up = 2,
    Cancel = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Mouse = 0,
    Touch = 1,
    Pen = 2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSample {
    pub seq: u32,
    pub t: f64,
    pub x: f32,
    pub y: f32,
    pub pointer_id: u16,
    pub phase: Phase,
    pub device: Device,
    pub buttons: u8,
    pub pressure: f32,
}

impl PointerSample {
    fn decode(seq: u32, rec: &[f64]) -> Option<Self> {
        let [t, x, y, meta] = *rec else { return None };
        if !(t.is_finite() && x.is_finite() && y.is_finite() && meta.is_finite()) || meta < 0.0 {
            return None;
        }
        let m = meta as u64;
        let phase = match (m >> 16) & 0b11 {
            0 => Phase::Move,
            1 => Phase::Down,
            2 => Phase::Up,
            _ => Phase::Cancel,
        };
        let device = match (m >> 18) & 0b11 {
            1 => Device::Touch,
            2 => Device::Pen,
            _ => Device::Mouse,
        };
        Some(Self {
            seq,
            t,
            x: x as f32,
            y: y as f32,
            pointer_id: (m & 0xffff) as u16,
            phase,
            device,
            buttons: ((m >> 20) & 0xff) as u8,
            pressure: ((m >> 28) & 0xff) as f32 / 255.0,
        })
    }
}

/// Shared-memory access the reader needs. The wasm build backs this with
/// `Atomics.load` and typed-array copies; tests back it with plain memory.
pub trait RingMemory {
    /// Atomic (sequentially consistent) load of WRITE_SEQ.
    fn load_write_seq(&self) -> u32;
    fn capacity(&self) -> u32;
    /// Copy `out.len() / 4` records starting at ring slot `slot` (no wrap).
    fn copy_records(&self, slot: u32, out: &mut [f64]);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollStats {
    pub delivered: u32,
    /// Overrun (consumer lagged more than CAPACITY) or torn and discarded.
    pub dropped: u32,
}

/// One consumer's cursor.
#[derive(Debug, Clone, Default)]
pub struct PointerRingReader {
    next: u32,
    scratch: Vec<f64>,
}

/// `a < b` in wrapping u32 sequence space (valid for distances < 2³¹).
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

impl PointerRingReader {
    /// Starts reading at the producer's current position (ignores history).
    pub fn attach<M: RingMemory>(mem: &M) -> Self {
        Self {
            next: mem.load_write_seq(),
            scratch: Vec::new(),
        }
    }

    pub fn poll<M: RingMemory>(&mut self, mem: &M, out: &mut Vec<PointerSample>) -> PollStats {
        let cap = mem.capacity();
        let write = mem.load_write_seq();
        let pending = write.wrapping_sub(self.next);
        if pending == 0 || cap == 0 {
            return PollStats::default();
        }
        let mut stats = PollStats::default();
        let mut start = self.next;
        if pending > cap {
            stats.dropped += pending - cap;
            start = write.wrapping_sub(cap);
        }
        let count = write.wrapping_sub(start);

        // Copy [start, write) in at most two contiguous segments.
        self.scratch.resize((count * RECORD_F64S) as usize, 0.0);
        let first_slot = start % cap;
        let first_len = count.min(cap - first_slot);
        let (a, b) = self
            .scratch
            .split_at_mut((first_len * RECORD_F64S) as usize);
        mem.copy_records(first_slot, a);
        if count > first_len {
            mem.copy_records(0, b);
        }

        // Anything the producer may have begun overwriting during the copy is suspect.
        let write_after = mem.load_write_seq();
        let valid_from = write_after.wrapping_sub(cap).wrapping_add(1);
        for i in 0..count {
            let seq = start.wrapping_add(i);
            let rec = &self.scratch[(i * RECORD_F64S) as usize..((i + 1) * RECORD_F64S) as usize];
            // Slot of `seq` is reused by `seq + cap`; if that write may have begun, drop it.
            if seq_lt(seq, valid_from) {
                stats.dropped += 1;
                continue;
            }
            match PointerSample::decode(seq, rec) {
                Some(s) => {
                    out.push(s);
                    stats.delivered += 1;
                }
                None => stats.dropped += 1,
            }
        }
        self.next = write;
        stats
    }
}

/// Encodes the meta field (shared with the TS producer for tests).
#[must_use]
pub fn encode_meta(
    pointer_id: u16,
    phase: Phase,
    device: Device,
    buttons: u8,
    pressure: f32,
) -> f64 {
    let p = (pressure.clamp(0.0, 1.0) * 255.0).round() as u64;
    (u64::from(pointer_id)
        | (phase as u64) << 16
        | (device as u64) << 18
        | u64::from(buttons) << 20
        | p << 28) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// Plain-memory ring with a hook to simulate a concurrent producer.
    struct TestRing {
        seq: Cell<u32>,
        cap: u32,
        data: RefCell<Vec<f64>>,
        /// Records the "producer" writes between the consumer's copy and recheck.
        race: Cell<u32>,
    }

    impl TestRing {
        fn new(cap: u32) -> Self {
            Self {
                seq: Cell::new(0),
                cap,
                data: RefCell::new(vec![0.0; (cap * RECORD_F64S) as usize]),
                race: Cell::new(0),
            }
        }
        fn push(&self, x: f64, phase: Phase) {
            let s = self.seq.get();
            let slot = (s % self.cap) as usize * 4;
            let mut d = self.data.borrow_mut();
            d[slot..slot + 4].copy_from_slice(&[
                f64::from(s),
                x,
                1.0,
                encode_meta(7, phase, Device::Touch, 1, 0.5),
            ]);
            self.seq.set(s.wrapping_add(1));
        }
    }

    impl RingMemory for TestRing {
        fn load_write_seq(&self) -> u32 {
            self.seq.get()
        }
        fn capacity(&self) -> u32 {
            self.cap
        }
        fn copy_records(&self, slot: u32, out: &mut [f64]) {
            let from = (slot * RECORD_F64S) as usize;
            out.copy_from_slice(&self.data.borrow()[from..from + out.len()]);
            for _ in 0..self.race.replace(0) {
                self.push(-1.0, Phase::Move);
            }
        }
    }

    #[test]
    fn delivers_in_order_and_decodes_meta() {
        let ring = TestRing::new(8);
        let mut reader = PointerRingReader::attach(&ring);
        ring.push(10.0, Phase::Down);
        ring.push(11.0, Phase::Up);
        let mut out = Vec::new();
        assert_eq!(
            reader.poll(&ring, &mut out),
            PollStats {
                delivered: 2,
                dropped: 0
            }
        );
        assert_eq!(out[0].phase, Phase::Down);
        assert!((out[1].x - 11.0).abs() < f32::EPSILON);
        assert_eq!(
            (out[1].pointer_id, out[1].device, out[1].buttons),
            (7, Device::Touch, 1)
        );
        assert!((out[1].pressure - 0.5).abs() < 0.01);
        assert_eq!(reader.poll(&ring, &mut out).delivered, 0, "nothing new");
    }

    #[test]
    fn overrun_drops_oldest_and_keeps_newest() {
        let ring = TestRing::new(8);
        let mut reader = PointerRingReader::attach(&ring);
        for i in 0..20 {
            ring.push(f64::from(i), Phase::Move);
        }
        let mut out = Vec::new();
        let stats = reader.poll(&ring, &mut out);
        assert_eq!(stats.dropped + stats.delivered, 20);
        assert!(stats.dropped >= 12);
        assert_eq!(out.last().map(|s| s.x), Some(19.0));
    }

    #[test]
    fn records_overwritten_during_copy_are_discarded_not_delivered_torn() {
        let ring = TestRing::new(8);
        let mut reader = PointerRingReader::attach(&ring);
        for i in 0..8 {
            ring.push(f64::from(i), Phase::Move);
        }
        ring.race.set(3); // producer laps 3 slots while we copy
        let mut out = Vec::new();
        let stats = reader.poll(&ring, &mut out);
        assert!(stats.dropped >= 3, "{stats:?}");
        assert!(
            out.iter().all(|s| s.x >= 0.0),
            "a torn/overwritten record leaked: {out:?}"
        );
        // The 3 racing records are picked up by the next poll.
        let mut next = Vec::new();
        assert_eq!(reader.poll(&ring, &mut next).delivered, 3);
    }

    #[test]
    fn sequence_wraparound_is_seamless() {
        let ring = TestRing::new(4);
        ring.seq.set(u32::MAX - 1);
        let mut reader = PointerRingReader::attach(&ring);
        for i in 0..3 {
            ring.push(f64::from(i), Phase::Move);
        }
        let mut out = Vec::new();
        assert_eq!(reader.poll(&ring, &mut out).delivered, 3);
        assert!(out.iter().map(|s| s.x).eq([0.0f32, 1.0, 2.0]));
    }
}
