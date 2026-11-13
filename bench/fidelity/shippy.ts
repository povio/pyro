// Chromium vs pyro screenshot fidelity on the ToS app ("Shippy"): the screens
// the ToS feature videos capture, opened in both browsers on the same data,
// at the videos' capture size (2056x1200 CSS px, deviceScaleFactor 3, so
// 6168x3600 PNGs), then compared: SSIM on luma (11x11 Gaussian window, sigma
// 1.5, K1 0.01, K2 0.03) on the full image and on a 3x box-downscaled copy
// (what a capture shows at 100%), mean absolute difference, a diff heatmap and
// a side-by-side image per screen.
//
//   bun bench/fidelity/shippy.ts --tos <ToS checkout> [--port 47300] [--bin target/release/pyro]
//       [--out <dir>] [--screens positions,finances,...] [--stack <state file>] [--browsers chromium]
//       [--chromium-channel chromium|headless-shell] [--settle 1000]
//   bun bench/fidelity/shippy.ts --compare <run dir>      # metrics and images again from a run's captures
//
// Screens: `positions` (the positions list with its Financial Status pills),
// `finances` (a position's drawer, Finances tab: the charge table and totals),
// `dialog` (the same after typing an exchange rate above the Office's allowed
// range: the "Exchange rate out of range" confirmation), `bookkeeping` and
// `bookkeeping-ranges` (Master Data > Office > Settings > Bookkeeping, top and
// scrolled to its end) and `invoices` (Finances > Invoices). The dialog and
// the Allowed Exchange Rates section need a ToS checkout with the exchange
// rate range warning (branch feat/exchange-rate-range-warning, ToS PR 2201).
//
// The ToS checkout must be built for the port first (in it: `bun install
// --frozen-lockfile`, `FE_E2E_PORT=<port> bun scripts/fe-e2e/build.mts`). The
// harness starts ToS's own FE E2E stack (tos-stack.ts; or reuses one started
// with it, `--stack`), creates one fixture Office per screen through the
// stack's fixture worker, adjusts the data in the run database (exchange
// rate ranges, financial states) and signs both browsers in the way the ToS
// E2E harness does (tokens in localStorage). Chromium is the Playwright
// build the ToS checkout pins (`apps/fe/node_modules/playwright`), pyro is
// driven through `@povio/pyro/wright` (packages/pyro), deviceScaleFactor
// through CDP `Emulation.setDeviceMetricsOverride`. Both get Chromium's user
// agent and the Europe/Ljubljana time zone. The app's text is `system-ui`
// (SF Pro on macOS) and it loads no web fonts.
//
// Output (default ~/Library/Caches/lightningpanda/fidelity/<timestamp>; the
// captures show seed data, keep them out of git): per screen
// `<screen>/chromium.png`, `pyro.png`, `side.png` (Chromium left), `diff.png`
// (heatmap over the faded Chromium capture), `*-small.png` at 1/3, and
// `report.json` with the table this prints. The table's second part splits
// each screen's SSIM deficit by where it is, from regions of Chromium's DOM
// (empty inputs with a placeholder, text cut with an ellipsis, icons, text,
// the rest): which known gap costs how much.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { SQL } from 'bun';
import { decodePng, diffHeatmap, downscale, downscaleImage, encodePng, luminance, meanAbsoluteDifference, sideBySide, ssim, type Image } from './image';
import { startStack, type StackRuntime } from './tos-stack';

const args = process.argv.slice(2);
const opt = (name: string, fallback?: string) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const repo = resolve(import.meta.dir, '../..');
const VIEWPORT = { width: 2056, height: 1200 };
const DSF = 3;
const TIME_ZONE = 'Europe/Ljubljana';
process.env.TZ = TIME_ZONE;

type Office = {
  officeId: string;
  officeName: string;
  employeeId: string;
  token: string;
  expiresAt: number;
  endpoint: string;
  data: Record<string, any>;
};
/** What both browsers' drivers use: the part of Playwright's Page that pyro's wright implements too. */
type AnyPage = any;
type Screen = {
  name: string;
  office: string;
  path: (office: Office) => string;
  /** Waits until the screen shows its data. */
  ready: (page: AnyPage, office: Office) => Promise<void>;
  /** What the user does on the screen before the capture. */
  act?: (page: AnyPage, office: Office) => Promise<void>;
  /** Scroll the scroller at or under this CSS selector to its end before the capture (both scrollTops go to report.json). */
  scrollToEnd?: string;
};

