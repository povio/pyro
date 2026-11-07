// page.coverage: JS block coverage (V8's format, what c8 / v8-to-istanbul read)
// and CSS rule usage, on pages served through page.route.
import { expect, test } from './fixtures';

const APP_JS = `function used(x) {
  if (x > 0) {
    return 'positive';
  } else {
    return 'negative';
  }
}
function unused() {
  return 'never';
}
document.getElementById('out').textContent = used(1) + used(2);
`;
const MODULE_JS = `export function fromModule() { return 'module'; }
export function notFromModule() { return 'nope'; }
document.getElementById('mod').textContent = fromModule();
`;
// Anonymous functions: V8 names them after what they are assigned to.
const NAMES_JS = `var ns = { deep: {} };
ns.a = function () { return 1; };
ns.deep.b = () => 2;
ns['lit'] = function () { return 3; };
function Ctor() { this.m = function () { return 4; }; }
Ctor.prototype.p = function () { return 5; };
var o = { g: [function () { return 6; }] };
ns.paren = (function () { return 7; });
class K {}
class L extends K {}
window.run = () => [ns.a(), ns.deep.b(), ns.lit(), new Ctor().m(), new Ctor().p(), o.g[0](), ns.paren(), new L()];
`;
const STYLE_CSS = `.used { color: red; }
.unused { color: blue; }
`;
const PAGE = `<!doctype html><html><head>
<link rel="stylesheet" href="/cov/style.css">
<style>#out { font-weight: bold; } .nothing-here { color: green; }</style>
</head><body><div id="out"></div><div id="mod" class="used"></div>
<script src="/cov/app.js"></script>
<script type="module" src="/cov/module.js"></script>
<script>eval('1 + 1');</script>
</body></html>`;

async function serve(page: any) {
  await page.route('**/cov/**', (route: any) => {
    const path = new URL(route.request().url()).pathname;
    const body: Record<string, [string, string]> = {
      '/cov/page.html': ['text/html', PAGE],
      '/cov/other.html': ['text/html', '<!doctype html><script src="/cov/other.js"></script>'],
      '/cov/app.js': ['application/javascript', APP_JS],
      '/cov/other.js': ['application/javascript', 'function other() { return 1; } other();'],
      '/cov/module.js': ['application/javascript', MODULE_JS],
      '/cov/style.css': ['text/css', STYLE_CSS],
      '/cov/nice.html': ['text/html', '<!doctype html><script>function nice() { return 1; } nice();\n//# sourceURL=nicename.js</script>'],
      '/cov/lp.html': ['text/html', '<!doctype html><script>function widget() { return 1; } widget();\n//# sourceURL=lp-widget.js</script>'],
      '/cov/names.html': ['text/html', '<!doctype html><script src="/cov/names.js"></script>'],
      '/cov/names.js': ['application/javascript', NAMES_JS],
      '/cov/brief.html': ['text/html', '<!doctype html><style>.brief { color: teal; } .never { color: navy; }</style><body></body>'],
    };
    const [contentType, text] = body[path] ?? ['text/plain', ''];
    return route.fulfill({ status: body[path] ? 200 : 404, contentType, body: text });
  });
}

type Fn = { functionName: string; isBlockCoverage: boolean; ranges: Array<{ startOffset: number; endOffset: number; count: number }> };
const fn = (functions: Fn[], name: string) => {
  const found = functions.find((f) => f.functionName === name);
  expect(found, name).toBeTruthy();
  return found!;
};
const slice = (source: string, r: { startOffset: number; endOffset: number }) => source.slice(r.startOffset, r.endOffset);

test('JS coverage: executed and unexecuted functions and blocks', async ({ page }) => {
  await serve(page);
  await page.coverage.startJSCoverage();
  await page.goto('/cov/page.html');
  await expect(page.locator('#mod')).toHaveText('module');
  await expect(page.locator('#out')).toHaveText('positivepositive');
  const entries = await page.coverage.stopJSCoverage();

  const app = entries.find((e) => e.url.endsWith('/cov/app.js'))!;
  expect(app, JSON.stringify(entries.map((e) => e.url))).toBeTruthy();
  expect(typeof app.scriptId).toBe('string');
  expect(app.source).toBe(APP_JS);
  // The first function is the script as a whole.
  expect(app.functions[0].ranges[0]).toMatchObject({ startOffset: 0, endOffset: APP_JS.length });
  expect(app.functions[0].ranges[0].count).toBeGreaterThan(0);
  for (const f of app.functions) {
    expect(typeof f.functionName).toBe('string');
    expect(f.ranges.length).toBeGreaterThan(0);
    for (const r of f.ranges) expect(r.endOffset).toBeGreaterThan(r.startOffset);
  }
  const used = fn(app.functions, 'used');
  expect(used.isBlockCoverage).toBe(true);
  expect(slice(APP_JS, used.ranges[0])).toMatch(/^function used/);
  expect(used.ranges[0].count).toBeGreaterThan(0);
  // The else branch never ran: a nested range with count 0 covers it.
  const zero = used.ranges.slice(1).filter((r) => r.count === 0).map((r) => slice(APP_JS, r));
  expect(zero.some((t) => t.includes("'negative'"))).toBe(true);
  expect(zero.some((t) => t.includes("'positive'"))).toBe(false);
  const unused = fn(app.functions, 'unused');
  expect(unused.ranges[0].count).toBe(0);
  expect(slice(APP_JS, unused.ranges[0])).toMatch(/^function unused\(\) \{[\s\S]*\}$/);

  const mod = entries.find((e) => e.url.endsWith('/cov/module.js'))!;
  expect(mod).toBeTruthy();
  expect(mod.source).toBe(MODULE_JS);
  expect(fn(mod.functions, 'fromModule').ranges[0].count).toBeGreaterThan(0);
  expect(fn(mod.functions, 'notFromModule').ranges[0].count).toBe(0);

  // eval code is anonymous: left out unless reportAnonymousScripts.
  expect(entries.every((e) => e.url !== '')).toBe(true);
});

