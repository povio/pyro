# Assertions (expect)

`expect` from `@povio/pyro/wright` has Playwright's API. Web-first matchers on locators, pages and API responses retry until they pass or time out. Every other matcher is Bun's, which follows Jest as Playwright's do. `.not`, `.soft`, `.poll`, `.configure`, `.extend`, `toPass` and the asymmetric matchers work as in Playwright.

```ts
import { test, expect } from '@povio/pyro/wright';

test('cart', async ({ page }) => {
  await page.goto('/cart');
  await expect(page).toHaveTitle(/Cart/);
  await expect(page.getByRole('listitem')).toHaveCount(3);
  await expect(page.getByTestId('total')).toHaveText('$42.00');
  await expect.soft(page.getByRole('button', { name: 'Pay' })).toBeEnabled();
  expect(await page.getByRole('listitem').allTextContents()).toContain('Socks');
});
```

## How web-first matchers wait

The default timeout is 5 000 ms, or the config's `expect.timeout`. Each matcher takes `{ timeout }`.

There are two ways a locator matcher runs:

- **In the browser.** `toBeVisible`, `toBeHidden`, `toBeEnabled`, `toBeDisabled`, `toBeFocused`, `toBeChecked` and `toHaveCount` always run as one browser-side check, as do `toHaveText` / `toContainText` with a plain string, `toHaveValue` and `toHaveAttribute(name, value)` with a plain string, and `toHaveAccessibleName` with a plain string. The browser re-tests the condition each time the DOM changes and replies once.
- **By polling.** The other matchers, and the ones above with a RegExp, `ignoreCase` or `useInnerText`, read the value in the page and retry at Playwright's intervals (0, 50, 100, 250, 500, 1000 ms).

Failure messages follow Playwright's shape:

```text
Error: expect(locator).toHaveText(expected) failed

Locator: getByTestId('total')
Expected: "$42.00"
Received: $40.00
Timeout: 5000ms

Call log:
  - waiting for getByTestId('total')
```

Matchers that need one element fail with a strict mode violation when the locator matches several.

## Locator matchers