// ---- data ----

/** One Office per screen group, from the stack's fixture worker (ToS's FE E2E fixtures). */
const OFFICES: Record<string, { feature: string; fixtures?: string[] }> = {
  positions: { feature: 'operations-position-listing' },
  // Charges in US Dollar and Euro, and an employee who may read the Office (its exchange rate ranges).
  finances: { feature: 'finance-exchange-rates' },
  bookkeeping: { feature: 'configuration-offices', fixtures: ['bank-currency'] },
  invoices: { feature: 'finance-invoices' },
};

// Sent as text and cast: a JS string bound straight to jsonb arrives as a JSON string, not an array.
const RANGES = JSON.stringify([{ currency: "USD", minRate: 0.8, maxRate: 1.25 }]);

async function createOffices(stack: StackRuntime, suffix: string): Promise<Record<string, Office>> {
  const offices: Record<string, Office> = {};
  for (const [key, spec] of Object.entries(OFFICES)) {
    const response = await fetch(`${stack.app}/__e2e/fixture`, {
      method: 'POST',
      headers: { authorization: `Bearer ${stack.rpcToken}`, 'content-type': 'application/json' },
      body: JSON.stringify({ action: 'office', feature: spec.feature, name: `fidelity-${key}${suffix}`, ...(spec.fixtures ? { fixtures: spec.fixtures } : {}) }),
    });
    const body = (await response.json()) as { value?: Office; error?: string };
    if (!response.ok || !body.value) throw new Error(`fixture ${spec.feature}: ${body.error ?? response.status}`);
    offices[key] = body.value;
  }
  return offices;
}

/** The run database (the stack's cluster holds one per run) that has `officeId`. */
async function runDatabase(stack: StackRuntime, officeId: string): Promise<SQL> {
  const base = `postgres://lpdt@127.0.0.1:${stack.port + 9}`;
  const admin = new SQL(`${base}/postgres`);
  const names = (await admin`select datname from pg_database where datname like 'lpdt_run_%'`).map((r: { datname: string }) => r.datname);
  await admin.close();
  for (const name of names) {
    const db = new SQL(`${base}/${name}`);
    const [hit] = await db`select 1 as hit from "Office" where id = ${officeId}`;
    if (hit) return db;
    await db.close();
  }
  throw new Error(`no run database holds office ${officeId}`);
}

/**
 * What the fixtures leave out and the videos show: US Dollar next to Euro with
 * an allowed exchange rate range, positions in each financial state, and one
 * position whose stored rate is out of range.
 */
async function adjustData(db: SQL, offices: Record<string, Office>) {
  await db`insert into "Currency" (iso_code, name, symbol, alignment) values ('USD', 'US Dollar', '$', 'Right') on conflict (iso_code) do nothing`;
  for (const key of ['positions', 'finances', 'bookkeeping'])
    await db`update "Office" set default_currency_id = 'EUR', available_currency_ids = ${'{EUR,USD}'}::text[],
      exchange_rate_ranges = ${RANGES}::text::jsonb where id = ${offices[key].officeId}`;
  // Finances: the first charge is on a registered incoming invoice, which makes its rates read-only; unlink it so
  // that its US Dollar rate can be typed into, as in the video.
  await db`delete from "InvoiceChargeLink" where position_charge_item_id = ${offices.finances.data.item.id}`;
  // Positions list: each financial state on some position (counts as the BE's position cache keeps them).
  const rows = offices.positions.data.rows as Record<string, { id: string }>;
  const states: Record<string, Record<string, number>> = {
    list: { uninvoiced_incoming: 1, uninvoiced_outgoing: 1 },
    alpha: {},
    zulu: { draft_incoming: 1, outstanding_outgoing: 2 },
    normalized: { uninvoiced_incoming: 1, overdue_outgoing: 1 },
    route: { draft_incoming: 1, draft_outgoing: 1 },
    road: { uninvoiced_incoming: 1, draft_incoming: 1, outstanding_outgoing: 1, overdue_outgoing: 1 },
    cancelled: {},
  };
  const columns = ['uninvoiced', 'draft', 'outstanding', 'overdue'].flatMap((s) => [`${s}_incoming`, `${s}_outgoing`]);
  for (const [key, counts] of Object.entries(states)) {
    if (!rows[key]) continue;
    const values = Object.fromEntries(columns.map((c) => [c, counts[c] ?? 0]));
    await db`update "PositionCache" set ${db(values)} where position_id = ${rows[key].id}`;
  }
  // A stored incoming invoice in US Dollar at 2.50: the red triangle in the Financial Status column.
  await db`insert into "Invoice" (id, office_id, position_id, invoice_direction, invoice_type, status, language, currency_notation, exchange_rate)
    values (${crypto.randomUUID()}, ${offices.positions.officeId}, ${rows.list.id}, 'Incoming', 'Invoice', 'Draft', 'en', 'USD', 2.5)`;
  await db`select pg_notify('shippy_application_cache', 'offices')`;
  await db`select pg_notify('shippy_application_cache', 'master-data')`;
}