test('JS coverage: anonymous scripts and resetOnNavigation', async ({ page }) => {
  await serve(page);
  await page.coverage.startJSCoverage({ reportAnonymousScripts: true, resetOnNavigation: false });
  await page.goto('/cov/page.html');
  await expect(page.locator('#out')).toHaveText('positivepositive');
  await page.goto('/cov/other.html');
  const entries = await page.coverage.stopJSCoverage();
  const urls = entries.map((e) => e.url);
  // Kept across the navigation.
  expect(urls.some((u) => u.endsWith('/cov/app.js')), JSON.stringify(urls)).toBe(true);
  expect(urls.some((u) => u.endsWith('/cov/other.js')), JSON.stringify(urls)).toBe(true);
  const anonymous = entries.filter((e) => e.url === '');
  expect(anonymous.some((e) => e.source === '1 + 1'), JSON.stringify(anonymous)).toBe(true);

  // The default drops what earlier documents ran.
  await page.coverage.startJSCoverage();
  await page.goto('/cov/page.html');
  await page.goto('/cov/other.html');
  const after = (await page.coverage.stopJSCoverage()).map((e) => e.url);
  expect(after.some((u) => u.endsWith('/cov/other.js')), JSON.stringify(after)).toBe(true);
  expect(after.some((u) => u.endsWith('/cov/app.js')), JSON.stringify(after)).toBe(false);
});

test('CSS coverage: used rules of linked and inline sheets', async ({ page }) => {
  await serve(page);
  await page.coverage.startCSSCoverage();
  await page.goto('/cov/page.html');
  await expect(page.locator('#mod')).toHaveText('module');
  const entries = await page.coverage.stopCSSCoverage();

  const linked = entries.find((e) => e.url.endsWith('/cov/style.css'))!;
  expect(linked, JSON.stringify(entries.map((e) => e.url))).toBeTruthy();
  expect(linked.text).toBe(STYLE_CSS);
  const usedText = linked.ranges.map((r) => linked.text!.slice(r.start, r.end)).join('');
  expect(usedText).toContain('.used { color: red; }');
  expect(usedText).not.toContain('.unused');

  const inline = entries.find((e) => e.text?.includes('#out'))!;
  expect(inline).toBeTruthy();
  expect(inline.url).toMatch(/\/cov\/page\.html$/);
  const inlineUsed = inline.ranges.map((r) => inline.text!.slice(r.start, r.end)).join('');
  expect(inlineUsed).toContain('#out { font-weight: bold; }');
  expect(inlineUsed).not.toContain('.nothing-here');
});

test('stopping coverage that was never started gives no entries', async ({ page }) => {
  expect(await page.coverage.stopJSCoverage()).toEqual([]);
  expect(await page.coverage.stopCSSCoverage()).toEqual([]);
});

test('JS coverage: a relative sourceURL names the script', async ({ page }) => {
  await serve(page);
  await page.coverage.startJSCoverage();
  await page.goto('/cov/nice.html');
  const entries = await page.coverage.stopJSCoverage();
  const urls = entries.map((e) => e.url);
  expect(urls, JSON.stringify(urls)).toContain('nicename.js');
});

test('JS coverage: a page script named lp-something is page code', async ({ page }) => {
  await serve(page);
  await page.coverage.startJSCoverage();
  await page.goto('/cov/lp.html');
  const entries = await page.coverage.stopJSCoverage();
  const widget = entries.find((e) => e.url === 'lp-widget.js')!;
  expect(widget, JSON.stringify(entries.map((e) => e.url))).toBeTruthy();
  expect(fn(widget.functions, 'widget').ranges[0].count).toBeGreaterThan(0);
});