| Matcher | Options | Notes |
| --- | --- | --- |
| `toBeAttached()` | `attached`, `timeout` | |
| `toBeChecked()` | `checked`, `indeterminate`, `timeout` | Checkboxes, radios, and `aria-checked` on ARIA roles. |
| `toBeDisabled()` | `timeout` | |
| `toBeEditable()` | `editable`, `timeout` | |
| `toBeEmpty()` | `timeout` | Empty value for inputs, else no text. |
| `toBeEnabled()` | `enabled`, `timeout` | |
| `toBeFocused()` | `timeout` | |
| `toBeHidden()` | `timeout` | |
| `toBeInViewport()` | `ratio`, `timeout` | |
| `toBeVisible()` | `visible`, `timeout` | |
| `toContainClass(names)` | `timeout` | A string or a list (one per element). |
| `toContainText(text)` | `ignoreCase`, `useInnerText`, `timeout` | A string, RegExp, or a list matched in order. |
| `toHaveAccessibleDescription(text)` | `ignoreCase`, `timeout` | Reads `aria-description`, else `title`. `aria-describedby` is not followed. |
| `toHaveAccessibleName(text)` | `ignoreCase`, `timeout` | A plain string uses the browser's accessible name computation. A RegExp or `ignoreCase` reads `aria-label`, else the text content. |
| `toHaveAttribute(name)` | `timeout` | The attribute exists. |
| `toHaveAttribute(name, value)` | `ignoreCase`, `timeout` | |
| `toHaveClass(value)` | `timeout` | A string, RegExp, or a list. |
| `toHaveCount(n)` | `timeout` | |
| `toHaveCSS(name, value)` | `timeout` | Computed style. |
| `toHaveId(id)` | `timeout` | |
| `toHaveJSProperty(name, value)` | `timeout` | Compared as JSON. |
| `toHaveRole(role)` | `timeout` | Reads the `role` attribute only. Implicit roles (`<button>` is a button) do not match. |
| `toHaveScreenshot(name?, options?)` | see [Snapshots](/playwright/snapshots) | Rendered by Pyro. `.not` is not supported. |
| `toHaveText(text)` | `ignoreCase`, `useInnerText`, `timeout` | A string, RegExp, or a list (one per element, in order). Whitespace is normalized. |
| `toHaveValue(value)` | `timeout` | |
| `toHaveValues(values)` | `timeout` | Selected options of a `<select multiple>`. |
| `toMatchAriaSnapshot(template?)` | `name`, `timeout` | See [Clock, coverage, ARIA](/playwright/advanced#aria-snapshots). |

Playwright's `toHaveAccessibleErrorMessage` is not implemented.

## Page matchers

| Matcher | Notes |
| --- | --- |
| `toHaveTitle(text)` | String or RegExp. |
| `toHaveURL(url)` | String (resolved against `baseURL`), RegExp, or a predicate `(url: URL) => boolean`. Option `ignoreCase`. Rechecked on each URL change, including `pushState`. |
| `toHaveScreenshot(name?, options?)` | See [Snapshots](/playwright/snapshots). |

`expect(frame)` has no web-first matchers.

## API response matchers

```ts
const r = await request.post('/api/orders', { data: { sku: 'A1' } });
await expect(r).toBeOK();       // status 200-299
await expect(r).not.toBeOK();
```

A failure prints the status and up to 1 000 characters of the response body.

## Generic matchers

Any matcher not listed above goes to Bun's `expect`: `toBe`, `toEqual`, `toStrictEqual`, `toContain`, `toMatch`, `toMatchObject`, `toHaveLength`, `toHaveProperty`, `toThrow`, `toBeGreaterThan`, `toBeCloseTo`, `toBeNull`, `toBeTruthy`, `.resolves`, `.rejects`, and the rest. They are synchronous, as in Playwright.

The asymmetric matchers come from Bun too: `expect.any`, `expect.anything`, `expect.arrayContaining`, `expect.objectContaining`, `expect.stringContaining`, `expect.stringMatching`, `expect.closeTo`. Also `expect.assertions`, `expect.hasAssertions`, `expect.getState` and `expect.addSnapshotSerializer`.

```ts
expect(order).toEqual({ id: expect.any(String), items: expect.arrayContaining([{ sku: 'A1' }]) });
```

Bun's matchers follow Jest closely, but messages and a few edge cases differ from Playwright's own copies. `toMatchSnapshot` is not Bun's here: it is Playwright's file snapshot (see [Snapshots](/playwright/snapshots#tomatchsnapshot)).

## Custom messages

```ts
await expect(page.getByRole('alert'), 'the save confirmation').toBeVisible();
expect(total, { message: 'cart total' }).toBe(42);
```

The message is prefixed to the error.

## expect.soft

```ts
await expect.soft(page.getByTestId('name')).toHaveText('Ada');
await expect.soft(page.getByTestId('email')).toHaveText('ada@example.com');
```

A soft failure is recorded and the test goes on. At the end the test fails with the first error, or with `N soft assertions failed:` and every message.

## expect.poll

```ts
await expect.poll(async () => {
  const r = await page.request.get('/api/jobs/1');
  return (await r.json()).state;
}, { timeout: 10_000, intervals: [250, 500] }).toBe('done');
```

The function is called until the matcher passes. Defaults: the `expect` timeout and intervals `[100, 250, 500, 1000]`. Any matcher works, `.not` included.

## toPass

```ts
await expect(async () => {
  const r = await page.request.get('/api/health');
  expect(r.status()).toBe(200);
}).toPass({ timeout: 15_000, intervals: [1_000] });
```

The block is retried until it does not throw. The default timeout is 0, which means it retries until the test times out, as in Playwright. `.not.toPass()` is not supported.

## expect.configure

```ts
const slowExpect = expect.configure({ timeout: 20_000 });
const softExpect = expect.configure({ soft: true });
await slowExpect(page.getByText('Report ready')).toBeVisible();
```

Takes `timeout`, `soft` and `message`.

## expect.extend

```ts
import { expect as base, type Locator } from '@povio/pyro/wright';

export const expect = base.extend({
  async toHaveAmount(locator: Locator, expected: number) {
    const text = await locator.textContent();
    const amount = Number(text?.replace(/[^0-9.]/g, ''));
    return {
      pass: amount === expected,
      message: () => `expected ${expected}${this.isNot ? ' not' : ''}, got ${amount}`,
    };
  },
});

await expect(page.getByTestId('total')).toHaveAmount(42);
```

A matcher gets `this.isNot`, `this.timeout` and `this.utils` (`printReceived`, `printExpected`, `stringify`). It may be async. Matchers are typed from their arguments, as in Playwright. At run time `extend` adds to the shared `expect`, so the matchers work everywhere once the module that extends it is loaded; for types, import the extended `expect`.

`expect(locator).not.toHaveAmount(1)` works at run time but is a type error: on a locator, the type of `.not` has only the built-in locator matchers.

## Next

- Locators and what they find: [Locators and selectors](/playwright/locators).
- Visual assertions: [Screenshots and snapshots](/playwright/snapshots).