// ---- screens ----

const visible = (locator: AnyPage, timeout = 20_000) => locator.first().waitFor({ state: 'visible', timeout });
/** Until some input's value contains `text` (inline-edit cells show their values in inputs). */
const inputShows = (page: AnyPage, text: string) =>
  page.waitForFunction((t: string) => [...document.querySelectorAll('input')].some((i) => i.value.includes(t)), text, { timeout: 20_000 });

const SCREENS: Screen[] = [
  {
    name: 'positions',
    office: 'positions',
    path: () => '/platform/logistics/positions',
    ready: async (page, office) => {
      await visible(page.getByText(office.data.rows.list.number, { exact: true }));
      await visible(page.getByText(office.data.rows.road.number, { exact: true }));
    },
  },
  {
    name: 'finances',
    office: 'finances',
    path: (office) => `/platform/logistics/positions/${office.data.position.id}/finances`,
    ready: async (page) => {
      await visible(page.getByText('Grand Total', { exact: true }));
      await inputShows(page, 'E2E Finance Vendor');
    },
  },
  {
    name: 'dialog',
    office: 'finances',
    path: (office) => `/platform/logistics/positions/${office.data.position.id}/finances`,
    ready: async (page) => {
      await visible(page.getByText('Grand Total', { exact: true }));
    },
    // As a user does: hover the charge row so it turns editable, type a rate above the range, leave the field.
    act: async (page, office) => {
      const rate = page.locator(`tr[data-finance-row="${office.data.item.id}"]`).getByRole('textbox', { name: 'Exchange rate' }).first();
      await rate.hover();
      await rate.click();
      await rate.fill('2.5');
      await rate.press('Tab');
      await visible(page.getByText('Exchange rate out of range', { exact: true }));
    },
  },
  {
    name: 'bookkeeping',
    office: 'bookkeeping',
    path: () => '/platform/master-data/office/settings/bookkeeping',
    ready: async (page) => {
      await visible(page.getByText('General Bookkeeping Settings', { exact: true }));
      await visible(page.getByText('US Dollar', { exact: true }));
    },
  },
  {
    name: 'bookkeeping-ranges',
    office: 'bookkeeping',
    path: () => '/platform/master-data/office/settings/bookkeeping',
    ready: async (page) => {
      await visible(page.getByText('Allowed Exchange Rates', { exact: true }));
    },
    scrollToEnd: 'main',
  },
  {
    name: 'invoices',
    office: 'invoices',
    path: () => '/platform/finances/invoices',
    ready: async (page, office) => {
      await visible(page.getByText(office.data.outgoing.invoiceNumber, { exact: true }));
      await visible(page.getByText(office.data.incoming.invoiceNumber, { exact: true }));
    },
  },
];

