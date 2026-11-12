<script setup lang="ts">
// The Pyro mascot: a red panda sitting in a particle fire.
// The fire is two canvases drawn in SVG units: flames behind the panda cover the box
// [-60, -80] .. [460, 440] of its 400 x 440 viewBox; embers in front fly over a much
// larger box, [-340, -260] .. [740, 560], out across the hero.
// The tag on the collar carries Povio's "p" (the glyph from the povio.com wordmark).
import { onBeforeUnmount, onMounted, ref } from 'vue';

const povioP =
  'M10.1738 5.44751C15.2203 5.44751 19.3309 9.75897 19.3477 15.0227C19.3475 20.3532 15.237 24.6809 10.1738 24.6809C7.46687 24.6642 4.94347 23.428 3.27246 21.3059V32.0002H1V15.0393C1 9.7589 5.11067 5.44757 10.1738 5.44751ZM10.1738 7.76977C6.36394 7.76984 3.27246 11.0289 3.27246 15.0393C3.27257 19.0998 6.36401 22.4084 10.1738 22.4084C13.967 22.4084 17.0584 19.0998 17.0752 15.0393C17.0752 11.0288 13.9838 7.76977 10.1738 7.76977Z';

const root = ref<HTMLDivElement>();
const glc = ref<HTMLCanvasElement>();
const back = ref<HTMLCanvasElement>();
const front = ref<HTMLCanvasElement>();

const BOX = { x: -60, y: -80, w: 520, h: 520 };
const EBOX = { x: -340, y: -260, w: 1080, h: 820 };
const CX = 200;

type Flame = { x: number; y: number; vx: number; vy: number; age: number; life: number; size: number; seed: number; axis: number };
type Spark = { x: number; y: number; vx: number; vy: number; age: number; life: number; size: number; seed: number; orbit: number; trail: number[] };

// Life palette: hot core -> orange -> red -> ember. Kept red-heavy.
const STOPS: [number, [number, number, number]][] = [
  [0.0, [255, 236, 190]],
  [0.12, [255, 170, 80]],
  [0.3, [255, 92, 38]],
  [0.55, [222, 32, 34]],
  [0.8, [140, 12, 20]],
  [1.0, [60, 6, 10]],
];
function palette(t: number): [number, number, number] {
  for (let i = 1; i < STOPS.length; i++) {
    const [t1, c1] = STOPS[i];
    const [t0, c0] = STOPS[i - 1];
    if (t <= t1) {
      const k = (t - t0) / (t1 - t0);
      return [0, 1, 2].map((j) => Math.round(c0[j] + (c1[j] - c0[j]) * k)) as [number, number, number];
    }
  }
  return STOPS[STOPS.length - 1][1];
}

const SPRITES = 32;
function makeSprites(): HTMLCanvasElement[] {
  const out: HTMLCanvasElement[] = [];
  for (let i = 0; i < SPRITES; i++) {
    const [r, g, b] = palette(i / (SPRITES - 1));
    const c = document.createElement('canvas');
    c.width = c.height = 64;
    const x = c.getContext('2d')!;
    const grad = x.createRadialGradient(32, 32, 0, 32, 32, 32);
    grad.addColorStop(0, `rgba(${r},${g},${b},1)`);
    grad.addColorStop(0.45, `rgba(${r},${g},${b},0.6)`);
    grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
    x.fillStyle = grad;
    x.fillRect(0, 0, 64, 64);
    out.push(c);
  }
  return out;
}

// Flame tongues: [x offset from the center, height factor, spawn weight].
const TONGUES: [number, number, number][] = [
  [-122, 0.95, 0.75],
  [-64, 1.3, 1.0],
  [0, 1.6, 1.25],
  [64, 1.35, 1.0],
  [122, 1.0, 0.75],
];
const TONGUE_WEIGHT = TONGUES.reduce((s, t) => s + t[2], 0);

