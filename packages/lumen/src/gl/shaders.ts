/**
 * Lumen GLSL ES 3.00 shaders.
 *
 * Pass 1 (coverage) rasterizes shredded MSDF glyph fragments into an RGBA8
 * target. It writes an ENCODED SCREEN-SPACE SIGNED DISTANCE per ink role,
 * not coverage:
 *
 *     enc = clamp(sd_px / DIST_RANGE_PX + 0.5, 0, 1)      sd_px > 0 inside
 *
 * with blendEquation(MAX). Because coverage is monotone in distance, MAX over
 * encoded distances is exactly the union of the fragments (see
 * docs/spikes/s1-glyph-shredding.md). Keeping distance, not coverage, also
 * lets Pass 3 apply optical weight compensation (u_weightPx) per theme
 * without re-running Pass 1.
 *
 * Pass 3 (composite) decodes the distances, shifts the edge by u_weightPx,
 * applies a 1 px analytic AA ramp and the per-theme coverage curve, then
 * paper, images, highlight underlays, ink roles, warmth, the anti-glare
 * luminance ceiling, extra-dim and linear → sRGB.
 *
 * All colors arrive as LINEAR sRGB (generated from tokens, see @sanad/tokens).
 */

/** ±2 px around the edge survive encoding: 1/64 px resolution in 8 bits. */
export const DIST_RANGE_PX = 4.0;

/** Instance layout for Pass 1 (bytes). Shared with the Compositor's emitter. */
export const GLYPH_INSTANCE = {
  /** a_rect: page-space quad x, y, w, h (plane bounds incl. distance padding). */
  rectOffset: 0,
  /** a_slot: atlas slot x, y, w, h in texels (all fragments of a glyph share w, h). */
  slotOffset: 16,
  /** a_role: 0 ink · 1 ink2 · 2 accent · 3 highlight underlay. */
  roleOffset: 32,
  /** a_highlight: highlight color index 1–4 (role 3 only). */
  highlightOffset: 36,
  stride: 40,
} as const;

export const ROLE = { ink: 0, ink2: 1, accent: 2, highlight: 3 } as const;

export const COVERAGE_VERT = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_slot;
layout(location = 2) in uint a_role;
layout(location = 3) in float a_highlight;

// Page space (points) → clip space. Zoom and pan only ever change this.
uniform mat3 u_camera;

out vec2 v_local;            // slot-local texel coordinates, 0 … slot size
flat out vec4 v_slot;
flat out uint v_role;
flat out float v_highlight;

void main() {
  // TRIANGLE_STRIP corners from gl_VertexID: (0,0) (1,0) (0,1) (1,1). No vertex buffer.
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
  vec2 p = a_rect.xy + corner * a_rect.zw;
  // Slot-LOCAL coordinates: every fragment of a glyph has the same rect and
  // slot size, so it interpolates bit-identical values wherever its slot is.
  v_local = corner * a_slot.zw;
  v_slot = a_slot;
  v_role = a_role;
  v_highlight = a_highlight;
  vec3 clip = u_camera * vec3(p, 1.0);
  gl_Position = vec4(clip.xy, 0.0, 1.0);
}
`;

export const COVERAGE_FRAG = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

in vec2 v_local;
flat in vec4 v_slot;
flat in uint v_role;
flat in float v_highlight;

uniform sampler2D u_atlas;     // shredded MSDF fragments (RGB distance); texelFetch only
uniform float u_pxRange;       // distance range baked into the atlas, in atlas texels
uniform float u_distRangePx;   // encoding range in screen px (DIST_RANGE_PX)

out vec4 o_enc;

float median3(vec3 v) {
  return max(min(v.r, v.g), min(max(v.r, v.g), v.b));
}

// Bilinear reconstruction done here, not by the sampler, from slot-local
// coordinates and clamped to the slot:
// - the weights depend only on v_local, so every fragment of a glyph is
//   sampled at exactly the same sub-texel position whatever its place in the
//   atlas, and the MAX union is exact on any GPU (hardware filtering rounds
//   absolute texel coordinates, so fragments in different slots would differ
//   in the last bit, and 1 LSB of Pass 1 shows at dark ink edges);
// - the footprint never leaves the slot, so neighboring slots cannot bleed in;
// - weights are full float, not the 8-bit sub-texel weights of fixed-function
//   filtering.
vec3 sampleSlot() {
  vec2 p = v_local - 0.5;
  vec2 cell = floor(p);
  vec2 f = p - cell;
  ivec2 lo = ivec2(v_slot.xy);
  ivec2 hi = lo + ivec2(v_slot.zw) - 1;
  ivec2 a = clamp(lo + ivec2(cell), lo, hi);
  ivec2 b = clamp(lo + ivec2(cell) + 1, lo, hi);
  vec3 t00 = texelFetch(u_atlas, a, 0).rgb;
  vec3 t10 = texelFetch(u_atlas, ivec2(b.x, a.y), 0).rgb;
  vec3 t01 = texelFetch(u_atlas, ivec2(a.x, b.y), 0).rgb;
  vec3 t11 = texelFetch(u_atlas, b, 0).rgb;
  return mix(mix(t00, t10, f.x), mix(t01, t11, f.x), f.y);
}

void main() {
  if (v_role == 3u) {
    // Highlight underlay quad: write only the highlight channel (MAX-blended).
    o_enc = vec4(0.0, 0.0, 0.0, v_highlight * 0.25);
    return;
  }
  // Screen-space pixel range of the distance field (msdfgen's formulation):
  // how many screen pixels one unit of normalized distance spans here.
  vec2 texelsPerPx = max(fwidth(v_local), vec2(1e-6));
  float screenPxRange = max(0.5 * u_pxRange * (1.0 / texelsPerPx.x + 1.0 / texelsPerPx.y), 1.0);

  float sdPx = (median3(sampleSlot()) - 0.5) * screenPxRange;
  float enc = clamp(sdPx / u_distRangePx + 0.5, 0.0, 1.0);

  // Route to the role's channel. Every other channel gets 0 (= far outside),
  // which is the identity for MAX blending.
  o_enc = enc * vec4(equal(uvec4(v_role), uvec4(0u, 1u, 2u, 99u)));
}
`;