test('JS coverage: per page, with scripts a page ran before it started', async ({ page, context }) => {
  await serve(page);
  const other = await context.newPage();
  await serve(other);
  await page.coverage.startJSCoverage();
  // Loaded while only the first page has coverage on.
  await other.goto('/cov/page.html');
  await expect(other.locator('#out')).toHaveText('positivepositive');
  await other.coverage.startJSCoverage();
  await other.evaluate(() => (window as any).used(-1));
  await page.goto('/cov/other.html');
  const mine = (await page.coverage.stopJSCoverage()).map((e) => e.url);
  const theirs = await other.coverage.stopJSCoverage();
  expect(mine.some((u) => u.endsWith('/cov/other.js')), JSON.stringify(mine)).toBe(true);
  expect(mine.some((u) => u.endsWith('/cov/app.js')), JSON.stringify(mine)).toBe(false);
  const app = theirs.find((e) => e.url.endsWith('/cov/app.js'))!;
  expect(app, JSON.stringify(theirs.map((e) => e.url))).toBeTruthy();
  expect(app.source).toBe(APP_JS);
  expect(fn(app.functions, 'used').ranges[0].count).toBeGreaterThan(0);
  // Ran before it started, not since: left out, as V8 does.
  expect(app.functions.some((f) => f.functionName === 'unused')).toBe(false);
  expect(theirs.some((e) => e.url.endsWith('/cov/other.js'))).toBe(false);
  await other.close();
});

test('JS coverage: started after goto, what runs since', async ({ page }) => {
  await serve(page);
  await page.goto('/cov/page.html');
  await expect(page.locator('#out')).toHaveText('positivepositive');
  await page.coverage.startJSCoverage();
  await page.evaluate(() => (window as any).used(-1) + (window as any).used(-2));
  const entries = await page.coverage.stopJSCoverage();
  // As Chrome: counts start at zero and V8 reports a function only when it
  // ran since, or the reported function around it did. The script's top
  // level and `unused` ran (or were declared) before: left out, and so is
  // a script none of whose code ran since.
  expect(entries.some((e) => e.url.endsWith('/cov/module.js')), JSON.stringify(entries.map((e) => e.url))).toBe(false);
  const app = entries.find((e) => e.url.endsWith('/cov/app.js'))!;
  expect(app, JSON.stringify(entries.map((e) => e.url))).toBeTruthy();
  expect(app.source).toBe(APP_JS);
  expect(app.functions.map((f) => f.functionName)).toEqual(['used']);
  const used = fn(app.functions, 'used');
  expect(used.ranges[0].count).toBe(2);
  expect(slice(APP_JS, used.ranges[0])).toMatch(/^function used/);
});

test('CSS coverage: a rule matched by a style recalculation of an element gone since', async ({ page }) => {
  await serve(page);
  await page.coverage.startCSSCoverage();
  await page.goto('/cov/brief.html');
  // In the document only while its style is read.
  await page.evaluate(() => {
    const el = document.body.appendChild(document.createElement('div'));
    el.className = 'brief';
    getComputedStyle(el).color;
    el.remove();
  });
  const entries = await page.coverage.stopCSSCoverage();
  const sheet = entries.find((e) => e.text?.includes('.brief'))!;
  expect(sheet, JSON.stringify(entries)).toBeTruthy();
  const used = sheet.ranges.map((r) => sheet.text!.slice(r.start, r.end)).join('');
  expect(used).toContain('.brief { color: teal; }');
  expect(used).not.toContain('.never');
});

// Event listeners on the connection, by CDP method, where the page has ours
// (lightning-panda's Page); undefined on stock Playwright.
const cdpListeners = (page: any): Record<string, number> | undefined => {
  const map: Map<string, Set<unknown>> | undefined = page._cdp?.listeners;
  if (!(map instanceof Map)) return undefined;
  const out: Record<string, number> = {};
  for (const [method, set] of map) if (/^(Debugger|CSS)\.|^Page\.frameNavigated$/.test(method)) out[method] = set.size;
  return out;
};

test('coverage listeners go with the page when it closes without stopping', async ({ context }) => {
  const page = await context.newPage();
  await serve(page);
  const before = cdpListeners(page);
  await page.coverage.startJSCoverage();
  await page.coverage.startCSSCoverage();
  await page.goto('/cov/page.html');
  if (before) expect(cdpListeners(page)).not.toEqual(before);
  await page.close();
  // Both JS and CSS coverage's listeners are gone, not just the first's.
  if (before) {
    const after = cdpListeners(page)!;
    for (const method of new Set([...Object.keys(before), ...Object.keys(after)])) {
      expect(after[method] ?? 0, method).toBeLessThanOrEqual(before[method] ?? 0);
    }
  }
  // Nothing left to stop.
  expect(await page.coverage.stopJSCoverage().catch(() => [])).toEqual([]);
  expect(await page.coverage.stopCSSCoverage().catch(() => [])).toEqual([]);
});

test('JS coverage: anonymous functions named as V8 names them', async ({ page }) => {
  await serve(page);
  await page.coverage.startJSCoverage();
  await page.goto('/cov/names.html');
  await page.evaluate(() => (window as any).run());
  const entries = await page.coverage.stopJSCoverage();
  const names = entries.find((e) => e.url.endsWith('/cov/names.js'))!;
  // A function right after `(` keeps no inferred name, and a class's
  // implicit constructor is no function of the script.
  expect(names.functions.map((f) => f.functionName)).toEqual(['', 'ns.a', 'ns.deep.b', 'ns.lit', 'Ctor', 'Ctor.m', 'Ctor.p', 'o.g', '', 'window.run']);
});