// WebGL flames: turbulent noise shaping five tongues, drawn in the back box.
const FIRE_VS = `attribute vec2 p; void main() { gl_Position = vec4(p, 0.0, 1.0); }`;
const FIRE_FS = `
precision highp float;
uniform vec2 res;
uniform float time;
uniform float wind;
uniform float halo;
uniform float boost;
float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}
float fbm(vec2 p) {
  float v = 0.0, a = 0.5;
  mat2 m = mat2(1.6, 1.2, -1.2, 1.6);
  for (int i = 0; i < 5; i++) { v += a * noise(p); p = m * p; a *= 0.5; }
  return v;
}
// One flame: widest at the base, narrowing to a point at its height h.
float tongue(vec2 q, float c, float w, float h, float ph) {
  h *= (1.0 + 0.3 * boost) * (0.88 + 0.12 * sin(time * 2.3 + ph) * sin(time * 1.3 + ph * 2.0));
  float yy = q.y / h;
  if (yy >= 1.0 || yy < -0.2) return 0.0;
  float hw = w * pow(max(1.0 - yy, 0.0), 0.7) * (1.0 + 0.15 * sin(time * 3.1 + ph + q.y * 9.0));
  float dx = abs(q.x - c) / max(hw, 0.0005);
  return clamp(1.0 - dx * dx, 0.0, 1.0) * (1.0 - 0.55 * max(yy, 0.0));
}
float flames(vec2 q) {
  float f = tongue(q, 0.500, 0.300, 0.26, 0.4);
  f = max(f, tongue(q, 0.270, 0.085, 0.50, 0.0));
  f = max(f, tongue(q, 0.385, 0.095, 0.72, 1.7));
  f = max(f, tongue(q, 0.500, 0.110, 0.90, 3.1));
  f = max(f, tongue(q, 0.615, 0.095, 0.74, 4.4));
  f = max(f, tongue(q, 0.730, 0.085, 0.52, 5.9));
  f = max(f, tongue(q, 0.445, 0.050, 0.60, 2.3));
  f = max(f, tongue(q, 0.560, 0.050, 0.64, 5.1));
  return f;
}
vec3 ramp(float f) {
  vec3 c = mix(vec3(0.42, 0.02, 0.04), vec3(0.72, 0.04, 0.07), smoothstep(0.0, 0.2, f));
  c = mix(c, vec3(0.90, 0.13, 0.14), smoothstep(0.15, 0.4, f));
  c = mix(c, vec3(1.00, 0.36, 0.12), smoothstep(0.4, 0.65, f));
  c = mix(c, vec3(1.00, 0.62, 0.26), smoothstep(0.65, 0.85, f));
  return mix(c, vec3(1.00, 0.86, 0.58), smoothstep(0.88, 1.0, f));
}
void main() {
  vec2 uv = gl_FragCoord.xy / res;
  float y = uv.y - 0.07;
  float x = uv.x - wind * max(y, 0.0) * max(y, 0.0) * 0.35;
  float n1 = fbm(vec2(x * 5.0, y * 3.2 - time * 1.9));
  float n2 = fbm(vec2(x * 11.0 + 3.1, y * 7.0 - time * 3.3));
  x += (n1 - 0.5) * 0.15 * (0.2 + y * 1.4);
  float f = flames(vec2(x, y + (n1 - 0.5) * 0.05));
  // Licks: noise eats into the flame, more toward the tips.
  f += ((n2 - 0.5) * 0.55 * (0.3 + y * 1.2) + (n1 - 0.5) * 0.25) * smoothstep(0.0, 0.25, f);
  f = clamp(f, 0.0, 1.0) * smoothstep(-0.03, 0.05, y + (n1 - 0.5) * 0.04);
  float a = smoothstep(0.06, 0.2, f);
  vec4 fire = vec4(ramp(f) * a, a);
  float dx = (uv.x - 0.5) / 0.21;
  float g = halo * exp(-dx * dx) * smoothstep(0.85, 0.12, uv.y) * smoothstep(0.0, 0.1, uv.y) * (0.9 + 0.1 * sin(time * 2.7));
  vec4 glow = vec4(vec3(0.86, 0.12, 0.13) * g, g);
  gl_FragColor = fire + glow * (1.0 - fire.a);
}`;