export const COMPOSITE_VERT = /* glsl */ `#version 300 es
precision highp float;

out vec2 v_uv;

void main() {
  // Single full-screen triangle: (-1,-1) (3,-1) (-1,3).
  vec2 p = vec2(float((gl_VertexID & 1) << 2) - 1.0, float((gl_VertexID & 2) << 1) - 1.0);
  v_uv = p * 0.5 + 0.5;
  gl_Position = vec4(p, 0.0, 1.0);
}
`;

export const COMPOSITE_FRAG = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

in vec2 v_uv;

uniform sampler2D u_cov;        // Pass 1: R ink · G ink2 · B accent (encoded distance) · A highlight/4
uniform sampler2D u_img;        // Pass 2: images, premultiplied LINEAR (1×1 transparent when none)

uniform vec3 u_paper;           // LINEAR sRGB theme colors
uniform vec3 u_ink;
uniform vec3 u_ink2;
uniform vec3 u_accent;
uniform vec3 u_highlight[4];

uniform float u_distRangePx;    // must equal Pass 1
uniform float u_weightPx;       // optical weight compensation: edge offset in screen px (+ bolder, − thinner)
uniform float u_covGamma;       // coverage curve per theme (< 1 sturdier dark-on-light, > 1 crisper light-on-dark)
uniform vec3 u_warmGain;        // white-balance gains for the warmth setting (1,1,1 = neutral)
uniform float u_lumaCeil;       // anti-glare: maximum output luminance (linear Y), 1.0 = off
uniform float u_dim;            // extra-dim below the OS brightness floor, 0 … 0.6

out vec4 o_color;

vec3 linearToSrgb(vec3 c) {
  c = clamp(c, 0.0, 1.0);
  vec3 lo = c * 12.92;
  vec3 hi = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
  return mix(lo, hi, step(vec3(0.0031308), c));
}

// Decoded signed distance → coverage. The weight shifts the edge before the
// 1 px analytic AA ramp, which is the screen-space equivalent of moving the
// MSDF threshold, and it fades naturally at large zoom.
float coverage(float enc) {
  float d = (enc - 0.5) * u_distRangePx;
  float c = clamp(d + u_weightPx + 0.5, 0.0, 1.0);
  return pow(c, u_covGamma);
}

void main() {
  vec4 e = texture(u_cov, v_uv);
  vec4 img = texture(u_img, v_uv);

  // Paper, then images (premultiplied over paper).
  vec3 c = u_paper * (1.0 - img.a) + img.rgb;

  // Highlight underlay sits beneath the ink, so ink contrast is preserved.
  int h = int(e.a * 4.0 + 0.5);
  if (h > 0) c = u_highlight[clamp(h - 1, 0, 3)];

  // Ink roles, composited in linear light.
  c = mix(c, u_ink, coverage(e.r));
  c = mix(c, u_ink2, coverage(e.g));
  c = mix(c, u_accent, coverage(e.b));

  // Warmth: white-balance the whole page coherently (paper, ink, images).
  c *= u_warmGain;

  // Anti-glare ceiling: scale (not clip) any pixel brighter than the ceiling,
  // preserving hue, so a white diagram cannot flash-bang a night reader.
  float y = dot(c, vec3(0.2126, 0.7152, 0.0722));
  c *= min(1.0, u_lumaCeil / max(y, 1e-5));

  // Extra-dim below the OS minimum brightness.
  c *= 1.0 - u_dim;

  o_color = vec4(linearToSrgb(c), 1.0);
}
`;
