import { test } from '../../../src/wright/index';

test.describe('a beforeAll that asks for a page', () => {
  test.beforeAll(async ({ page }) => {
    await page.goto('about:blank');
  });
  test.afterAll(async () => {
    console.log('AFTERALL runs after the failed beforeAll');
  });
  test('first', async () => {});
  test('second', async () => {});
});

test.describe('a beforeAll with a test fixture', () => {
  let got = '';
  test.beforeAll(async ({ request }) => {
    got = typeof request.get;
  });
  test('gets its own instance', async () => {
    if (got !== 'function') throw new Error(`beforeAll got ${got}`);
  });
});

test('a test outside the describes runs', async () => {});