type GlFire = { draw: (time: number, wind: number, dark: boolean, boost: number) => void; resize: () => void };
function makeGlFire(canvas: HTMLCanvasElement): GlFire | undefined {
  const gl = canvas.getContext('webgl', { premultipliedAlpha: true, alpha: true, antialias: false });
  if (!gl) return undefined;
  const shader = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (gl.getShaderParameter(s, gl.COMPILE_STATUS)) return s;
    console.warn('pyro fire shader:', gl.getShaderInfoLog(s));
    return undefined;
  };
  const vs = shader(gl.VERTEX_SHADER, FIRE_VS);
  const fs = shader(gl.FRAGMENT_SHADER, FIRE_FS);
  if (!vs || !fs) return undefined;
  const prog = gl.createProgram()!;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return undefined;
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const u = (n: string) => gl.getUniformLocation(prog, n);
  const uRes = u('res'), uTime = u('time'), uWind = u('wind'), uHalo = u('halo'), uBoost = u('boost');
  return {
    resize() {
      const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      canvas.width = Math.max(1, Math.round(canvas.clientWidth * dpr));
      canvas.height = Math.max(1, Math.round(canvas.clientHeight * dpr));
      gl.viewport(0, 0, canvas.width, canvas.height);
    },
    draw(time, wind, dark, boost) {
      gl.uniform1f(uBoost, boost);
      gl.uniform2f(uRes, canvas.width, canvas.height);
      gl.uniform1f(uTime, time);
      gl.uniform1f(uWind, wind / 120);
      gl.uniform1f(uHalo, dark ? 0.3 : 0.14);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    },
  };
}

const gauss = () => (Math.random() + Math.random() + Math.random() - 1.5) / 1.5;

let raf = 0;
let cleanup: (() => void) | undefined;