/** The page as the ToS E2E harness signs in: tokens and the selected Office in localStorage, set before the app runs. */
function signInScript(office: Office) {
  const storage = {
    'auth-custom-jwt-accessToken': office.token,
    'auth-custom-jwt-accessTokenExpiresAt': String(office.expiresAt),
    'auth-custom-jwt-userType': 'employee',
    'user-store': JSON.stringify({
      version: 3,
      state: { officeId: office.officeId, selectedOffice: { id: office.officeId, name: office.officeName }, selectedTool: 'tosPlatform' },
    }),
  };
  return `(() => { if (sessionStorage.getItem('__fidelitySignedIn')) return; for (const [k, v] of Object.entries(${JSON.stringify(storage)})) localStorage.setItem(k, v); sessionStorage.setItem('__fidelitySignedIn', '1'); })();`;
}

/** Network idle, fonts, two frames and a pause: what is left of animations and late layout. */
async function settle(page: AnyPage, ms: number) {
  await page.waitForLoadState('networkidle', { timeout: 15_000 }).catch(() => {});
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(undefined)))));
  await page.waitForTimeout(ms);
}

type Browsers = {
  chromium: { browser: AnyPage; version: string; userAgent: string };
  pyro?: { browser: AnyPage; version: string };
};

async function launch(tos: string, bin: string | undefined, channel: string): Promise<Browsers> {
  const playwright = await import(join(tos, 'apps/fe/node_modules/playwright/index.mjs'));
  const chromium = await playwright.chromium.launch({ headless: true, ...(channel === 'headless-shell' ? {} : { channel }) });
  const probe = await chromium.newPage();
  const userAgent = (await probe.evaluate(() => navigator.userAgent)).replace('HeadlessChrome', 'Chrome');
  await probe.close();
  if (!bin) return { chromium: { browser: chromium, version: chromium.version(), userAgent } };
  const wright = await import(join(repo, 'packages/pyro/src/wright/index.ts'));
  const pyro = await wright.chromium.launch({ bin });
  return { chromium: { browser: chromium, version: chromium.version(), userAgent }, pyro: { browser: pyro, version: pyro.version?.() ?? '' } };
}

async function capture(kind: 'chromium' | 'pyro', browsers: Browsers, screen: Screen, office: Office, app: string, file: string, settleMs: number) {
  const userAgent = browsers.chromium.userAgent;
  const context =
    kind === 'chromium'
      ? await browsers.chromium.browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: DSF, userAgent, timezoneId: TIME_ZONE, locale: 'en-US' })
      : await browsers.pyro!.browser.newContext({ viewport: VIEWPORT, userAgent });
  try {
    await context.addInitScript(signInScript(office));
    const page = await context.newPage();
    if (kind === 'pyro') {
      // deviceScaleFactor is not a pyro context option yet: CDP, before the first navigation.
      const cdp = await context.newCDPSession(page);
      await cdp.send('Emulation.setDeviceMetricsOverride', { ...VIEWPORT, deviceScaleFactor: DSF, mobile: false });
    }
    await page.goto(`${app}${screen.path(office)}`, { waitUntil: 'load' });
    await screen.ready(page, office);
    await settle(page, 300);
    if (screen.act) await screen.act(page, office);
    let scrollTop: number | undefined;
    if (screen.scrollToEnd)
      scrollTop = await page.evaluate((selector: string) => {
        // The page's scroller: the first element under `selector` (or it) that scrolls vertically.
        const root = document.querySelector(selector) ?? document.scrollingElement!;
        const scrollers = [root, ...root.querySelectorAll('*')].filter((e) => e.scrollHeight > e.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(e).overflowY));
        const target = (scrollers[0] ?? document.scrollingElement!) as HTMLElement;
        target.scrollTop = target.scrollHeight;
        return target.scrollTop;
      }, screen.scrollToEnd);
    // Neutral pointer and no text caret, the same in both.
    await page.mouse.move(VIEWPORT.width - 2, VIEWPORT.height - 2);
    await page.addStyleTag({ content: '*, *::before, *::after { caret-color: transparent !important; }' });
    await settle(page, settleMs);
    const metrics = await page.evaluate(() => ({ innerWidth, innerHeight, devicePixelRatio, userAgent: navigator.userAgent }));
    const regions = kind === 'chromium' ? await page.evaluate(REGIONS) : undefined;
    const png: Buffer = await page.screenshot({ type: 'png', ...(kind === 'chromium' ? { caret: 'initial', animations: 'allow' } : {}) });
    writeFileSync(file, png);
    return { ...metrics, scrollTop, regions };
  } finally {
    await context.close();
  }
}

