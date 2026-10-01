/**
 * Lumen's two text passes (Pass 2, images, plugs in as `u_img`).
 *
 *   Pass 1: instanced glyph-fragment quads → RGBA8 coverage target, MAX blend
 *   Pass 3: full-screen composite → default framebuffer (sRGB out)
 *
 * Render-on-demand: callers invoke `render()` only when the camera, theme or
 * content changes. Idle frames cost nothing.
 */
import type { LumenContext } from "./context.ts";
import { createProgram, type Program } from "./program.ts";
import { COMPOSITE_FRAG, COMPOSITE_VERT, COVERAGE_FRAG, COVERAGE_VERT, DIST_RANGE_PX, GLYPH_INSTANCE } from "./shaders.ts";
import { warmGain, type ThemeUniforms } from "./theme.ts";

type CoverageUniform = "u_camera" | "u_atlas" | "u_pxRange" | "u_distRangePx";
type CompositeUniform =
  | "u_cov"
  | "u_img"
  | "u_paper"
  | "u_ink"
  | "u_ink2"
  | "u_accent"
  | "u_highlight"
  | "u_distRangePx"
  | "u_weightPx"
  | "u_covGamma"
  | "u_warmGain"
  | "u_lumaCeil"
  | "u_dim";

export interface AtlasImage {
  readonly width: number;
  readonly height: number;
  /** RGBA8, rows top-to-bottom. RGB = MSDF channels. Never lossy-compressed. */
  readonly data: Uint8Array;
  /** Distance range baked by the atlas generator, in atlas texels. */
  readonly pxRange: number;
}

export type BlendMode = "max" | "add";

export class LumenRenderer {
  readonly #gl: WebGL2RenderingContext;
  readonly #coverage: Program<CoverageUniform>;
  readonly #composite: Program<CompositeUniform>;
  readonly #vao: WebGLVertexArrayObject;
  readonly #instances: WebGLBuffer;
  readonly #emptyImage: WebGLTexture;
  #atlas: WebGLTexture | null = null;
  #pxRange = 1;
  #covTex: WebGLTexture | null = null;
  #covFbo: WebGLFramebuffer | null = null;
  #width = 0;
  #height = 0;
  #count = 0;
  #camera: Float32Array = new Float32Array([1, 0, 0, 0, 1, 0, 0, 0, 1]);
  #theme: ThemeUniforms | null = null;