onMounted(() => {
  const el = root.value!;
  const bc = back.value!;
  const fc = front.value!;
  const bx = bc.getContext('2d')!;
  const fx = fc.getContext('2d')!;
  const sprites = makeSprites();
  const flames: Flame[] = [];
  const sparks: Spark[] = [];
  let scale = 1;
  let escale = 1;
  let wind = 0;
  let windTarget = 0;
  let flare = 0;
  let time = 0;
  let flameDebt = 0;
  let sparkDebt = 0;
  let last = performance.now();
  let running = false;
  let visible = true;
  let dark = document.documentElement.classList.contains('dark');
  const mo = new MutationObserver(() => { dark = document.documentElement.classList.contains('dark'); if (!running) draw(); });
  mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
  const glFire = makeGlFire(glc.value!);
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;

  function resize() {
    glFire?.resize();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    for (const c of [bc, fc]) {
      c.width = Math.max(1, Math.round(c.clientWidth * dpr));
      c.height = Math.max(1, Math.round(c.clientHeight * dpr));
    }
    scale = bc.width / BOX.w;
    escale = fc.width / EBOX.w;
  }

  function spawnFlame(burst = false) {
    let pick = Math.random() * TONGUE_WEIGHT;
    let tongue = TONGUES[0];
    for (const tg of TONGUES) { pick -= tg[2]; if (pick <= 0) { tongue = tg; break; } }
    const [off, height] = tongue;
    // Each tongue's axis sways on its own.
    const axis = CX + off + Math.sin(time * 1.7 + off) * 10;
    const x = axis + gauss() * 26;
    const edge = Math.abs(off) / 118;
    flames.push({
      x,
      y: 395 - Math.random() * 70 - edge * 30,
      vx: (Math.random() - 0.5) * 20,
      vy: -(130 + Math.random() * 100) * (burst ? 1.5 : 1),
      age: 0,
      life: (0.7 + Math.random() * 0.7) * height,
      size: (30 + Math.random() * 22) * (1.1 - edge * 0.35),
      seed: Math.random() * 1000,
      axis,
    });
  }

  // Embers: thrown off the flame tips, carried by swirling air; some circle the panda
  // (orbit > 0, the direction and strength), the rest drift off across the hero.
  function spawnSpark(burst = false) {
    const angle = -Math.PI / 2 + gauss() * (burst ? 1.6 : 1.1);
    const speed = (70 + Math.random() * 170) * (burst ? 1.6 : 1);
    sparks.push({
      x: CX + gauss() * 100,
      y: 130 + Math.random() * 190,
      vx: Math.cos(angle) * speed,
      vy: Math.sin(angle) * speed,
      age: 0,
      life: 2.2 + Math.random() * 3.6,
      size: 2.2 + Math.random() * 3.6,
      seed: Math.random() * 1000,
      orbit: Math.random() < 0.35 ? (Math.random() < 0.5 ? -1 : 1) * (0.6 + Math.random() * 0.6) : 0,
      trail: [],
    });
  }

  function step(dt: number) {
    time += dt;
    wind += (windTarget - wind) * Math.min(1, dt * 3);
    flare *= Math.exp(-dt * 2.2);
    flameDebt += dt * 430;
    while (flameDebt > 1) { if (!glFire) spawnFlame(); flameDebt--; }
    sparkDebt += dt * 16;
    while (sparkDebt > 1) { spawnSpark(); sparkDebt--; }

    for (let i = flames.length - 1; i >= 0; i--) {
      const p = flames[i];
      p.age += dt;
      if (p.age >= p.life) { flames.splice(i, 1); continue; }
      const t = p.age / p.life;
      p.vy -= 70 * dt; // buoyancy
      // Pulled to its tongue's axis, then toward the middle as it rises; licks sideways.
      p.vx += (Math.sin(time * 5 + p.seed) * 70 + wind * (0.4 + t) + (p.axis - p.x) * 2.2 + (CX - p.x) * t * 0.9) * dt;
      p.vx *= 1 - dt * 1.5;
      p.x += p.vx * dt;
      p.y += p.vy * dt;
    }
    for (let i = sparks.length - 1; i >= 0; i--) {
      const s = sparks[i];
      s.age += dt;
      if (s.age >= s.life) { sparks.splice(i, 1); continue; }
      s.trail.push(s.x, s.y);
      if (s.trail.length > 12) s.trail.splice(0, 2);
      // Swirling air: a cheap curl of sines over position and time.
      s.vx += (Math.sin(s.y * 0.018 + time * 1.4 + s.seed) * 120 + wind * 1.6) * dt;
      s.vy += (Math.cos(s.x * 0.016 + time * 1.1 + s.seed) * 90 - 18) * dt;
      if (s.orbit) {
        const dx = s.x - CX, dy = s.y - 250;
        const r = Math.hypot(dx, dy) || 1;
        const want = 175 + Math.sin(s.seed) * 35;
        s.vx += ((-dy / r) * 150 * s.orbit + (dx / r) * (want - r) * 0.9) * dt;
        s.vy += ((dx / r) * 150 * s.orbit + (dy / r) * (want - r) * 0.9) * dt;
      }
      s.vx *= 1 - dt * 0.55;
      s.vy *= 1 - dt * 0.55;
      s.x += s.vx * dt;
      s.y += s.vy * dt;
    }
  }

  const px = (x: number) => (x - BOX.x) * scale;
  const py = (y: number) => (y - BOX.y) * scale;
  const ex = (x: number) => (x - EBOX.x) * escale;
  const ey = (y: number) => (y - EBOX.y) * escale;

  function draw() {
    bx.clearRect(0, 0, bc.width, bc.height);
    if (glFire) glFire.draw(time, wind, dark, flare);
    fx.clearRect(0, 0, fc.width, fc.height);

    if (!glFire) {
    // Heat glow, pulsing.
    const pulse = 0.85 + Math.sin(time * 2.3) * 0.08 + Math.sin(time * 5.1) * 0.04;
    const glow = bx.createRadialGradient(px(CX), py(300), 0, px(CX), py(280), 158 * scale);
    glow.addColorStop(0, `rgba(255,70,40,${0.42 * pulse})`);
    glow.addColorStop(0.45, `rgba(220,31,38,${0.16 * pulse})`);
    glow.addColorStop(1, 'rgba(220,31,38,0)');
    bx.fillStyle = glow;
    bx.fillRect(0, 0, bc.width, bc.height);

    bx.globalCompositeOperation = dark ? 'lighter' : 'source-over';
    for (const p of flames) {
      const t = p.age / p.life;
      const size = p.size * Math.pow(1 - t, 0.42) * scale;
      if (size < 0.5) continue;
      bx.globalAlpha = Math.max(0, Math.min(1, (p.y + 60) / 110)) * (t < 0.08 ? t / 0.08 : 1) * (dark ? Math.min(1, (1 - t) * 1.25) * 0.58 : Math.pow(1 - t, 1.6) * 0.9);
      const s = sprites[Math.min(SPRITES - 1, Math.floor(t * SPRITES))];
      // Stretch upward a little: tongues rather than blobs.
      bx.drawImage(s, px(p.x) - size, py(p.y) - size * 1.35, size * 2, size * 2.7);
    }
    bx.globalAlpha = 1;
    bx.globalCompositeOperation = 'source-over';
    }

    fx.globalCompositeOperation = dark ? 'lighter' : 'source-over';
    fx.lineCap = 'round';
    for (const s of sparks) {
      const t = s.age / s.life;
      const fade = Math.min(1, s.age / 0.15) * (t > 0.6 ? (1 - t) / 0.4 : 1);
      const flicker = 0.6 + 0.4 * Math.sin(time * 18 + s.seed) * Math.sin(time * 7.3 + s.seed * 2);
      // Cools from orange to deep red; on a light page start hotter-red so it reads as fire, not pink.
      const heat = Math.min(1, (dark ? 0.06 : 0.3) + t * (dark ? 0.62 : 0.45));
      const [r, g, b] = palette(heat);
      // Trail.
      if (s.trail.length >= 4) {
        fx.globalAlpha = 0.35 * fade;
        fx.strokeStyle = `rgb(${r},${g},${b})`;
        fx.lineWidth = Math.max(0.6, s.size * 0.45 * escale);
        fx.beginPath();
        fx.moveTo(ex(s.trail[0]), ey(s.trail[1]));
        for (let k = 2; k < s.trail.length; k += 2) fx.lineTo(ex(s.trail[k]), ey(s.trail[k + 1]));
        fx.lineTo(ex(s.x), ey(s.y));
        fx.stroke();
      }
      // Glow, then a hot core.
      const glow = s.size * (dark ? 2.6 : 1.5) * escale;
      fx.globalAlpha = fade * flicker * (dark ? 0.75 : 0.4);
      fx.drawImage(sprites[Math.floor(heat * (SPRITES - 1))], ex(s.x) - glow, ey(s.y) - glow, glow * 2, glow * 2);
      const core = s.size * (dark ? 0.7 : 0.85) * escale;
      fx.globalAlpha = fade * flicker;
      fx.drawImage(sprites[Math.floor(heat * 0.4 * (SPRITES - 1))], ex(s.x) - core, ey(s.y) - core, core * 2, core * 2);
    }
    fx.globalAlpha = 1;
    fx.globalCompositeOperation = 'source-over';
  }

  function frame(now: number) {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    step(dt);
    draw();
    raf = requestAnimationFrame(frame);
  }
  function start() {
    if (running || reduced || !visible || document.hidden) return;
    running = true;
    last = performance.now();
    raf = requestAnimationFrame(frame);
  }
  function stop() {
    running = false;
    cancelAnimationFrame(raf);
  }

  function onMove(e: PointerEvent) {
    const r = el.getBoundingClientRect();
    const k = ((e.clientX - r.left) / r.width - 0.5) * 2; // -1 .. 1
    windTarget = -k * 120; // the flames lean away from the cursor
  }
  function onLeave() { windTarget = 0; }
  function onDown() {
    flare = 1;
    for (let i = 0; i < 70; i++) if (!glFire) spawnFlame(true);
    for (let i = 0; i < 60; i++) spawnSpark(true);
    if (reduced) { for (let i = 0; i < 20; i++) step(1 / 30); draw(); }
  }

  resize();
  const ro = new ResizeObserver(() => { resize(); if (!running) draw(); });
  ro.observe(el);
  const io = new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    visible ? start() : stop();
  });
  io.observe(el);
  const onVis = () => (document.hidden ? stop() : start());
  document.addEventListener('visibilitychange', onVis);
  el.addEventListener('pointermove', onMove);
  el.addEventListener('pointerleave', onLeave);
  el.addEventListener('pointerdown', onDown);

  if (reduced) {
    for (let i = 0; i < 90; i++) step(1 / 30); // a settled fire, drawn once
    draw();
  } else {
    for (let i = 0; i < 45; i++) step(1 / 30); // start already burning
    start();
  }

  cleanup = () => {
    stop();
    ro.disconnect();
    mo.disconnect();
    io.disconnect();
    document.removeEventListener('visibilitychange', onVis);
    el.removeEventListener('pointermove', onMove);
    el.removeEventListener('pointerleave', onLeave);
    el.removeEventListener('pointerdown', onDown);
  };
});

