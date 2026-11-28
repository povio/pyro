import { expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

test('computed percentage transforms are matrices that DOMMatrix can consume', async () => {
  const browser = await Pyro.launch({ bin });
  try {
    await browser.profile('transform', { url: 'data:text/html,' + encodeURIComponent('<!doctype html><div id="target" style="width:200px;height:80px;transform:translate(50%,25%)"></div>') });
    await using page = await browser.page({ as: 'transform' });
    const result = await page.run([evaluate(`(() => {
      const el = document.getElementById('target');
      const matrix = new DOMMatrix(getComputedStyle(el).transform);
      return [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f];
    })()`, 'matrix')]);
    expect(result.values.matrix).toEqual([1, 0, 0, 1, 100, 20]);
    const variants = await page.run([evaluate(`(() => {
      const el = document.getElementById('target');
      el.style.transform = 'translate3d(50%,25%,30px)';
      const m = new DOMMatrix(getComputedStyle(el).transform);
      const three = [m.is2D, m.m41, m.m42, m.m43];
      el.style.transform = 'none';
      const none = getComputedStyle(el).transform;
      el.style.display = 'none';
      el.style.transform = 'translate(50%,25%)';
      return { three, none, hidden: getComputedStyle(el).transform };
    })()`, 'variants')]);
    expect(variants.values.variants).toEqual({ three: [false, 100, 20, 30], none: 'none', hidden: 'matrix(1, 0, 0, 1, 0, 0)' });
    // A display:none element has no box: Chrome resolves the percentages against an empty reference box (d316767f2).
    const animated = await page.run([evaluate(`(() => {
      const el = document.body.appendChild(document.createElement('div'));
      el.style.cssText = 'width:200px;height:80px;transform:translateX(0%);transition:transform 1000s steps(2) -500s';
      getComputedStyle(el).transform;
      el.style.transform = 'translateX(100%)';
      const m = new DOMMatrix(getComputedStyle(el).transform);
      return [m.a, m.d, m.e, m.f];
    })()`, 'matrix')]);
    expect(animated.values.matrix).toEqual([1, 1, 100, 0]);
  } finally { browser.close(); }
});
