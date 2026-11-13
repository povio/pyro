// omitBackground: page.screenshot, locator.screenshot and toHaveScreenshot capture
// a transparent default background (alpha 0), as Chromium does, and the opaque
// default comes back for the next capture. Also passes on stock Playwright.
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { decodePng } from '../../src/wright/snapshot';
import { expect, test } from './fixtures';

const PAGE =
  '<style>html,body{margin:0;background:transparent}</style>' +
  '<div style="margin:20px;width:40px;height:40px;background:rgb(255,0,0);border-radius:20px"></div>';

const pixel = (png: Buffer, x: number, y: number) => {
  const img = decodePng(png)!;
  const i = (y * img.width + x) * 4;
  return [...img.data.slice(i, i + 4)];
};

test('page.screenshot({ omitBackground }) leaves the background transparent, then restores it', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(PAGE);
  const clip = { x: 0, y: 0, width: 80, height: 80 };
  const clear = await page.screenshot({ omitBackground: true, clip });
  expect(pixel(clear, 5, 5)[3]).toBe(0); // outside the box: no background
  expect(pixel(clear, 40, 40)).toEqual([255, 0, 0, 255]); // the box itself
  const opaque = await page.screenshot({ clip });
  expect(pixel(opaque, 5, 5)).toEqual([255, 255, 255, 255]); // the override is gone again
});

test('locator.screenshot({ omitBackground }) keeps the corners of a rounded box transparent', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(PAGE);
  const clear = await page.locator('div').screenshot({ omitBackground: true });
  expect(pixel(clear, 1, 1)[3]).toBe(0);
  expect(pixel(clear, 20, 20)).toEqual([255, 0, 0, 255]);
  expect(pixel(await page.locator('div').screenshot(), 1, 1)).toEqual([255, 255, 255, 255]);
});

test('toHaveScreenshot({ omitBackground }) compares the transparent capture', async ({ page }, info) => {
  test.skip(!!process.env.PW_ENGINE, 'baselines come from our renderer');
  await page.goto('/simple');
  await page.setContent(PAGE);
  const file = info.snapshotPath('rounded.png', { kind: 'screenshot' });
  mkdirSync(dirname(file), { recursive: true });
  const baseline = await page.locator('div').screenshot({ omitBackground: true });
  expect(pixel(baseline, 1, 1)[3]).toBe(0);
  writeFileSync(file, baseline);
  try {
    await expect(page.locator('div')).toHaveScreenshot('rounded.png', { omitBackground: true });
    // Without it the corners are white: a mismatch.
    const e = await expect(page.locator('div')).toHaveScreenshot('rounded.png', { timeout: 1000 }).catch((e: Error) => e);
    expect(e).toBeInstanceOf(Error);
  } finally {
    for (const ext of ['.png', '.json', '.actual.png', '.diff.png']) rmSync(file.replace(/\.png$/, ext), { force: true });
  }
});