onBeforeUnmount(() => cleanup?.());
</script>

<template>
  <div ref="root" class="pyro-panda">
    <canvas ref="glc" class="fire back" aria-hidden="true" />
    <canvas ref="back" class="fire back" aria-hidden="true" />
    <svg viewBox="0 0 400 440" role="img" aria-label="Pyro, a red panda sitting in a fire">
      <defs>
        <linearGradient id="pp-fur" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="#e8622c" />
          <stop offset="100%" stop-color="#b9361a" />
        </linearGradient>
        <linearGradient id="pp-tail" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stop-color="#d24a22" />
          <stop offset="100%" stop-color="#a9301a" />
        </linearGradient>
        <radialGradient id="pp-shadow" cx="50%" cy="50%" r="50%">
          <stop offset="0%" stop-color="#000" stop-opacity="0.35" />
          <stop offset="100%" stop-color="#000" stop-opacity="0" />
        </radialGradient>
      </defs>

      <ellipse cx="200" cy="404" rx="130" ry="14" fill="url(#pp-shadow)" />

      <!-- Tail -->
      <g class="tail">
        <path d="M246 356 C318 366 370 322 362 260 C356 216 376 188 398 184" fill="none" stroke="url(#pp-tail)" stroke-width="50" stroke-linecap="round" />
        <path d="M246 356 C318 366 370 322 362 260 C356 216 376 188 398 184" fill="none" stroke="#5c160b" stroke-width="50" stroke-dasharray="13 21" stroke-dashoffset="-10" />
        <path d="M252 344 C312 350 352 316 346 262 C342 228 356 204 376 196" fill="none" stroke="#f08a52" stroke-width="7" stroke-linecap="round" opacity="0.28" />
      </g>

      <g class="panda">
        <!-- Body, arms, paws -->
        <ellipse cx="128" cy="394" rx="20" ry="9" fill="#2a0c06" />
        <ellipse cx="272" cy="394" rx="20" ry="9" fill="#2a0c06" />
        <path d="M200 250 C150 250 118 290 117 336 C116 378 150 402 200 402 C250 402 284 378 283 336 C282 290 250 250 200 250Z" fill="url(#pp-fur)" />
        <path d="M200 300 C172 300 157 330 159 362 C161 390 180 400 200 400 C220 400 239 390 241 362 C243 330 228 300 200 300Z" fill="#3a130b" />
        <path d="M168 298 C148 310 144 352 148 384 C151 398 177 400 181 386 C186 360 187 328 185 310 Z" fill="#2b0d07" />
        <path d="M232 298 C252 310 256 352 252 384 C249 398 223 400 219 386 C214 360 213 328 215 310 Z" fill="#2b0d07" />
        <ellipse cx="164" cy="393" rx="20" ry="9.5" fill="#1f0804" />
        <ellipse cx="236" cy="393" rx="20" ry="9.5" fill="#1f0804" />
        <ellipse cx="200" cy="268" rx="62" ry="12" fill="#000" opacity="0.2" />

        <!-- Head -->
        <g class="head">
          <path d="M136 178 C126 150 128 122 141 102 C160 111 178 128 184 148 Z" fill="#fff4ec" />
          <path d="M145 167 C139 149 141 131 147 118 C159 126 169 137 173 151 Z" fill="#341109" />
          <path d="M264 178 C274 150 272 122 259 102 C240 111 222 128 216 148 Z" fill="#fff4ec" />
          <path d="M255 167 C261 149 259 131 253 118 C241 126 231 137 227 151 Z" fill="#341109" />

          <path d="M128 214 C122 184 142 148 174 139 C190 134 210 134 226 139 C258 148 278 184 272 214 L286 226 L270 231 L277 246 L256 243 C242 257 222 264 200 264 C178 264 158 257 144 243 L123 246 L130 231 L114 226 Z" fill="url(#pp-fur)" />

          <path d="M126 228 C130 210 148 202 164 208 C178 214 180 234 168 243 C156 251 140 248 130 240 L118 236 L128 232 Z" fill="#fff4ec" />
          <path d="M274 228 C270 210 252 202 236 208 C222 214 220 234 232 243 C244 251 260 248 270 240 L282 236 L272 232 Z" fill="#fff4ec" />
          <ellipse cx="200" cy="236" rx="34" ry="24" fill="#fff4ec" />
          <ellipse cx="176" cy="180" rx="11.5" ry="6.5" fill="#fff4ec" transform="rotate(-12 176 180)" />
          <ellipse cx="224" cy="180" rx="11.5" ry="6.5" fill="#fff4ec" transform="rotate(12 224 180)" />
          <path d="M173 210 C169 220 167 230 171 242" fill="none" stroke="#8f2a12" stroke-width="10" stroke-linecap="round" opacity="0.45" />
          <path d="M227 210 C231 220 233 230 229 242" fill="none" stroke="#8f2a12" stroke-width="10" stroke-linecap="round" opacity="0.45" />

          <g class="eye">
            <circle cx="178" cy="203" r="10" fill="#140806" />
            <circle cx="182" cy="199" r="3.4" fill="#fff" />
            <circle cx="175" cy="207" r="1.4" fill="#fff" opacity="0.8" />
          </g>
          <g class="eye">
            <circle cx="222" cy="203" r="10" fill="#140806" />
            <circle cx="226" cy="199" r="3.4" fill="#fff" />
            <circle cx="219" cy="207" r="1.4" fill="#fff" opacity="0.8" />
          </g>

          <path d="M188 223 Q200 215 212 223 Q208 234 200 236 Q192 234 188 223Z" fill="#140806" />
          <ellipse cx="196" cy="222" rx="3.2" ry="1.6" fill="#fff" opacity="0.55" />
          <path d="M200 236 Q196 245 187 244 M200 236 Q204 245 213 244" fill="none" stroke="#140806" stroke-width="2.6" stroke-linecap="round" />
        </g>

        <!-- Collar with Povio's p on the tag -->
        <path d="M150 262 Q200 291 250 262" fill="none" stroke="#1b1b1f" stroke-width="9" stroke-linecap="round" />
        <g class="tag">
          <circle cx="200" cy="292" r="13" fill="#f4f4f5" stroke="#1b1b1f" stroke-width="2" />
          <path :d="povioP" fill="#1b1b1f" transform="translate(193.9 280.8) scale(0.6)" />
        </g>
      </g>
    </svg>
    <canvas ref="front" class="fire front" aria-hidden="true" />
  </div>