  constructor(ctx: LumenContext) {
    const gl = ctx.gl;
    this.#gl = gl;
    this.#coverage = createProgram(gl, "coverage", COVERAGE_VERT, COVERAGE_FRAG, [
      "u_camera",
      "u_atlas",
      "u_pxRange",
      "u_distRangePx",
    ]);
    this.#composite = createProgram(gl, "composite", COMPOSITE_VERT, COMPOSITE_FRAG, [
      "u_cov",
      "u_img",
      "u_paper",
      "u_ink",
      "u_ink2",
      "u_accent",
      "u_highlight",
      "u_distRangePx",
      "u_weightPx",
      "u_covGamma",
      "u_warmGain",
      "u_lumaCeil",
      "u_dim",
    ]);

    const vao = gl.createVertexArray();
    const buf = gl.createBuffer();
    const empty = gl.createTexture();
    if (vao === null || buf === null || empty === null) throw new Error("LumenRenderer: GL allocation failed");
    this.#vao = vao;
    this.#instances = buf;
    this.#emptyImage = empty;

    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    const s = GLYPH_INSTANCE.stride;
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, s, GLYPH_INSTANCE.rectOffset);
    gl.vertexAttribDivisor(0, 1);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, s, GLYPH_INSTANCE.uvOffset);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribIPointer(2, 1, gl.UNSIGNED_INT, s, GLYPH_INSTANCE.roleOffset);
    gl.vertexAttribDivisor(2, 1);
    gl.enableVertexAttribArray(3);
    gl.vertexAttribPointer(3, 1, gl.FLOAT, false, s, GLYPH_INSTANCE.highlightOffset);
    gl.vertexAttribDivisor(3, 1);
    gl.bindVertexArray(null);

    gl.bindTexture(gl.TEXTURE_2D, empty);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
    gl.bindTexture(gl.TEXTURE_2D, null);
  }

  /** Drawing-buffer size in device pixels. Reallocates the coverage target. */
  resize(width: number, height: number): void {
    const gl = this.#gl;
    if (width === this.#width && height === this.#height) return;
    this.#width = width;
    this.#height = height;
    if (this.#covTex !== null) gl.deleteTexture(this.#covTex);
    if (this.#covFbo !== null) gl.deleteFramebuffer(this.#covFbo);
    const tex = gl.createTexture();
    const fbo = gl.createFramebuffer();
    if (tex === null || fbo === null) throw new Error("LumenRenderer: coverage target allocation failed");
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, width, height);
    // 1:1 with the drawing buffer: NEAREST reads exact per-pixel distances.
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    const status = gl.checkFramebufferStatus(gl.FRAMEBUFFER);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    if (status !== gl.FRAMEBUFFER_COMPLETE) throw new Error(`coverage framebuffer incomplete: 0x${status.toString(16)}`);
    this.#covTex = tex;
    this.#covFbo = fbo;
  }

  setAtlas(atlas: AtlasImage): void {
    const gl = this.#gl;
    if (atlas.data.length !== atlas.width * atlas.height * 4) throw new Error("atlas: data size mismatch");
    if (this.#atlas !== null) gl.deleteTexture(this.#atlas);
    const tex = gl.createTexture();
    if (tex === null) throw new Error("atlas: createTexture failed");
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, atlas.width, atlas.height, 0, gl.RGBA, gl.UNSIGNED_BYTE, atlas.data);
    // MSDF requires bilinear reconstruction of the distance field, never mipmaps
    // (minified MSDF is handled by screenPxRange, not by prefiltering).
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D, null);
    this.#atlas = tex;
    this.#pxRange = atlas.pxRange;
  }

  /** Instance data in GLYPH_INSTANCE layout (from the Compositor). */
  setInstances(data: ArrayBuffer | ArrayBufferView, count: number): void {
    const gl = this.#gl;
    const bytes = ArrayBuffer.isView(data) ? data.byteLength : data.byteLength;
    if (bytes < count * GLYPH_INSTANCE.stride) throw new Error("instances: buffer smaller than count × stride");
    gl.bindBuffer(gl.ARRAY_BUFFER, this.#instances);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    this.#count = count;
  }

  /** Column-major mat3: page space → clip space. */
  setCamera(m: Float32Array): void {
    this.#camera = m;
  }

  setTheme(theme: ThemeUniforms): void {
    this.#theme = theme;
  }

  /** Pass 1. `blend` is "max" in production; "add" exists for the seam test. */
  renderCoverage(blend: BlendMode = "max"): void {
    const gl = this.#gl;
    if (this.#covFbo === null || this.#atlas === null) throw new Error("render: resize() and setAtlas() first");
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.#covFbo);
    gl.viewport(0, 0, this.#width, this.#height);
    gl.clearColor(0, 0, 0, 0); // 0 = −2 px = "far outside" in every channel
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.enable(gl.BLEND);
    if (blend === "max") {
      gl.blendEquation(gl.MAX);
    } else {
      gl.blendEquation(gl.FUNC_ADD);
      gl.blendFunc(gl.ONE, gl.ONE);
    }
    gl.useProgram(this.#coverage.program);
    gl.uniformMatrix3fv(this.#coverage.uniforms.u_camera, false, this.#camera);
    gl.uniform1f(this.#coverage.uniforms.u_pxRange, this.#pxRange);
    gl.uniform1f(this.#coverage.uniforms.u_distRangePx, DIST_RANGE_PX);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.#atlas);
    gl.uniform1i(this.#coverage.uniforms.u_atlas, 0);
    gl.bindVertexArray(this.#vao);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.#count);
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  }

  /** Pass 3 to the default framebuffer. */
  renderComposite(images: WebGLTexture | null = null): void {
    const gl = this.#gl;
    const t = this.#theme;
    if (t === null || this.#covTex === null) throw new Error("render: setTheme() first");
    const u = this.#composite.uniforms;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, this.#width, this.#height);
    gl.useProgram(this.#composite.program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.#covTex);
    gl.uniform1i(u.u_cov, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, images ?? this.#emptyImage);
    gl.uniform1i(u.u_img, 1);
    gl.uniform3fv(u.u_paper, t.paper);
    gl.uniform3fv(u.u_ink, t.ink);
    gl.uniform3fv(u.u_ink2, t.ink2);
    gl.uniform3fv(u.u_accent, t.accent);
    gl.uniform3fv(u.u_highlight, t.highlights.flat());
    gl.uniform1f(u.u_distRangePx, DIST_RANGE_PX);
    gl.uniform1f(u.u_weightPx, t.weightPx);
    gl.uniform1f(u.u_covGamma, t.covGamma);
    gl.uniform3fv(u.u_warmGain, warmGain(t.warmth));
    gl.uniform1f(u.u_lumaCeil, t.lumaCeil);
    gl.uniform1f(u.u_dim, t.dim);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  render(): void {
    this.renderCoverage("max");
    this.renderComposite();
  }

  dispose(): void {
    const gl = this.#gl;
    gl.deleteProgram(this.#coverage.program);
    gl.deleteProgram(this.#composite.program);
    gl.deleteVertexArray(this.#vao);
    gl.deleteBuffer(this.#instances);
    gl.deleteTexture(this.#emptyImage);
    if (this.#atlas !== null) gl.deleteTexture(this.#atlas);
    if (this.#covTex !== null) gl.deleteTexture(this.#covTex);
    if (this.#covFbo !== null) gl.deleteFramebuffer(this.#covFbo);
  }
}

/** Orthographic page → clip camera: `scale` device px per page unit, `offset` in device px. */
export function orthoCamera(width: number, height: number, scale: number, offsetX: number, offsetY: number): Float32Array {
  const sx = (2 * scale) / width;
  const sy = (-2 * scale) / height; // page y grows downward
  return new Float32Array([sx, 0, 0, 0, sy, 0, (2 * offsetX) / width - 1, 1 - (2 * offsetY) / height, 1]);
}
