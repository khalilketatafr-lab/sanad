/**
 * Lumen WebGL2 context: creation, capability gating, and loss/restore.
 *
 * Context attributes are part of the security and UX contract:
 *  - preserveDrawingBuffer: false. The drawing buffer is cleared after every
 *    composite, so readPixels/toDataURL/drawImage from outside our own frame
 *    yields an empty buffer rather than the page (blueprint 03 §4.7). Verified
 *    at runtime, because some drivers ignore attributes.
 *  - antialias: false. Glyph anti-aliasing is analytic in the shaders (MSDF
 *    distance → 1 px ramp), and MSAA would only add bandwidth.
 *  - alpha: false. The page is opaque paper, which lets the compositor skip
 *    blending the canvas with the page behind it.
 *  - depth/stencil: false. Lumen draws 2D passes only.
 */

export type LumenSurface = HTMLCanvasElement | OffscreenCanvas;

export interface LumenCapabilities {
  readonly maxTextureSize: number;
  readonly maxRenderbufferSize: number;
  readonly maxViewport: readonly [number, number];
  /** Renderer string when the browser exposes it (WEBGL_debug_renderer_info). */
  readonly renderer: string;
  /** True for software rasterizers (SwiftShader, llvmpipe), used for device tiering. */
  readonly software: boolean;
  /** RGBA16F render targets with blending (EXT_color_buffer_float). Optional. */
  readonly floatRenderTargets: boolean;
  /** Linear filtering of float textures (OES_texture_float_linear). Optional. */
  readonly floatLinear: boolean;
  /** Parallel shader compile (KHR_parallel_shader_compile): non-blocking link status. */
  readonly parallelCompile: boolean;
}

export type ContextState = "live" | "lost";

export interface LumenContextOptions {
  /**
   * Fail instead of running on a software rasterizer. Tier detection first
   * tries `true` and falls back to `false` (blueprint 03 §9, Tier C).
   */
  readonly failIfMajorPerformanceCaveat?: boolean;
  readonly powerPreference?: WebGLPowerPreference;
  /** Called after loss. All GL objects are invalid; stop issuing calls. */
  readonly onLost?: () => void;
  /** Called after restore with fresh state. Rebuild everything from cached ciphertext. */
  readonly onRestored?: (gl: WebGL2RenderingContext) => void;
}

export interface LumenContext {
  readonly gl: WebGL2RenderingContext;
  readonly caps: LumenCapabilities;
  readonly state: ContextState;
  /** Test hook: simulate loss/restore via WEBGL_lose_context. */
  simulateLoss(): boolean;
  simulateRestore(): boolean;
  dispose(): void;
}

export class LumenContextError extends Error {
  override readonly name = "LumenContextError";
  readonly reason: "unavailable" | "attributes" | "capabilities";

  constructor(reason: LumenContextError["reason"], message: string) {
    super(message);
    this.reason = reason;
  }
}

/** Minimums Lumen relies on: 4096² atlases and render targets the size of a 4K spread. */
const REQUIRED_TEXTURE_SIZE = 4096;
const REQUIRED_RENDERBUFFER_SIZE = 4096;

const SOFTWARE_RENDERERS = /swiftshader|llvmpipe|softpipe|software|basic render driver/iu;

function contextAttributes(opts: LumenContextOptions): WebGLContextAttributes {
  return {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    premultipliedAlpha: true,
    preserveDrawingBuffer: false,
    desynchronized: false,
    failIfMajorPerformanceCaveat: opts.failIfMajorPerformanceCaveat ?? false,
    powerPreference: opts.powerPreference ?? "default",
  };
}

