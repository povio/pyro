// Compile-only checks (`bun run test:types`): what the types accept and,
// with @ts-expect-error, what they reject.
import { chromium, expect, test, type FullConfig, type FullProject, type Locator, type Page, type WorkerInfo } from '../../src/wright/index.js';

test('types', async ({ page, request }) => {
  const l: Locator = page.getByRole('button', { name: 'Go' });
  // @ts-expect-error not an ARIA role
  page.getByRole('buton');
  await expect(l).toHaveText('Go');
  await expect(l).not.toBeVisible({ timeout: 10 });
  // @ts-expect-error a Locator matcher on a value
  await expect(1).toHaveText('1');
  await expect(page).toHaveURL(/x/);
  // @ts-expect-error a Page matcher on a Locator
  await expect(l).toHaveURL(/x/);
  await expect(await request.get('/')).toBeOK();
  expect(1).toBe(1);
  // @ts-expect-error toBeGreaterThan takes a number
  expect(1).toBeGreaterThan('2');
  await expect.poll(() => 1).toBe(1);
  await expect(async () => {}).toPass();
  // @ts-expect-error toPass needs a function
  await expect(1).toPass();
  expect('x').toMatchSnapshot('x.txt');
  expect(Buffer.from('x')).toMatchSnapshot({ name: 'x.png', maxDiffPixels: 2 });
  const ext = expect.extend({ toBeEven(r: number, by?: number) { return { pass: r % (by ?? 2) === 0, message: () => '' }; } });
  ext(4).toBeEven();
  ext(4).not.toBeEven(3);
  // @ts-expect-error toBeEven takes a number
  ext(4).toBeEven('x');
  const n: number = await page.evaluate(() => 1);
  const s: string = await page.evaluate((a) => a.x, { x: 'y' });
  // @ts-expect-error evaluate returns a number here
  const bad: string = await page.evaluate(() => 1);
  // @ts-expect-error deprecated, removed
  await l.type('x');
  // @ts-expect-error deprecated, removed
  page.accessibility;
  // Deprecated options compile, with Playwright's meaning (none, for these).
  await l.isVisible({ timeout: 1500 });
  await l.isHidden({ timeout: 1500 });
  await l.click({ noWaitAfter: true });
  await l.fill('x', { noWaitAfter: true });
  await l.press('Enter', { noWaitAfter: true });
  await l.check({ noWaitAfter: true });
  await l.selectOption('a', { noWaitAfter: true });
  await l.setInputFiles([], { noWaitAfter: true });
  await l.dragTo(l, { noWaitAfter: true });
  const h = await l.elementHandle();
  await h?.inputValue({ timeout: 10 });
  await h?.press('Enter', { noWaitAfter: true });
  await page.context().browser()?.newPage({ logger: { isEnabled: () => false, log: () => {} } });
  await page.clock.install({ time: new Date('2024-01-01') });
  await page.clock.runFor('01:00');
  // @ts-expect-error ticks are a number or "hh:mm:ss"
  await page.clock.fastForward(new Date());
  const yaml: string = await l.ariaSnapshot({ depth: 2 });
  const ai: string = await page.ariaSnapshot({ mode: 'ai', depth: 3 }) + (await l.ariaSnapshot({ mode: 'default' }));
  await page.locator('aria-ref=e5').click();
  // @ts-expect-error mode is 'ai' or 'default'
  await l.ariaSnapshot({ mode: 'codegen' });
  await expect(l).toMatchAriaSnapshot(`- button "Go"`);
  await expect(l).toMatchAriaSnapshot({ name: 'go.aria.yml' });
  // @ts-expect-error aria snapshots are for locators
  await expect(page).toMatchAriaSnapshot('- button');
  const cov = await page.coverage.stopJSCoverage();
  const firstUrl: string | undefined = cov[0]?.url;
  void [yaml, ai, firstUrl];
  const context = page.context();
  await context.grantPermissions(['geolocation'] as const, { origin: 'https://example.com' });
  await context.setGeolocation({ latitude: 46.05, longitude: 14.51 });
  await context.setGeolocation(null);
  await context.setOffline(true);
  // @ts-expect-error setOffline takes a boolean
  await context.setOffline();
  const el = await page.$('div');
  const tag: string | undefined = await el?.evaluate((e) => e.tagName);
  void [n, s, bad, tag, (p: Page) => p];
});

// A global setup as Playwright's docs write it, with the import rewritten.

export default async function globalSetup(config: FullConfig): Promise<void> {
  const { baseURL, storageState } = config.projects[0].use;
  const browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(baseURL!);
  await page.context().storageState({ path: storageState as string });
  await browser.close();
  const project: FullProject = config.projects[0];
  const counts: number[] = [config.workers, project.retries, project.timeout, project.repeatEach];
  const names: string[] = [...project.dependencies, project.teardown ?? '', config.rootDir, config.configFile ?? ''];
  const shard: { current: number; total: number } | null = config.shard;
  // @ts-expect-error not a `use` option
  config.projects[0].use.notAnOption;
  void [counts, names, shard];
}

test.extend<{}, { session: string }>({
  session: [async ({}, use, workerInfo: WorkerInfo) => use(`${workerInfo.config.workers}:${workerInfo.project.name}`), { scope: 'worker' }],
});
test('testInfo carries the FullConfig', async ({}, info) => {
  const workers: number = info.config.workers;
  const deps: string[] = info.project.dependencies;
  void [workers, deps];
});
