import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

const { chromium } = createRequire(new URL('../../bench/package.json', import.meta.url))('playwright-core');
const [executablePath, modulePath] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage();
  if (modulePath) {
    const source = await readFile(modulePath, 'utf8');
    await page.evaluate(`globalThis.OfflineAudioContext = (${source})({}).OfflineAudioContext`);
  }
  const result = await page.evaluate(async () => {
    const frame = document.body.appendChild(document.createElement('iframe'));
    const foreign = frame.contentWindow;
    const context = new OfflineAudioContext(1, 128, 8192);
    const node = context.createWaveShaper();
    const assign = value => {
      try { node.curve = value; return 'accepted'; } catch (error) { return error.name; }
    };
    const table = foreign.Float32Array.of(-9, -1, 0.375, 1, 9).subarray(1, 4);
    const wrong = foreign.Uint32Array.of(1, 2);
    Object.defineProperty(wrong, Symbol.toStringTag, { value: 'Float32Array' });
    const distinctRealm = !(table instanceof Float32Array);
    const invalid = [assign(wrong), assign([0, 1]), assign(new foreign.DataView(new foreign.ArrayBuffer(8))),
      assign(new foreign.Float32Array(new foreign.ArrayBuffer(8, { maxByteLength: 16 })))];
    const short = assign(new foreign.Float32Array(1));
    const valid = assign(table);
    table.fill(0);
    frame.remove();
    const snapshot = node.curve && Array.from(node.curve);
    node.connect(context.destination);
    const sample = (await context.startRendering()).getChannelData(0)[0];
    return { distinctRealm, invalid, short, valid, snapshot, sample, clear: assign(null), repeat: assign(Float32Array.of(0, 1)) };
  });
  console.log(JSON.stringify(result));
  assert.equal(result.distinctRealm, true);
  assert.deepEqual(result.invalid, Array(4).fill('TypeError'));
  assert.equal(result.short, modulePath ? 'InvalidStateError' : 'InvalidAccessError');
  assert.equal(result.valid, 'accepted');
  assert.deepEqual(result.snapshot, [-1, 0.375, 1]);
  assert.equal(result.sample, 0.375);
  assert.equal(result.clear, 'accepted');
  assert.equal(result.repeat, modulePath ? 'InvalidStateError' : 'accepted');
} finally { await browser.close(); }