function readCapabilities(gl: WebGL2RenderingContext): LumenCapabilities {
  const debug = gl.getExtension("WEBGL_debug_renderer_info");
  const renderer = debug
    ? String(gl.getParameter(debug.UNMASKED_RENDERER_WEBGL))
    : String(gl.getParameter(gl.RENDERER));
  const viewport = gl.getParameter(gl.MAX_VIEWPORT_DIMS) as Int32Array;
  return {
    maxTextureSize: gl.getParameter(gl.MAX_TEXTURE_SIZE) as number,
    maxRenderbufferSize: gl.getParameter(gl.MAX_RENDERBUFFER_SIZE) as number,
    maxViewport: [viewport[0] ?? 0, viewport[1] ?? 0],
    renderer,
    software: SOFTWARE_RENDERERS.test(renderer),
    floatRenderTargets: gl.getExtension("EXT_color_buffer_float") !== null,
    floatLinear: gl.getExtension("OES_texture_float_linear") !== null,
    parallelCompile: gl.getExtension("KHR_parallel_shader_compile") !== null,
  };
}

function assertContract(gl: WebGL2RenderingContext, caps: LumenCapabilities): void {
  const attrs = gl.getContextAttributes();
  if (attrs === null) throw new LumenContextError("unavailable", "context lost during creation");
  if (attrs.preserveDrawingBuffer) {
    // P1 hygiene: never run with a readable persistent drawing buffer.
    throw new LumenContextError("attributes", "driver forced preserveDrawingBuffer=true");
  }
  if (attrs.alpha) throw new LumenContextError("attributes", "driver forced an alpha drawing buffer");
  if (caps.maxTextureSize < REQUIRED_TEXTURE_SIZE || caps.maxRenderbufferSize < REQUIRED_RENDERBUFFER_SIZE) {
    throw new LumenContextError(
      "capabilities",
      `GPU limits too small: texture ${caps.maxTextureSize}, renderbuffer ${caps.maxRenderbufferSize} (need ${REQUIRED_TEXTURE_SIZE})`,
    );
  }
}

/**
 * Creates the WebGL2 context Lumen renders into. Works on an
 * `OffscreenCanvas` inside the Render Worker (preferred) or on a main-thread
 * `<canvas>` (fallback for engines without WebGL in OffscreenCanvas).
 */
export function createLumenContext(surface: LumenSurface, opts: LumenContextOptions = {}): LumenContext {
  const gl = surface.getContext("webgl2", contextAttributes(opts)) as WebGL2RenderingContext | null;
  if (gl === null) {
    throw new LumenContextError("unavailable", "WebGL2 unavailable (or blocked by failIfMajorPerformanceCaveat)");
  }
  let caps = readCapabilities(gl);
  assertContract(gl, caps);

  let state: ContextState = "live";
  const loseExt = gl.getExtension("WEBGL_lose_context");

  const onLost = (event: Event): void => {
    // preventDefault() is what allows the browser to restore the context later.
    event.preventDefault();
    state = "lost";
    opts.onLost?.();
  };
  const onRestored = (): void => {
    caps = readCapabilities(gl);
    state = "live";
    opts.onRestored?.(gl);
  };
  surface.addEventListener("webglcontextlost", onLost);
  surface.addEventListener("webglcontextrestored", onRestored);

  return {
    gl,
    get caps() {
      return caps;
    },
    get state() {
      return state;
    },
    simulateLoss: () => {
      if (loseExt === null) return false;
      loseExt.loseContext();
      return true;
    },
    simulateRestore: () => {
      if (loseExt === null) return false;
      loseExt.restoreContext();
      return true;
    },
    dispose: () => {
      surface.removeEventListener("webglcontextlost", onLost);
      surface.removeEventListener("webglcontextrestored", onRestored);
    },
  };
}

/**
 * Device tier probe (blueprint 03 §9): prefer a hardware context, fall back
 * to software rendering, and report which one we got.
 */
export function createTieredContext(
  makeSurface: () => LumenSurface,
  opts: Omit<LumenContextOptions, "failIfMajorPerformanceCaveat"> = {},
): LumenContext {
  try {
    return createLumenContext(makeSurface(), { ...opts, failIfMajorPerformanceCaveat: true });
  } catch (err) {
    if (err instanceof LumenContextError && err.reason === "unavailable") {
      return createLumenContext(makeSurface(), { ...opts, failIfMajorPerformanceCaveat: false });
    }
    throw err;
  }
}
