// ariaSnapshot({ mode: 'ai' }) and the aria-ref selector: refs, [active],
// [cursor=pointer], generic containers and iframe snapshots. Also passes on
// stock Playwright (the parity run), which checks our AI tree against
// Playwright's own on the same page.
import { expect, test } from './fixtures';

const PAGE = `
  <div><h1>Shop</h1></div>
  <nav aria-label="Main"><a href="/home">Home</a> <a href="/about">About us</a></nav>
  <main>
    <div><div><button id="buy">Buy</button></div></div>
    <span aria-hidden="true">decor</span>
    <div hidden>secret</div>
    <input id="q" placeholder="Search">
    <div id="card" style="cursor:pointer" onclick="this.querySelector('b').textContent='clicked'"><span>Card</span> <b>title</b></div>
    <p style="pointer-events:none">no pointer <button>Inert</button></p>
  </main>`;

const MAIN = [
  '- main [ref=e7]:',
  '  - button "Buy" [ref=e10]',
  '  - text: decor',
  '  - textbox "Search" [ref=e11]',
  '  - generic [ref=e12] [cursor=pointer]: Card title',
  '  - paragraph:',
  '    - text: no pointer',
  '    - button "Inert"',
];

test.describe('aria snapshot, ai mode', () => {
  test('refs, [active], [cursor=pointer] and generic containers', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    // The body is the active element; a generic with one ref'd child gives
    // way to it; aria-hidden but visible text stays; pointer-events:none
    // gets no ref.
    expect(await page.ariaSnapshot({ mode: 'ai' })).toBe(
      [
        '- generic [active] [ref=e1]:',
        '  - heading "Shop" [level=1] [ref=e3]',
        '  - navigation "Main" [ref=e4]:',
        '    - link "Home" [ref=e5] [cursor=pointer]:',
        '      - /url: /home',
        '    - link "About us" [ref=e6] [cursor=pointer]:',
        '      - /url: /about',
        ...MAIN.map((l) => '  ' + l),
      ].join('\n'),
    );
    // Refs stay with their elements; [active] follows focus.
    await page.locator('#q').focus();
    expect(await page.locator('main').ariaSnapshot({ mode: 'ai' })).toBe(MAIN.join('\n').replace('"Search" [ref', '"Search" [active] [ref'));
    // AI mode does not wait for a locator that matches nothing.
    const started = Date.now();
    await expect(page.locator('#nope').ariaSnapshot({ mode: 'ai', timeout: 3000 })).rejects.toThrow(/does not match any element/);
    expect(Date.now() - started).toBeLessThan(1500);
    // The default mode is unchanged.
    expect(await page.locator('main').ariaSnapshot()).not.toContain('[ref=');
  });

  test('depth', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    const snap = await page.ariaSnapshot({ mode: 'ai', depth: 2 });
    expect(snap).toContain('  - main [ref=e7]:\n    - button "Buy" [ref=e10]');
    expect(snap).toMatch(/ {4}- paragraph$/);
  });

  test('aria-ref finds the element of a ref', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    const snap = await page.ariaSnapshot({ mode: 'ai' });
    const ref = (name: string) => new RegExp(`${name} \\[ref=(e\\d+)\\]`).exec(snap)![1];
    await expect(page.locator(`aria-ref=${ref('button "Buy"')}`)).toHaveText('Buy');
    await page.locator(`aria-ref=${ref('textbox "Search"')}`).fill('shoes');
    await expect(page.locator('#q')).toHaveValue('shoes');
    await page.locator(`aria-ref=${ref('generic')}`).click();
    await expect(page.locator('#card')).toHaveText('Card clicked');
    expect(await page.locator('aria-ref=e999').count()).toBe(0);
    // A new snapshot keeps the refs.
    expect(await page.ariaSnapshot({ mode: 'ai' })).toContain(`button "Buy" [ref=${ref('button "Buy"')}]`);
  });

  test('iframes: nested snapshots and f<seq> refs', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(`<p>top</p><iframe srcdoc="<button onclick=&quot;this.textContent='hit'&quot;>Inner</button><a href=/x>link</a>"></iframe><button>Out</button>`);
    await expect(page.frameLocator('iframe').getByRole('button')).toBeVisible();
    const snap = await page.ariaSnapshot({ mode: 'ai' });
    expect(snap).toMatch(
      new RegExp(
        [
          '^- generic \\[active\\] \\[ref=e\\d+\\]:',
          '  - paragraph \\[ref=e\\d+\\]: top',
          '  - iframe \\[ref=(e\\d+)\\]:',
          '    - generic \\[active\\] \\[ref=(f\\d+)e1\\]:',
          '      - button "Inner" \\[ref=\\2e2\\]',
          '      - link "link" \\[ref=\\2e3\\] \\[cursor=pointer\\]:',
          '        - /url: /x',
          '  - button "Out" \\[ref=e\\d+\\]$',
        ].join('\n'),
      ),
    );
    const frame = /ref=(f\d+)e1/.exec(snap)![1];
    const iframe = /iframe \[ref=(e\d+)\]/.exec(snap)![1];
    // At the depth limit the iframe's snapshot is cut, as in Playwright.
    expect(await page.ariaSnapshot({ mode: 'ai', depth: 1 })).toContain(`  - iframe [ref=${iframe}]:\n    \n  - button "Out"`);
    // A frame ref is found from the page.
    await page.locator(`aria-ref=${frame}e2`).click();
    await expect(page.frameLocator('iframe').getByRole('button')).toHaveText('hit');
    await expect(page.locator(`aria-ref=${iframe}`).contentFrame().getByRole('link')).toHaveText('link');
    // A locator inside the frame snapshots with the frame's prefix.
    expect(await page.frameLocator('iframe').getByRole('link').ariaSnapshot({ mode: 'ai' })).toBe(`- link "link" [ref=${frame}e3] [cursor=pointer]:\n  - /url: /x`);
  });
});
