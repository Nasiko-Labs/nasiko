/**
 * The login showcase's backdrop (nasiko-login-gradient, 2026-11-12): a warm gold-and-espresso field seen through
 * fluted glass. Each 52 px flute is a lens over a squeezed strip of the field; the pointer bends the flutes near it,
 * and sweeping across them sets the slats swinging on a spring that ripples to their neighbours. WebGL 1, one
 * full-screen triangle. Decorative: under reduced motion the field is drawn once and redrawn only as the pointer moves.
 */
const FLUTE_PX = 52
/** Flutes the shader tracks (a 2496 px panel); flutes past it never swing. */
const MAX_FLUTES = 48
/** How far, in px, a flute's seams move at a full swing. */
const SWING_PX = 14

const VERTEX = `
attribute vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`

const FRAGMENT = `
precision highp float;
uniform vec2 uRes;
uniform float uTime;
uniform vec2 uPtr;
uniform float uOn;
uniform float uFlute;
uniform float uSwing[${MAX_FLUTES}];

// The palette (2026-11-10 board): Black #151515, Espresso #2A1A1A, Gold #C1993C, and lighter and darker golds and
// browns made from them. Nothing else: no off-white or grey (user request 2026-11-12).
const vec3 BLACK = vec3(0.082, 0.082, 0.082);
const vec3 ESPRESSO = vec3(0.165, 0.102, 0.102);
const vec3 GOLD = vec3(0.757, 0.600, 0.235);
const vec3 BAND = mix(BLACK, ESPRESSO, 0.4) * 0.85;
const vec3 LIT = vec3(0.88, 0.73, 0.40);
const vec3 TOP = vec3(0.384, 0.272, 0.133);
const vec3 CRESCENT = vec3(0.80, 0.58, 0.18);
const vec3 GROUND = vec3(0.42, 0.22, 0.11);
const vec3 FOOT = mix(ESPRESSO, BLACK, 0.4);

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }

// A soft ellipse at c, radii r, turned by a (radians): 1 at its centre, falling off like a gaussian.
float blob(vec2 p, vec2 c, vec2 r, float a) {
  vec2 d = p - c;
  d = vec2(cos(a) * d.x + sin(a) * d.y, -sin(a) * d.x + cos(a) * d.y) / r;
  return exp(-dot(d, d));
}

vec3 field(vec2 p) {
  float t = uTime;
  vec2 m = vec2(sin(t * 0.9), cos(t * 0.7)) * 0.03; // the slow drift
  vec3 c = GROUND;
  c = mix(c, CRESCENT, blob(p, vec2(0.58, 0.64) + m, vec2(0.58, 0.19), -0.5));
  c = mix(c, CRESCENT, blob(p, vec2(0.96, 0.44) - m, vec2(0.16, 0.22), 0.0) * 0.8);
  c = mix(c, LIT, blob(p, vec2(0.66, 0.39) + m * 1.4, vec2(0.42, 0.19), -0.5));
  // Above the band: the dim top, lit toward the top right.
  float b = p.y + 0.4 * p.x + 0.025 * sin(p.x * 5.0 + t * 1.3) - m.y;
  c = mix(c, TOP, 1.0 - smoothstep(0.2, 0.36, b));
  c = mix(c, LIT * 0.85, blob(p, vec2(0.92, 0.16) - m, vec2(0.3, 0.13), -0.5) * (1.0 - smoothstep(0.32, 0.44, b)));
  // The dark band itself, widest and darkest toward the left.
  c = mix(c, BAND, exp(-pow((b - 0.42) / (0.085 + 0.03 * (1.0 - p.x)), 2.0)));
  // Dark flanks and foot.
  c = mix(c, FOOT, smoothstep(0.82, 1.08, p.x) * smoothstep(0.2, 0.75, p.y) * 0.9);
  c = mix(c, FOOT, smoothstep(0.8, 1.1, p.y) * 0.6);
  return mix(c, FOOT, smoothstep(0.12, -0.04, p.x) * 0.55);
}

// A flute's swing (a constant-index loop: GLSL ES 1.0 fragment shaders can't index a uniform array directly).
float swingAt(float c) {
  float s = 0.0;
  for (int i = 0; i < ${MAX_FLUTES}; i++) {
    if (float(i) == c) s = uSwing[i];
  }
  return s;
}

// Where flute c's left seam sits once it has swung: the glass slat itself moves.
float seamAt(float c) { return c * uFlute + swingAt(c) * ${SWING_PX.toFixed(1)}; }

void main() {
  vec2 uv = gl_FragCoord.xy / uRes;
  uv.y = 1.0 - uv.y;
  float aspect = uRes.x / uRes.y;
  float x = gl_FragCoord.x;
  // Which slat this pixel is behind, with every seam where its swing has moved it.
  float col = floor(x / uFlute);
  float seam = seamAt(col);
  if (x < seam) {
    col -= 1.0;
    seam = seamAt(col);
  } else if (x >= seamAt(col + 1.0)) {
    col += 1.0;
    seam = seamAt(col);
  }
  float width = seamAt(col + 1.0) - seam;
  float swing = swingAt(col);
  float px = x - seam;
  float f = px / width;
  // How near the pointer this flute is: a wide oval, so a few flutes either side join in.
  vec2 d = vec2((seam + width * 0.5) / uRes.x - uPtr.x, uv.y - uPtr.y);
  d.x *= aspect;
  float near = exp(-dot(d, d) / 0.03) * uOn;
  // Each flute shows a squeezed strip of what lies behind it, more so near the pointer or mid-swing. A swinging slat
  // carries its strip with it, the same distance its seams move, so glass and picture move as one.
  float bend = 0.012 + 0.11 * near + 0.08 * abs(swing);
  float drip = exp(-px / 2.5) * (0.05 + 0.05 * hash(vec2(col, 1.0)));
  vec2 s = vec2((x - swing * ${SWING_PX.toFixed(1)}) / uRes.x - (f - 0.5) * bend, uv.y - drip);
  vec3 c = field(s);
  // The seam: a thin shadow and a faint sheen just inside it.
  c *= mix(0.8, 1.0, smoothstep(0.0, 1.6, px));
  c += GOLD * 0.04 * exp(-pow((px - 2.2) / 1.2, 2.0));
  c += (hash(gl_FragCoord.xy + 0.5) - 0.5) * 0.05;
  gl_FragColor = vec4(c, 1.0);
}
`

