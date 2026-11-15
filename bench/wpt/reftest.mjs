// Reftests for run.mjs --kind reftest: render a test and its references in the browser under test, compare the
// screenshots with WPT's rules, and report the page as one subtest ("reftest"), so gate.mjs gates reftests exactly as
// it gates testharness subtests. See README.md, "Reftests".
//
// WPT's semantics (docs/writing-tests/reftests.md and tools/wptrunner at the pinned revision):
// - Every page renders in an 800x600 CSS px viewport at device scale factor 1 (a <meta name=viewport-size> sets
//   another size).
// - The screenshot waits for the load event, document.fonts.ready and two animation frames. With class="reftest-wait"
//   on the root element, the runner then fires TestRendered at it and waits until the class is gone, two frames after
//   each change of the root's attributes (wptrunner's test-wait.js). The frames are real ones, as Chrome's at 60 Hz:
//   each requestAnimationFrame callback comes a frame's time (16.7 ms) after the last wait began. pyro runs frames
//   back to back after each client command, so a page's animations would otherwise be captured within a millisecond
//   of their start (backface-visibility-hidden-animated-002 turns 120 degrees in its first millisecond).
// - Two screenshots are equal if no pixel differs in R, G or B. With fuzzy metadata ([ref:]maxDifference;totalPixels,
//   inclusive ranges) they are equal if the largest channel difference and the number of differing pixels both fall
//   in the ranges, or if nothing differs and a range allows 0 (wptrunner's check_pass).
// - A test passes if it equals at least one of its `match` references (when it has any) and none of its `mismatch`
//   references. A reference's own references are not followed (as wptrunner's ReftestTest.from_manifest).
// The reference renders are cached by URL and viewport (as wptrunner's screenshot cache), so a reference that many
// tests share renders once per runner.

import path from 'node:path';

/** CDP screenshot of the viewport as raw RGBA. pyro answers format "rgba" without encoding; other browsers get PNG. */
export async function capture(page, cdp) {
  try {
    const r = await cdp.send('Page.captureScreenshot', { format: 'rgba' });
    if (r.pyro?.width) return { width: r.pyro.width, height: r.pyro.height, data: Buffer.from(r.data, 'base64') };
  } catch {}
  const { decodePng } = await import('../fidelity/image.ts');
  const img = decodePng(await page.screenshot({ type: 'png' }));
  return { width: img.width, height: img.height, data: Buffer.from(img.data.buffer, img.data.byteOffset, img.data.byteLength) };
}

/** Pixels whose R, G or B differ, and the largest such difference (alpha ignored, as wptrunner converts to RGB). */
export function difference(a, b) {
  if (a.width !== b.width || a.height !== b.height) return { pixels: Math.max(a.width * a.height, b.width * b.height), max: 255, size: true };
  if (a.data.equals(b.data)) return { pixels: 0, max: 0 };
  const x = a.data, y = b.data;
  let pixels = 0, max = 0;
  for (let i = 0; i < x.length; i += 4) {
    const d = Math.max(Math.abs(x[i] - y[i]), Math.abs(x[i + 1] - y[i + 1]), Math.abs(x[i + 2] - y[i + 2]));
    if (d) { pixels++; if (d > max) max = d; }
  }
  return { pixels, max };
}

/** wptrunner's check_pass for one comparison: are the two renders equal within `fuzzy` ([[maxDiff lo, hi], [pixels lo, hi]])? */
export function equalWithin({ pixels, max }, fuzzy) {
  if (!fuzzy || (fuzzy[0][0] === 0 && fuzzy[0][1] === 0 && fuzzy[1][0] === 0 && fuzzy[1][1] === 0)) return pixels === 0 && max === 0;
  const [[dLo, dHi], [pLo, pHi]] = fuzzy;
  return (pixels === 0 && pLo === 0) || (max === 0 && dLo === 0) || (dLo <= max && max <= dHi && pLo <= pixels && pixels <= pHi);
}

/** The test's fuzzy value for a reference: the one keyed by its URL, else the unprefixed one. */
export const fuzzyFor = (t, ref) => t.fuzzy?.[ref] ?? t.fuzzy?.[''];

/** Pass or fail, and a one-line account of the comparisons, from the test's render and its references' renders. */
export function verdict(t, test, refs) {
  const parts = [];
  const cmp = t.refs.map((r, i) => {
    const d = difference(test, refs[i]);
    const eq = equalWithin(d, fuzzyFor(t, r.url));
    parts.push(`${r.rel} ${r.url.split('/').pop()}: ${d.size ? 'size differs' : `${d.pixels} px differ, max ${d.max}`}${t.fuzzy ? ` (fuzzy ${JSON.stringify(fuzzyFor(t, r.url) ?? null)})` : ''}`);
    return { rel: r.rel, eq };
  });
  const matches = cmp.filter((c) => c.rel === '==');
  const pass = (!matches.length || matches.some((c) => c.eq)) && cmp.filter((c) => c.rel === '!=').every((c) => !c.eq);
  return { pass, msg: parts.join('; ') };
}

