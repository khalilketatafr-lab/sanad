//! `wasm-bindgen` surface used by the reader's workers.
//!
//! The pointer ring is a `SharedArrayBuffer` the main thread writes (see
//! `packages/lumen/src/input/pointer-ring.ts`). The worker hands it to the
//! Compositor once with [`Compositor::attach_pointer_ring`]. After that,
//! every [`Compositor::poll_pointer`] reads new samples with `Atomics.load` and
//! typed-array copies (no postMessage, no allocation per sample), maps them
//! through the view transform and hit-tests completed taps.

use js_sys::{Atomics, Float32Array, Float64Array, Int32Array, SharedArrayBuffer, Uint32Array};
use wasm_bindgen::prelude::*;

use crate::item::{ItemParams, RunView};
use crate::layout::{GlyphKind, ParagraphLayout, layout_paragraph};
use crate::linebreak::BreakParams;
use crate::ring::{
    HEADER_BYTES, LAYOUT_VERSION, Phase, PointerRingReader, PointerSample, RECORD_F64S, RingMemory,
};

/// `RingMemory` over the shared buffer.
#[derive(Debug)]
struct JsRing {
    header: Int32Array,
    records: Float64Array,
    capacity: u32,
}

impl RingMemory for JsRing {
    fn load_write_seq(&self) -> u32 {
        Atomics::load(&self.header, 0).unwrap_or(0) as u32
    }

    fn capacity(&self) -> u32 {
        self.capacity
    }

    fn copy_records(&self, slot: u32, out: &mut [f64]) {
        let begin = slot * RECORD_F64S;
        let end = begin + out.len() as u32;
        self.records.subarray(begin, end).copy_to(out);
    }
}

/// Validates the header and maps typed-array views over the shared buffer.
fn open_ring(sab: &SharedArrayBuffer) -> Result<JsRing, JsError> {
    let header = Int32Array::new_with_byte_offset_and_length(sab, 0, 4);
    let load = |i: u32| {
        Atomics::load(&header, i).map_err(|_| JsError::new("pointer ring: Atomics unavailable"))
    };
    if load(3)? != LAYOUT_VERSION {
        return Err(JsError::new("pointer ring: unsupported layout version"));
    }
    let capacity = load(1)? as u32;
    let stride = load(2)? as u32;
    if capacity == 0 || !capacity.is_power_of_two() || stride != RECORD_F64S {
        return Err(JsError::new("pointer ring: bad capacity or stride"));
    }
    if sab.byte_length() < HEADER_BYTES + capacity * RECORD_F64S * 8 {
        return Err(JsError::new("pointer ring: buffer too small"));
    }
    let records =
        Float64Array::new_with_byte_offset_and_length(sab, HEADER_BYTES, capacity * RECORD_F64S);
    Ok(JsRing {
        header,
        records,
        capacity,
    })
}

/// Raw consumer for the Render Worker's gesture physics (pinch, inertia),
/// which needs every sample rather than hit-tested taps.
#[wasm_bindgen]
#[derive(Debug)]
pub struct PointerRingConsumer {
    ring: JsRing,
    reader: PointerRingReader,
    samples: Vec<PointerSample>,
    dropped: u32,
}

/// Values per sample returned by [`PointerRingConsumer::poll`].
const SAMPLE_FIELDS: usize = 9;

#[wasm_bindgen]
impl PointerRingConsumer {
    #[wasm_bindgen(constructor)]
    pub fn new(sab: &SharedArrayBuffer) -> Result<PointerRingConsumer, JsError> {
        let ring = open_ring(sab)?;
        let reader = PointerRingReader::attach(&ring);
        Ok(Self {
            ring,
            reader,
            samples: Vec::with_capacity(256),
            dropped: 0,
        })
    }

    /// New samples since the last poll as a flat array of
    /// `[seq, t, x, y, pointerId, phase, device, buttons, pressure]` per sample.
    pub fn poll(&mut self) -> Float64Array {
        self.samples.clear();
        let stats = self.reader.poll(&self.ring, &mut self.samples);
        self.dropped = self.dropped.wrapping_add(stats.dropped);
        let mut out = Vec::with_capacity(self.samples.len() * SAMPLE_FIELDS);
        for s in &self.samples {
            out.extend_from_slice(&[
                f64::from(s.seq),
                s.t,
                f64::from(s.x),
                f64::from(s.y),
                f64::from(s.pointer_id),
                f64::from(s.phase as u8),
                f64::from(s.device as u8),
                f64::from(s.buttons),
                f64::from(s.pressure),
            ]);
        }
        Float64Array::from(out.as_slice())
    }

    /// Samples lost to overrun or torn reads since construction.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
}

/// A tap in progress: where the pointer went down, and when.
#[derive(Debug, Clone, Copy)]
struct Press {
    pointer_id: u16,
    x: f32,
    y: f32,
    t: f64,
}

const TAP_SLOP_PX: f32 = 8.0;
const TAP_MAX_MS: f64 = 500.0;

#[wasm_bindgen]
#[derive(Debug)]
pub struct Compositor {
    layout: Option<ParagraphLayout>,
    line_height: f32,
    /// Affine map surface CSS px → layout space: x' = a·x + c·y + e, y' = b·x + d·y + f.
    view: [f32; 6],
    ring: Option<JsRing>,
    reader: PointerRingReader,
    samples: Vec<PointerSample>,
    press: Option<Press>,
    dropped: u32,
}