function compile(gl: WebGLRenderingContext) {
  const program = gl.createProgram()
  if (!program) return null
  for (const [type, source] of [
    [gl.VERTEX_SHADER, VERTEX],
    [gl.FRAGMENT_SHADER, FRAGMENT],
  ] as const) {
    const shader = gl.createShader(type)
    if (!shader) return null
    gl.shaderSource(shader, source)
    gl.compileShader(shader)
    gl.attachShader(program, shader)
    gl.deleteShader(shader)
  }
  gl.linkProgram(program)
  return gl.getProgramParameter(program, gl.LINK_STATUS) ? program : null
}

/** Draws the field into `canvas` until the returned cleanup runs. No WebGL: the panel's own black shows instead. */
export function startLoginField(
  canvas: HTMLCanvasElement,
  still: boolean,
): (() => void) | undefined {
  const gl = canvas.getContext('webgl', { antialias: false, premultipliedAlpha: false })
  if (!gl) return
  const program = compile(gl)
  if (!program) return
  gl.useProgram(program)
  const triangle = gl.createBuffer()
  gl.bindBuffer(gl.ARRAY_BUFFER, triangle)
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW)
  const aPos = gl.getAttribLocation(program, 'aPos')
  gl.enableVertexAttribArray(aPos)
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0)
  const u = {
    res: gl.getUniformLocation(program, 'uRes'),
    time: gl.getUniformLocation(program, 'uTime'),
    ptr: gl.getUniformLocation(program, 'uPtr'),
    on: gl.getUniformLocation(program, 'uOn'),
    flute: gl.getUniformLocation(program, 'uFlute'),
    swing: gl.getUniformLocation(program, 'uSwing'),
  }
  // Where the pointer is (target) and where the shader sees it (eased toward the target).
  const target = { x: 0.5, y: 0.5, on: 0 }
  const shown = { x: 0.5, y: 0.5, on: 0 }
  const swing = new Float32Array(MAX_FLUTES)
  const velocity = new Float32Array(MAX_FLUTES)
  let lastFlute: number | null = null
  let lastX = 0
  let lastStamp = 0
  let frame = 0
  let lastFrame = 0
  let time = 0

  const resize = () => {
    const box = canvas.getBoundingClientRect()
    canvas.width = Math.max(1, Math.round(box.width))
    canvas.height = Math.max(1, Math.round(box.height))
    gl.viewport(0, 0, canvas.width, canvas.height)
  }
  // Each flute is a damped spring, coupled to its neighbours so a push ripples along the glass.
  const step = (dt: number) => {
    const n = Math.min(MAX_FLUTES, Math.ceil(canvas.width / FLUTE_PX))
    for (let i = 0; i < n; i++) {
      const c = swing[i] ?? 0
      const left = swing[i - 1] ?? c
      const right = i < n - 1 ? (swing[i + 1] ?? c) : c
      velocity[i] =
        (velocity[i] ?? 0) + (-55 * c - 3.2 * (velocity[i] ?? 0) + 30 * (left + right - 2 * c)) * dt
    }
    for (let i = 0; i < n; i++)
      swing[i] = Math.max(-1.4, Math.min(1.4, (swing[i] ?? 0) + (velocity[i] ?? 0) * dt))
  }
  const draw = (dt: number) => {
    if (!still) {
      time += dt * 0.05
      for (let left = dt; left > 0; left -= 1 / 120) step(Math.min(left, 1 / 120))
    }
    const ease = still ? 1 : 1 - Math.exp(-dt * 10)
    shown.x += (target.x - shown.x) * ease
    shown.y += (target.y - shown.y) * ease
    shown.on += (target.on - shown.on) * (still ? 1 : 1 - Math.exp(-dt * 4))
    if (canvas.width < 2) return
    gl.uniform2f(u.res, canvas.width, canvas.height)
    gl.uniform1f(u.time, time)
    gl.uniform2f(u.ptr, shown.x, shown.y)
    gl.uniform1f(u.on, shown.on)
    gl.uniform1f(u.flute, FLUTE_PX)
    gl.uniform1fv(u.swing, swing)
    gl.drawArrays(gl.TRIANGLES, 0, 3)
  }
  const tick = (now: number) => {
    const dt = lastFrame ? Math.min((now - lastFrame) / 1000, 0.05) : 0
    lastFrame = now
    draw(dt)
    frame = requestAnimationFrame(tick)
  }
  // The panel is inert (pointer-events-none): the page's pointer is read from window.
  const onMove = (e: PointerEvent) => {
    const box = canvas.getBoundingClientRect()
    if (!box.width) return
    const x = (e.clientX - box.left) / box.width
    const y = (e.clientY - box.top) / box.height
    const inside = x >= 0 && y >= 0 && x <= 1 && y <= 1
    // Entering: start the bend where the pointer is, not sliding in from where it left.
    if (inside && shown.on < 0.01) {
      shown.x = x
      shown.y = y
    }
    target.x = x
    target.y = y
    target.on = inside ? 1 : 0
    const px = x * box.width
    const flute = Math.floor(px / FLUTE_PX)
    // Crossing into another flute pushes every flute crossed, harder the faster the pointer moves.
    if (inside && !still && lastFlute !== null && flute !== lastFlute) {
      const speed = Math.abs(px - lastX) / Math.max(8, e.timeStamp - lastStamp)
      const dir = Math.sign(flute - lastFlute)
      const push = dir * 2.6 * Math.min(1, 0.25 + speed / 1.5)
      for (let i = lastFlute + dir; i !== flute + dir; i += dir)
        if (i >= 0 && i < MAX_FLUTES) velocity[i] = (velocity[i] ?? 0) + push * 4
    }
    lastFlute = inside ? flute : null
    lastX = px
    lastStamp = e.timeStamp
    if (still) draw(0)
  }
  const onLeave = () => {
    target.on = 0
    lastFlute = null
    if (still) draw(0)
  }

  resize()
  draw(0)
  if (!still) frame = requestAnimationFrame(tick)
  const observer =
    typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(() => {
          resize()
          draw(0)
        })
  observer?.observe(canvas)
  window.addEventListener('pointermove', onMove, { passive: true })
  document.documentElement.addEventListener('pointerleave', onLeave)
  return () => {
    cancelAnimationFrame(frame)
    observer?.disconnect()
    window.removeEventListener('pointermove', onMove)
    document.documentElement.removeEventListener('pointerleave', onLeave)
    gl.deleteBuffer(triangle)
    gl.deleteProgram(program)
  }
}