// Runs in the page: resolves {done: true} once the page is ready for its screenshot (above), or {done: false} after
// `ms`. Layout is forced first: pyro requests a page's web fonts when layout first needs them, and its
// document.fonts.ready does not wait for fonts not requested yet.
function settle(ms) {
  return new Promise((resolve) => {
    const root = document.documentElement;
    const timer = setTimeout(() => resolve({ done: false, wait: !!root?.classList.contains('reftest-wait') }), ms);
    let observer = null;
    const ready = () => {
      if (root && root.classList.contains('reftest-wait')) {
        if (!observer) {
          observer = new MutationObserver(paints);
          observer.observe(root, { attributes: true });
          root.dispatchEvent(new Event('TestRendered', { bubbles: true }));
        }
        return;
      }
      observer?.disconnect();
      clearTimeout(timer);
      resolve({ done: true });
    };
    // One real animation frame: a requestAnimationFrame callback a 60 Hz frame's time after now.
    const frame = (then) => setTimeout(() => requestAnimationFrame(then), 1000 / 60);
    function paints() { frame(() => frame(ready)); }
    const load = () => {
      void root?.getBoundingClientRect();
      void document.body?.offsetWidth;
      (document.fonts ? document.fonts.ready : Promise.resolve()).then(paints, paints);
    };
    if (document.readyState !== 'complete') addEventListener('load', load, { once: true });
    else load();
  });
}

const VIEWPORT = { width: 800, height: 600 };
export const REFTEST_CONTEXT = { viewport: VIEWPORT, deviceScaleFactor: 1 };

/** The browser's environment for reftests, as wptrunner starts Chrome: text drawn without antialiasing
 *  (`--enable-blink-features=NoFontAntialiasing`, wptrunner's chrome.py), and Ahem installed (`--install-fonts`), here
 *  the WPT checkout's fonts/Ahem.ttf given to pyro (`--font`) rather than put in the system's font folders. Through
 *  environment variables, which a pyro without these options ignores (an older binary in an A/B run). */
export const reftestEnv = (wpt) => ({
  PYRO_TEXT_ANTIALIASING: 'off',
  // As wptrunner's Chrome (`--autoplay-policy=no-user-gesture-required`): a page whose `<video autoplay>` must play
  // before it is done otherwise depends on the runner's first evaluation (a user gesture) beating the media's load
  // (css/css-overflow/overflow-video-hidden.html timed out on first runs).
  PYRO_AUTOPLAY_POLICY: 'no-user-gesture-required',
  ...ahemEnv(wpt),
});
/** Ahem alone, for testharness pages (wptrunner installs it for every test; geometry pages measure its boxes). */
export const ahemEnv = (wpt) => ({ PYRO_FONTS: path.join(wpt, 'fonts', 'Ahem.ttf') });

/** One page's render: navigate, settle (bounded by `deadline`, a Date.now() value), capture. */
async function render(page, cdp, base, url, deadline) {
  const left = () => Math.max(1, deadline - Date.now());
  await page.goto(url === 'about:blank' ? url : `${base}/${url}`, { waitUntil: 'commit', timeout: left() });
  // A navigation inside the page destroys the context the wait runs in: wait again in the new one.
  for (;;) {
    const r = await page.evaluate(settle, left()).catch((e) => (Date.now() < deadline && /destroyed|navigat/i.test(String(e.message)) ? null : Promise.reject(e)));
    if (r?.done) return capture(page, cdp);
    if (r) return { timeout: true, wait: r.wait };
  }
}

const cache = new Map();
const CACHE_MAX = 48; // ~92 MB of 800x600 RGBA renders
/** A reference's render, from the cache when this runner rendered it before at this viewport. */
async function renderRef(page, cdp, base, url, viewport, deadline) {
  const key = `${viewport.width}x${viewport.height} ${url}`;
  const hit = cache.get(key);
  if (hit) { cache.delete(key); cache.set(key, hit); return hit; }
  const r = await render(page, cdp, base, url, deadline);
  if (r.timeout) return r;
  cache.set(key, r);
  if (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value);
  return r;
}

// REFTEST_SHOTS=DIR: write the renders of each failing test (and its references) there as PNG, for debugging.
async function keep(t, test, refs) {
  const dir = process.env.REFTEST_SHOTS;
  if (!dir) return;
  const fs = await import('node:fs');
  const path = await import('node:path');
  const { encodePng } = await import('../fidelity/image.ts');
  fs.mkdirSync(dir, { recursive: true });
  const name = t.url.replace(/[^\w.-]+/g, '_');
  fs.writeFileSync(path.join(dir, `${name}.test.png`), encodePng(test, 1));
  refs.forEach((r, i) => fs.writeFileSync(path.join(dir, `${name}.ref${i}.png`), encodePng(r, 1)));
}

/** One reftest: { status, ms, subtests } as run.mjs records it, or throws (an error result). */
export async function runReftest(page, t, base, limit) {
  const t0 = Date.now();
  const deadline = t0 + limit;
  const viewport = t.viewport ?? VIEWPORT;
  const now = page.viewportSize();
  if (now?.width !== viewport.width || now?.height !== viewport.height) await page.setViewportSize(viewport);
  const cdp = await page.context().newCDPSession(page);
  try {
    const test = await render(page, cdp, base, t.url, deadline);
    if (test.timeout) return { status: 'timeout', ms: Date.now() - t0, subtests: [], error: test.wait ? 'reftest-wait was not removed' : 'the page did not load' };
    const refs = [];
    for (const r of t.refs) {
      const shot = await renderRef(page, cdp, base, r.url, viewport, deadline);
      if (shot.timeout) return { status: 'timeout', ms: Date.now() - t0, subtests: [], error: `reference ${r.url} ${shot.wait ? 'kept reftest-wait' : 'did not load'}` };
      refs.push(shot);
    }
    const v = verdict(t, test, refs);
    if (!v.pass) await keep(t, test, refs);
    return { status: 'complete', ms: Date.now() - t0, subtests: [{ name: 'reftest', status: v.pass ? 'Pass' : 'Fail', msg: v.msg.slice(0, 300) }] };
  } finally {
    cdp.detach().catch(() => {});
  }
}