</template>

<style scoped>
.pyro-panda {
  position: relative;
  width: min(100%, 400px);
  aspect-ratio: 400 / 440;
  margin: 0 auto;
  cursor: pointer;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
}
/* Below 960px VitePress gives the hero image a fixed square (320 / 392px): fit its height. */
@media (max-width: 959px) {
  .pyro-panda {
    width: auto;
    height: 80%;
  }
}
svg {
  position: relative;
  display: block;
  width: 100%;
  height: 100%;
  overflow: visible;
}
.fire {
  position: absolute;
  left: -15%;
  top: -18.1818%;
  width: 130%;
  height: 118.1818%;
  pointer-events: none;
}
.front {
  left: -85%;
  top: -59.0909%;
  width: 270%;
  height: 186.3636%;
}
.back { z-index: 0; }
svg { z-index: 1; }
.front { z-index: 2; }

.tail {
  transform-box: view-box;
  transform-origin: 246px 356px;
  animation: sway 3.4s ease-in-out infinite alternate;
}
@keyframes sway {
  from { transform: rotate(-5deg); }
  to { transform: rotate(5deg); }
}
.panda { animation: breathe 4s ease-in-out infinite; }
@keyframes breathe {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-2px); }
}
.eye {
  transform-box: fill-box;
  transform-origin: 50% 50%;
  animation: blink 5.5s infinite;
}
@keyframes blink {
  0%, 93%, 100% { transform: scaleY(1); }
  95.5% { transform: scaleY(0.1); }
}
.tag {
  transform-box: fill-box;
  transform-origin: 50% 0%;
  animation: swing 4s ease-in-out infinite;
}
@keyframes swing {
  0%, 100% { transform: rotate(-7deg); }
  50% { transform: rotate(7deg); }
}
@media (prefers-reduced-motion: reduce) {
  .tail, .panda, .eye, .tag { animation: none; }
}
</style>
