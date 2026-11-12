# Quick start

Three short starts, one per way of using Pyro. Each is complete: copy the files, run the command, and it passes. None of them needs a web server, because the browser serves a local directory in place of a real origin with `--static`.

Install the package first ([Installation](/guide/installation)) in an empty directory:

```sh
mkdir lp-start && cd lp-start
bun init -y
bun add --dev @povio/lightning-panda
```

All three starts use the same small app. Save it as `site/index.html`:

```html
<!doctype html>
<html>
<head><title>Todo</title></head>
<body>
  <h1>Todos</h1>
  <p id="user"></p>
  <label>New todo <input id="new" /></label>
  <button id="add">Add</button>
  <ul id="list"></ul>
  <script>
    const token = localStorage.getItem('token');
    document.getElementById('user').textContent = token ? 'Signed in as ' + token : 'Signed out';
    document.getElementById('add').addEventListener('click', async () => {
      const input = document.getElementById('new');
      const li = document.createElement('li');
      li.textContent = input.value;
      await new Promise((r) => setTimeout(r, 20)); // pretend to save
      document.getElementById('list').append(li);
      input.value = '';
    });
  </script>
</body>
</html>
```

## 1. Run a Playwright test unchanged

A Playwright test runs on Pyro when it imports from `@povio/lightning-panda/playwright` instead of `@playwright/test`. Nothing else changes, and Playwright is not installed.

`playwright.config.ts`:

```ts
import { defineConfig } from '@povio/lightning-panda/playwright';

export default defineConfig({
  testDir: './e2e',
  use: {
    baseURL: 'http://app.test',
    // Serve ./site in place of http://app.test.
    lightningpanda: { args: ['--static', `http://app.test=${import.meta.dirname}/site`] },
  },
});
```

`e2e/todo.spec.ts`:

```ts
import { test, expect } from '@povio/lightning-panda/playwright'; // was '@playwright/test'

test('adds a todo', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveTitle('Todo');
  await page.getByLabel('New todo').fill('Buy milk');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('listitem')).toHaveText(['Buy milk']);
});
```

Run it as you would run `playwright test`:

```sh
bunx lightningpanda test
```

```text
  ✓   1 e2e/todo.spec.ts › adds a todo (398ms)

  1 passed (0.6s)
```

`lightningpanda test` takes Playwright's usual flags (`-c`, `--project`, `-j`, `-g`, `--retries`, `-u`, ...) and needs Bun on `PATH`. The browser starts once per worker. Next: [Migrating a suite](/playwright/migration).

## 2. Write a native `page.run` test

The native client sends a whole scenario as one command. The browser runs every step on its own thread, waits where it needs to, and replies once. This test also uses a **profile**: pages of the app are booted ahead, already signed in through `localStorage`, before the test asks for one.

`tests/todo.test.ts`:

```ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Lightningpanda, button, check, label, role } from '@povio/lightning-panda';

let lp: Lightningpanda;

beforeAll(async () => {
  // Starts the bundled browser on a free port. It exits with this process.
  lp = await Lightningpanda.launch({
    args: ['--static', `http://app.test=${import.meta.dirname}/../site`],
  });
  // Pages of this profile boot ahead with a token in localStorage.
  await lp.profile('user', { url: 'http://app.test/', localStorage: { token: 'ada' } });
});
afterAll(() => lp.close());

test('adds a todo', async () => {
  await using page = await lp.page({ as: 'user' });
  const r = await page.run([
    check(role('paragraph')).text('Signed in as ada'),
    label('New todo').fill('Buy milk'),
    button('Add').click(),               // settles: waits for the 20 ms timer
    check(role('listitem')).count(1),
    role('listitem').read('first'),
  ]);
  expect(r.values.first).toBe('Buy milk');
});
```

```sh
bun test tests/todo.test.ts
```

There is no `await` per step and no polling. `button('Add').click()` waits until the page is quiet before the next step starts, so the check sees the new item. If a step never holds, `page.run` throws a `RunError` naming the step and what it was waiting for. Next: [Your first test](/e2e/first-test).

::: tip
`await using` closes the page when the test ends. An open page keeps its connection from booting the next page for the profile, so always close pages.
:::

## 3. Save a signed-in profile and scrape

The same client works outside a test runner. A profile is a named recipe for a page: where it boots and what is in its `localStorage` (or init scripts) before the app runs. Every page you take from it starts signed in.

`scrape.ts`:

```ts
import { Lightningpanda, check, css, evaluate } from '@povio/lightning-panda';

const lp = await Lightningpanda.launch({
  args: ['--static', `http://app.test=${import.meta.dirname}/site`],
});
try {
  await lp.profile('signed-in', { url: 'http://app.test/', localStorage: { token: 'ada' } });

  const page = await lp.page({ as: 'signed-in' });
  const r = await page.run([
    check(css('#user')).text('Signed in as ada'),  // waits until the app has rendered it
    evaluate(
      `({ title: document.title, user: document.querySelector('#user').textContent })`,
      'data',
    ),
  ]);
  console.log(r.values.data);
  page.close();
} finally {
  lp.close();
}
```

```sh
bun scrape.ts
```

```text
{
  title: "Todo",
  user: "Signed in as ada",
}
```

`evaluate(expression, name)` stores the JSON value of a page expression under `r.values[name]`; a returned promise is awaited. To scrape a real site, drop `--static` and use its URL in the profile. The client also runs under Node 22+ from a `.mjs` file. Next: [Scraping and automation](/scraping/overview).

## Where to go next

- [Choosing an API](/guide/choosing-an-api): which of the three to use, and how to mix them.
- [Settling](/guide/settling): the exact rule that replaces waits and sleeps.
- [Profiles](/guide/profiles) and [Fork mode](/guide/fork-mode): the two features that remove app boot time from tests.