// ---- attribution ----

/**
 * Where each known cause of difference can show, from Chromium's DOM at the
 * capture (CSS px rectangles, viewport coordinates): empty inputs with a
 * placeholder, boxes whose text overflows into an ellipsis, `<svg>` icons
 * (those drawing a `<symbol>` through `<use>` without a viewBox of their own
 * apart), and text. `compare` splits each screen's SSIM deficit by them.
 */
const REGIONS = `(() => {
  const rect = (r) => [r.left, r.top, r.width, r.height].map((v) => Math.round(v * 100) / 100);
  const visible = (r) => r.width > 0 && r.height > 0 && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth;
  const out = { placeholder: [], ellipsis: [], iconUseNoViewBox: [], icon: [], text: [] };
  for (const el of document.querySelectorAll('input[placeholder], textarea[placeholder]'))
    if (!el.value && el.placeholder) { const r = el.getBoundingClientRect(); if (visible(r)) out.placeholder.push(rect(r)); }
  for (const el of document.querySelectorAll('body *')) {
    const cs = getComputedStyle(el);
    if (cs.textOverflow === 'ellipsis' && el.scrollWidth > el.clientWidth + 1) { const r = el.getBoundingClientRect(); if (visible(r)) out.ellipsis.push(rect(r)); }
  }
  for (const svg of document.querySelectorAll('svg')) {
    if (svg.parentElement && svg.parentElement.closest('svg')) continue;
    const r = svg.getBoundingClientRect(); if (!visible(r)) continue;
    (svg.querySelector('use') && !svg.getAttribute('viewBox') ? out.iconUseNoViewBox : out.icon).push(rect(r));
  }
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (!n.textContent.trim()) continue;
    range.selectNodeContents(n);
    for (const r of range.getClientRects()) if (visible(r)) out.text.push(rect(r));
  }
  // Text inside inputs (values) is no text node: the input's box.
  for (const el of document.querySelectorAll('input, textarea'))
    if (el.value) { const r = el.getBoundingClientRect(); if (visible(r)) out.text.push(rect(r)); }
  return out;
})()`;

/** Painted last wins: a pixel belongs to the first of these that covers it. */
const CAUSES = ['placeholder', 'ellipsis', 'iconUseNoViewBox', 'icon', 'text'] as const;

/**
 * Each cause's share of the screen's SSIM deficit (the sum of 1 - SSIM over the
 * full-resolution map) and of its changed pixels, pixels assigned by the
 * Chromium regions above (the rest is `other`: boxes, borders, backgrounds).
 */
function attribute(regions: Record<string, number[][]> | undefined, map: Float32Array, mapWidth: number, mapHeight: number, a: Image, b: Image) {
  if (!regions) return undefined;
  const { width, height } = a;
  const owner = new Uint8Array(width * height).fill(CAUSES.length);
  for (let c = CAUSES.length - 1; c >= 0; c--)
    for (const [x, y, w, h] of regions[CAUSES[c]] ?? []) {
      const x0 = Math.max(0, Math.floor(x * DSF)), y0 = Math.max(0, Math.floor(y * DSF));
      const x1 = Math.min(width, Math.ceil((x + w) * DSF)), y1 = Math.min(height, Math.ceil((y + h) * DSF));
      for (let yy = y0; yy < y1; yy++) owner.fill(c, yy * width + x0, yy * width + x1);
    }
  const deficit = new Float64Array(CAUSES.length + 1);
  const changed = new Float64Array(CAUSES.length + 1);
  // The map's pixel (x, y) is the window centred on image pixel (x + 5, y + 5).
  for (let y = 0; y < mapHeight; y++)
    for (let x = 0; x < mapWidth; x++) deficit[owner[(y + 5) * width + x + 5]] += 1 - map[y * mapWidth + x];
  for (let i = 0, n = width * height; i < n; i++) {
    const o = i * 4;
    if (Math.max(Math.abs(a.data[o] - b.data[o]), Math.abs(a.data[o + 1] - b.data[o + 1]), Math.abs(a.data[o + 2] - b.data[o + 2])) > 16) changed[owner[i]]++;
  }
  const totalDeficit = deficit.reduce((x, y) => x + y, 0) || 1;
  const totalChanged = changed.reduce((x, y) => x + y, 0) || 1;
  return Object.fromEntries(
    [...CAUSES, 'other'].map((name, i) => [name, { deficitShare: deficit[i] / totalDeficit, changedShare: changed[i] / totalChanged, count: regions[name]?.length ?? 0 }]),
  );
}

