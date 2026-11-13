// Run from an isolated directory containing the installed npm tarball.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const require = createRequire(resolve('package.json'));
const { Pyro, check, css, evaluate, goto } = await import(pathToFileURL(require.resolve('@povio/pyro')).href);
const browser = await Pyro.launch();
try {
  const page = await browser.page();
  const result = await page.run([
    goto('data:text/html,<html><body><div id="ready">npm package works</div></body></html>'),
    check(css('#ready')).visible(),
    evaluate('document.querySelector("#ready").textContent', 'text'),
  ]);
  assert.equal(result.values.text, 'npm package works');
  const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  assert.equal(Buffer.from(screenshot.data, 'base64').subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
} finally { browser.close(); }