#[wasm_bindgen]
impl Compositor {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            layout: None,
            line_height: 1.0,
            view: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            ring: None,
            reader: PointerRingReader::default(),
            samples: Vec::with_capacity(256),
            press: None,
            dropped: 0,
        }
    }

    /// Spike entry point: lays out one single-run paragraph from SoA arrays.
    /// Production reads runs straight from the decrypted Folio chunk in WASM
    /// memory, so nothing crosses into JS but positions.
    /// Returns the number of lines.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_paragraph(
        &mut self,
        gids: &[u16],
        advances: &[i16],
        offsets: &[i16],
        flags: &[u8],
        bidi_levels: &[u8],
        kashida_priority: &[u8],
        kashida_max: &[u16],
        scale: f32,
        width: f32,
        base_level: u8,
        hyphen_gid: u16,
        hyphen_advance: i16,
        tatweel_gid: u16,
        tatweel_advance: i16,
    ) -> Result<u32, JsError> {
        let n = gids.len();
        if advances.len() != n
            || flags.len() != n
            || bidi_levels.len() != n
            || !(offsets.is_empty() || offsets.len() == 2 * n)
        {
            return Err(JsError::new("layout_paragraph: SoA length mismatch"));
        }
        let offsets: Vec<[i16; 2]> = offsets.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
        let run = RunView {
            scale,
            gids,
            advances,
            offsets: &offsets,
            flags,
            bidi_levels,
            kashida_priority,
            kashida_max,
            hyphen: (hyphen_advance > 0).then_some((hyphen_gid, hyphen_advance)),
            tatweel: (tatweel_advance > 0).then_some((tatweel_gid, tatweel_advance)),
        };
        let layout = layout_paragraph(
            &[run],
            width,
            base_level,
            &ItemParams::default(),
            &BreakParams::default(),
        );
        let lines = layout.lines.len() as u32;
        self.layout = Some(layout);
        Ok(lines)
    }

    /// Positioned glyphs of a line as `[gid, x, y, scale_x, kind]*` (y: offset
    /// from the baseline, positive up; kind: 0 glyph, 1 kashida, 2 hyphen).
    pub fn line_glyphs(&self, line: u32) -> Float32Array {
        let mut v: Vec<f32> = Vec::new();
        if let Some(l) = self
            .layout
            .as_ref()
            .and_then(|p| p.lines.get(line as usize))
        {
            for g in &l.glyphs {
                let kind = match g.kind {
                    GlyphKind::Glyph => 0.0,
                    GlyphKind::Kashida => 1.0,
                    GlyphKind::Hyphen => 2.0,
                };
                v.extend_from_slice(&[f32::from(g.gid), g.x, g.y, g.scale_x, kind]);
            }
        }
        Float32Array::from(v.as_slice())
    }

    pub fn set_line_height(&mut self, h: f32) {
        self.line_height = if h > 0.0 { h } else { 1.0 };
    }

    /// Surface CSS px → layout space (inverse of the camera).
    #[allow(clippy::many_single_char_names)] // affine matrix entries, conventional names
    pub fn set_view_transform(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        self.view = [a, b, c, d, e, f];
    }

    /// Attaches the main thread's pointer ring. Reading starts at the
    /// producer's current position.
    pub fn attach_pointer_ring(&mut self, sab: &SharedArrayBuffer) -> Result<(), JsError> {
        let ring = open_ring(sab)?;
        self.reader = PointerRingReader::attach(&ring);
        self.ring = Some(ring);
        Ok(())
    }

    /// Drains new pointer samples and returns completed taps, hit-tested, as
    /// `[line, cluster]*`. A tap is down→up within 8 px and 500 ms.
    pub fn poll_pointer(&mut self) -> Uint32Array {
        let mut taps: Vec<u32> = Vec::new();
        let Some(ring) = self.ring.as_ref() else {
            return Uint32Array::new_with_length(0);
        };
        self.samples.clear();
        let stats = self.reader.poll(ring, &mut self.samples);
        self.dropped = self.dropped.wrapping_add(stats.dropped);
        for s in &self.samples {
            match s.phase {
                Phase::Down => {
                    self.press = Some(Press {
                        pointer_id: s.pointer_id,
                        x: s.x,
                        y: s.y,
                        t: s.t,
                    });
                }
                Phase::Up => {
                    let tap = self.press.take().filter(|p| {
                        p.pointer_id == s.pointer_id
                            && (s.x - p.x).hypot(s.y - p.y) <= TAP_SLOP_PX
                            && s.t - p.t <= TAP_MAX_MS
                    });
                    if let Some((line, cluster)) = tap.and_then(|_| self.hit(s.x, s.y)) {
                        taps.extend_from_slice(&[line, cluster]);
                    }
                }
                Phase::Cancel => self.press = None,
                Phase::Move => {}
            }
        }
        Uint32Array::from(taps.as_slice())
    }

    /// Samples lost to overrun or torn reads since attach (telemetry).
    pub fn dropped_samples(&self) -> u32 {
        self.dropped
    }
}

impl Compositor {
    #[allow(clippy::many_single_char_names)] // affine matrix entries, conventional names
    fn hit(&self, sx: f32, sy: f32) -> Option<(u32, u32)> {
        let [a, b, c, d, e, f] = self.view;
        let x = a * sx + c * sy + e;
        let y = b * sx + d * sy + f;
        if y < 0.0 {
            return None;
        }
        let line = (y / self.line_height) as usize;
        let cluster = self.layout.as_ref()?.hit_test(line, x)?;
        Some((line as u32, cluster))
    }
}

impl Default for Compositor {
    fn default() -> Self {
        Self::new()
    }
}