// ---- comparison ----

type Row = {
  screen: string;
  ssimFull: number;
  ssim100: number;
  mad: number;
  changedShare: number;
  size: string;
  causes?: Record<string, { deficitShare: number; changedShare: number; count: number }>;
};

function compare(dir: string, screen: string, regions?: Record<string, number[][]>): Row {
  const read = (name: string): Image => decodePng(readFileSync(join(dir, screen, `${name}.png`)));
  const chromium = read('chromium');
  const pyro = read('pyro');
  if (chromium.width !== pyro.width || chromium.height !== pyro.height)
    throw new Error(`${screen}: sizes differ, chromium ${chromium.width}x${chromium.height}, pyro ${pyro.width}x${pyro.height}`);
  const la = luminance(chromium);
  const lb = luminance(pyro);
  const fullMap = ssim(la, lb, chromium.width, chromium.height);
  const full = fullMap.mean;
  const causes = attribute(regions, fullMap.map, fullMap.width, fullMap.height, chromium, pyro);
  const a3 = downscale(la, chromium.width, chromium.height, DSF);
  const b3 = downscale(lb, chromium.width, chromium.height, DSF);
  const view = ssim(a3.plane, b3.plane, a3.width, a3.height).mean;
  const { mad, changedShare } = meanAbsoluteDifference(chromium, pyro);
  const diff = diffHeatmap(chromium, pyro);
  const side = sideBySide([chromium, pyro]);
  const out = (name: string, image: Image) => writeFileSync(join(dir, screen, `${name}.png`), encodePng(image, 3));
  out('diff', diff);
  out('side', side);
  out('diff-small', downscaleImage(diff, DSF));
  out('side-small', downscaleImage(side, DSF));
  return { screen, ssimFull: full, ssim100: view, mad, changedShare, size: `${chromium.width}x${chromium.height}`, causes };
}

function table(rows: Row[]) {
  const lines = [
    'screen               SSIM full  SSIM 100%    MAD  changed>16  size',
    ...rows.map(
      (r) =>
        `${r.screen.padEnd(20)} ${r.ssimFull.toFixed(4).padStart(9)}  ${r.ssim100.toFixed(4).padStart(9)}  ${r.mad.toFixed(2).padStart(5)}  ${(r.changedShare * 100).toFixed(2).padStart(9)}%  ${r.size}`,
    ),
  ];
  const withCauses = rows.filter((r) => r.causes);
  if (withCauses.length) {
    const names = [...CAUSES, 'other'];
    lines.push('', 'share of the SSIM deficit (full resolution) by where it is, from Chromium\'s DOM:');
    lines.push(`${'screen'.padEnd(20)} ${names.map((n) => n.padStart(18)).join('')}`);
    for (const r of withCauses) lines.push(`${r.screen.padEnd(20)} ${names.map((n) => `${(r.causes![n].deficitShare * 100).toFixed(1)}%`.padStart(18)).join('')}`);
  }
  return lines.join('\n');
}

// ---- main ----

