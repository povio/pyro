// Crashtests for run.mjs --kind crashtest: load a page, wait as WPT's runner does, paint it, tear it down, and report
// the page as one subtest ("crashtest") that passes when the browser neither crashed, panicked nor hung, so gate.mjs
// gates crashtests exactly as it gates reftests. See README.md, "Crashtests".
//
// WPT's semantics (docs/writing-tests/crashtest.md and tools/wptrunner at the pinned revision):
// - A crashtest is a markup file whose name ends in -crash before its extension(s), or that lies under a directory
//   named crashtests (tools/manifest/sourcefile.py, name_is_crashtest), outside the non-test directories.
// - "The test passes if the load event is reached, and the browser finishes painting, without terminating." The
//   runner loads the page and runs test-wait.js with the class name test-wait: it waits for the load event,
//   document.fonts.ready and two animation frames; with class="test-wait" on the root element it then fires
//   TestRendered at it and waits until the class is gone, two frames after each change of the root's attributes.
// pyro lays out and paints on demand, so the runner then asks for a screenshot (and drops it): without it a page
// that only crashes layout or paint would pass unrendered. It then navigates to about:blank, so the document's
// teardown happens inside the test that built it rather than under the next page of the slot.

import fs from 'node:fs';
import path from 'node:path';
import { H2_NAME, NEEDS_H2, documentError } from './lib.mjs';

// The whole checkout: crashtests are few, and fetch-wpt.sh checks out every one of them (wpt-paths.txt).
export const CRASHTEST_DIRS = ['.'];

const MARKUP = /\.(html|htm|xht|xhtml|xml|svg)$/;
// The manifest's non-test directories: nothing under them is a test.
const NON_TEST_DIRS = new Set(['resources', 'support', 'tools']);
function walk(dir, out = []) {
  let entries;
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return out; }
  for (const e of entries) {
    if (e.name === '.git') continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) { if (!NON_TEST_DIRS.has(e.name)) walk(p, out); }
    else if (MARKUP.test(e.name)) out.push(p);
  }
  return out;
}

/** The manifest's test for a crashtest's name: the flag after the last '-' of the name (before the first '.' that
 *  follows it) is `crash`, or a directory of the path is `crashtests`. `rel` is relative to the WPT root. */
export function isCrashtestName(rel) {
  const parts = rel.split('/');
  const name = parts.pop().replace(/\.[^.]*$/, '');
  const flag = name.includes('-') ? name.slice(name.lastIndexOf('-') + 1).split('.')[0] : null;
  return flag === 'crash' || parts.includes('crashtests');
}

// The area a page is counted under: css/<spec>, html/<section>, svg/<section>, else the top-level directory.
const areaOf = (rel) => {
  const p = rel.split('/');
  return ['css', 'html', 'svg'].includes(p[0]) && p.length > 2 ? `${p[0]}/${p[1]}` : p[0];
};

// What a page needs from wptserve that serve.mjs does not do: substitutions, pipes, Python handlers, other hosts.
const SERVER_NAME = /\.(sub|https|h2|www)\./;
const SERVER_CONTENT = /\.py\b|[?&]pipe=|\.sub\.(html|htm|xht|xhtml|svg|css|js)\b/;
const TESTDRIVER = /testdriver\.js|testdriver-vendor/;

/** Every crashtest under `dirs` that serve.mjs can serve, one entry per file (crashtests have no variants).
 *  `excluded` counts what was left out and why. */
export function discoverCrashtests(wpt, dirs, { reasons, wptserve = false, testdriver = false } = {}) {
  const tests = [];
  const excluded = {};
  let rel;
  const exclude = (why) => { excluded[why] = (excluded[why] ?? 0) + 1; reasons?.set(rel, why); };
  const seen = new Set();
  for (const d of dirs) {
    for (const f of walk(path.join(wpt, d))) {
      rel = path.relative(wpt, f).split(path.sep).join('/');
      if (seen.has(rel) || !isCrashtestName(rel)) continue;
      seen.add(rel);
      const base = path.basename(f);
      // Before the wptserve check, so the count is the same under both servers.
      if (H2_NAME.test(base)) { exclude(NEEDS_H2); continue; }
      if (!wptserve && SERVER_NAME.test(base)) { exclude('needs wptserve (substitution, https, h2, another host)'); continue; }
      const src = fs.readFileSync(f, 'utf8').replace(/<!--[\s\S]*?-->/g, '');
      if (!testdriver && TESTDRIVER.test(src)) { exclude('needs testdriver'); continue; }
      if (!wptserve && SERVER_CONTENT.test(src)) { exclude('needs wptserve (substitution, pipe, Python handler)'); continue; }
      const long = /<meta\s+name=["']?timeout["']?\s+content=["']?long/i.test(src);
      tests.push({ dir: areaOf(rel), url: rel, long });
    }
  }
  tests.sort((a, b) => (a.url < b.url ? -1 : 1));
  return { tests, excluded };
}

