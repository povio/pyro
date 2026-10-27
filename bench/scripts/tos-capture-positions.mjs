// Captures the rendered TOS Positions list as standalone HTML (stylesheets inlined, scripts removed) from a running
// TOS FE E2E runtime (`bun e2e:up` in tos-monorepo; see its apps/fe/e2e/README.md). Needs the slot backend started
// through the coordinator (POST /run/start, POST /test/start). Not committed: the captured HTML is app markup.
// Usage: bun capture-positions.mjs <tos-root> <out.html> [email]
import { readFile, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';
const [root, out, email = 'fe-e2e.master-data@example.com'] = process.argv.slice(2);
const state = JSON.parse(await readFile(path.join(root, '.tmp/fe-e2e-portless/infra-state.json'), 'utf8'));
const slot = state.slots[0];
const databaseUrl = slot.hostDatabaseUrl ?? slot.databaseUrl;
const apiUrl = slot.url ?? state.browserApiUrl;
console.error('fe', state.feUrl, 'api', apiUrl, 'db', String(databaseUrl).replace(/:[^:@/]*@/, ':***@'));
const { stdout } = await promisify(execFile)('bun', ['--filter', 'be', 'magic-link'], {
  cwd: root, env: { ...process.env, MAGIC_LINK_LOGIN_EMAIL: email, DATABASE_URL: databaseUrl, STAGE: 'local' }, maxBuffer: 1 << 22,
});
const link = stdout.match(/https?:\/\/\S+/)?.[0];
if (!link) throw new Error('no magic link in output:\n' + stdout.slice(-800));
const { chromium } = await import('/Users/povioserver/Documents/GitHub/tos-monorepo/apps/fe/node_modules/playwright/index.mjs');
const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
// The FE build resolves its API base from this global (app.config.ts); the E2E browser fixture injects it the same way.
await context.addInitScript((url) => { globalThis.__E2E_API_URL__ = url; }, apiUrl);
const page = await context.newPage();
// The FE calls its own origin under /api; the E2E fixtures reroute that to the backend slot (api-routing.ts).
const feOrigin = new URL(state.feUrl).origin;
await page.route((u) => u.origin === feOrigin && (u.pathname === '/api' || u.pathname.startsWith('/api/')), async (route) => {
  const u = new URL(route.request().url());
  const target = new URL(apiUrl); target.pathname = u.pathname === '/api' ? '/' : u.pathname.slice(4); target.search = u.search;
  const resp = await route.fetch({ url: target.toString() });
  await route.fulfill({ response: resp });
});
await page.goto(link.replace(/^https?:\/\/[^/]+/, state.feUrl.replace(/\/$/, '')), { waitUntil: 'networkidle' });
await page.goto(state.feUrl.replace(/\/$/, '') + '/platform/logistics/positions/', { waitUntil: 'networkidle' });
await page.locator('table tbody tr').first().waitFor({ timeout: 60000 });
await page.waitForTimeout(500);
const html = await page.evaluate(async () => {
  const css = [];
  for (const sheet of Array.from(document.styleSheets)) {
    try { css.push(Array.from(sheet.cssRules).map((r) => r.cssText).join('\n')); } catch (e) { css.push('/* skipped: ' + e.message + ' */'); }
  }
  const doc = document.documentElement.cloneNode(true);
  doc.querySelectorAll('script, link[rel="stylesheet"], link[rel="modulepreload"], style').forEach((n) => n.remove());
  const style = document.createElement('style'); style.textContent = css.join('\n');
  doc.querySelector('head').appendChild(style);
  return { html: '<!doctype html>' + doc.outerHTML, rows: document.querySelectorAll('table tbody tr').length, elements: document.querySelectorAll('*').length, rules: css.join('\n').split('\n').length };
});
await writeFile(out, html.html);
console.error(JSON.stringify({ rows: html.rows, elements: html.elements, cssLines: html.rules, bytes: html.html.length }));
await browser.close();