async function main() {
  const compareOnly = opt('--compare');
  if (compareOnly) {
    const report = JSON.parse(readFileSync(join(compareOnly, 'report.json'), 'utf8'));
    const rows = (report.screens as { name: string; chromium?: { regions?: Record<string, number[][]> } }[]).map((s) => compare(compareOnly, s.name, s.chromium?.regions));
    writeFileSync(join(compareOnly, 'report.json'), JSON.stringify({ ...report, rows }, null, 2));
    console.log(table(rows));
    return;
  }
  const tos = opt('--tos');
  if (!tos) throw new Error('--tos <ToS checkout> is required (see the header of bench/fidelity/shippy.ts)');
  const port = Number(opt('--port', '47300'));
  // `--browsers chromium` captures Chromium alone (no comparison).
  const withPyro = opt('--browsers', 'chromium,pyro')!.split(',').includes('pyro');
  const bin = resolve(opt('--bin', join(repo, 'target/release/pyro'))!);
  if (withPyro && !existsSync(bin)) throw new Error(`no pyro binary at ${bin} (cargo build --release)`);
  const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
  const out = resolve(opt('--out', join(homedir(), 'Library/Caches/lightningpanda/fidelity', stamp))!);
  const only = opt('--screens')?.split(',');
  const screens = only ? SCREENS.filter((s) => only.includes(s.name)) : SCREENS;
  const settleMs = Number(opt('--settle', '1000'));
  const stateFile = opt('--stack');

  let stack: StackRuntime;
  let stop: (() => Promise<void>) | undefined;
  if (stateFile) stack = JSON.parse(readFileSync(stateFile, 'utf8'));
  else ({ runtime: stack, stop } = await startStack(tos, port));
  const onSignal = async () => {
    await stop?.();
    process.exit(130);
  };
  process.on('SIGINT', onSignal);
  let browsers: Browsers | undefined;
  try {
    const offices = await createOffices(stack, stateFile ? `-${stamp}` : '');
    const db = await runDatabase(stack, offices.positions.officeId);
    await adjustData(db, offices);
    await db.close();
    browsers = await launch(tos, withPyro ? bin : undefined, opt('--chromium-channel', 'chromium')!);
    const captured: Record<string, unknown>[] = [];
    for (const screen of screens) {
      mkdirSync(join(out, screen.name), { recursive: true });
      const office = offices[screen.office];
      // Back to back per screen, so relative times on the screen ("Last saved: Now") read the same.
      const c = await capture('chromium', browsers, screen, office, stack.app, join(out, screen.name, 'chromium.png'), settleMs);
      const p = withPyro ? await capture('pyro', browsers, screen, office, stack.app, join(out, screen.name, 'pyro.png'), settleMs) : undefined;
      captured.push({ name: screen.name, chromium: c, pyro: p });
      console.log(`captured ${screen.name}: chromium ${c.innerWidth}x${c.innerHeight}@${c.devicePixelRatio}${p ? `, pyro ${p.innerWidth}x${p.innerHeight}@${p.devicePixelRatio}` : ''}${c.scrollTop !== undefined ? `, scrollTop ${c.scrollTop} / ${p?.scrollTop}` : ''}`);
    }
    const tosCommit = Bun.spawnSync(['git', '-C', tos, 'rev-parse', 'HEAD']).stdout.toString().trim();
    const pyroCommit = Bun.spawnSync(['git', '-C', repo, 'rev-parse', 'HEAD']).stdout.toString().trim();
    const meta = { date: new Date().toISOString(), chromium: browsers.chromium.version, userAgent: browsers.chromium.userAgent, pyroBin: bin, pyroCommit, tosCommit, viewport: VIEWPORT, deviceScaleFactor: DSF };
    writeFileSync(join(out, 'report.json'), JSON.stringify({ meta, screens: captured }, null, 2));
    await browsers.chromium.browser.close();
    await browsers.pyro?.browser.close();
    browsers = undefined;
    await stop?.();
    stop = undefined;
    if (!withPyro) return console.log(out);
    const rows = screens.map((s, i) => compare(out, s.name, (captured[i].chromium as { regions?: Record<string, number[][]> }).regions));
    writeFileSync(join(out, 'report.json'), JSON.stringify({ meta, screens: captured, rows }, null, 2));
    console.log(table(rows));
    console.log(`\n${out}`);
  } finally {
    await browsers?.chromium.browser.close().catch(() => {});
    await browsers?.pyro?.browser.close().catch(() => {});
    await stop?.();
  }
}

await main();
process.exit(0);