// Runs in the page: wptrunner's test-wait.js for the class `test-wait`. Resolves {done: true} once the page is ready
// (above), or {done: false, wait} after `ms` (`wait`: the root still carries the class). The frames take real time,
// as reftest.mjs's do: a requestAnimationFrame callback a 60 Hz frame's time after the last wait began.
function settle(ms) {
  return new Promise((resolve) => {
    const root = document.documentElement;
    const timer = setTimeout(() => resolve({ done: false, wait: !!document.documentElement?.classList.contains('test-wait') }), ms);
    let observer = null;
    const ready = () => {
      if (root && root.classList.contains('test-wait')) {
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
    const frame = (then) => setTimeout(() => requestAnimationFrame(then), 1000 / 60);
    function paints() { frame(() => frame(ready)); }
    const load = () => {
      // Layout now: pyro requests a page's web fonts when layout first needs them.
      void root?.getBoundingClientRect();
      (document.fonts ? document.fonts.ready : Promise.resolve()).then(paints, paints);
    };
    if (document.readyState !== 'complete') addEventListener('load', load, { once: true });
    else load();
  });
}

/** One crashtest: { status, ms, subtests } as run.mjs records it, or throws (an error result; run.mjs tells a lost
 *  browser from the rest). A crash, a panic and a hang are run.mjs's to see: this only loads, waits, paints and
 *  tears down. */
export async function runCrashtest(page, t, base, limit) {
  const t0 = Date.now();
  const deadline = t0 + limit;
  const left = () => Math.max(1, deadline - Date.now());
  // An error document (the server failed, the file is missing) loads and paints like any page: never a pass.
  const bad = documentError(await page.goto(`${base}/${t.url}`, { waitUntil: 'commit', timeout: left() }), t.url);
  if (bad) return { status: 'error', ms: Date.now() - t0, subtests: [], error: bad };
  for (;;) {
    // A navigation inside the page destroys the context the wait runs in: wait again in the new one.
    let navigated = false;
    const r = await page.evaluate(settle, left()).catch((e) => {
      if (!/destroyed|navigat/i.test(String(e.message))) throw e;
      navigated = true;
      return null;
    });
    if (r?.done) break;
    // A page that navigates until its time is up never loaded.
    if (navigated && Date.now() >= deadline) return { status: 'timeout', ms: Date.now() - t0, subtests: [], error: 'the page kept navigating' };
    if (r) return { status: 'timeout', ms: Date.now() - t0, subtests: [], error: r.wait ? 'test-wait was not removed' : 'the page did not load' };
  }
  // "...and the browser finishes painting": layout and paint of the final state. pyro answers format "rgba" without
  // encoding; another browser takes its ordinary screenshot.
  const cdp = await page.context().newCDPSession(page);
  try {
    let painted = false;
    try { painted = !!(await cdp.send('Page.captureScreenshot', { format: 'rgba' })).data; } catch (e) { if (/closed|ECONNREFUSED/i.test(String(e.message))) throw e; }
    if (!painted) await page.screenshot({ type: 'png', timeout: left() + 5000 });
  } finally {
    cdp.detach().catch(() => {});
  }
  const ms = Date.now() - t0;
  // Teardown belongs to this test: a document whose destruction kills the browser would otherwise take the next
  // page of the slot down instead.
  await page.goto('about:blank', { timeout: 10000 });
  return { status: 'complete', ms, subtests: [{ name: 'crashtest', status: 'Pass', msg: '' }] };
}
