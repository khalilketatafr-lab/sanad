/** Shader compilation with actionable diagnostics. */

export class ShaderError extends Error {
  override readonly name = "ShaderError";
}

function compile(gl: WebGL2RenderingContext, type: GLenum, source: string, label: string): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) throw new ShaderError(`${label}: createShader failed (context lost?)`);
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!(gl.getShaderParameter(shader, gl.COMPILE_STATUS) as boolean) && !gl.isContextLost()) {
    const log = gl.getShaderInfoLog(shader) ?? "";
    gl.deleteShader(shader);
    const numbered = source
      .split("\n")
      .map((l, i) => `${String(i + 1).padStart(3)}| ${l}`)
      .join("\n");
    throw new ShaderError(`${label} compile failed:\n${log}\n${numbered}`);
  }
  return shader;
}

export interface Program<U extends string> {
  readonly program: WebGLProgram;
  readonly uniforms: Readonly<Record<U, WebGLUniformLocation | null>>;
}

export function createProgram<U extends string>(
  gl: WebGL2RenderingContext,
  label: string,
  vertex: string,
  fragment: string,
  uniforms: readonly U[],
): Program<U> {
  const vs = compile(gl, gl.VERTEX_SHADER, vertex, `${label}.vert`);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fragment, `${label}.frag`);
  const program = gl.createProgram();
  if (program === null) throw new ShaderError(`${label}: createProgram failed`);
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!(gl.getProgramParameter(program, gl.LINK_STATUS) as boolean) && !gl.isContextLost()) {
    throw new ShaderError(`${label} link failed: ${gl.getProgramInfoLog(program) ?? ""}`);
  }
  const locations = {} as Record<U, WebGLUniformLocation | null>;
  for (const name of uniforms) locations[name] = gl.getUniformLocation(program, name);
  return { program, uniforms: locations };
}
