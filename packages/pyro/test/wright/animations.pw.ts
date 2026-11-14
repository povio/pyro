// Animated values reach the cascade: computed styles and boxes read Web
// Animations, CSS animations and the objects that control them where they
// are, and a CSS transition a style change starts is listed at once.
// Passes on stock Playwright too.
import { expect, test } from './fixtures';

test.describe('animations', () => {
  test('element.animate: computed style and box follow currentTime', async ({ page }) => {
    await page.setContent('<div id="box" style="width:100px;height:50px;background:rgb(0,0,255)"></div>');
    const read = () =>
      page.evaluate(() => {
        const box = document.getElementById('box')!;
        return [getComputedStyle(box).opacity, box.getBoundingClientRect().width];
      });
    await page.evaluate(() => {
      const box = document.getElementById('box')!;
      const anim = box.animate([{ width: '100px', opacity: 0 }, { width: '300px', opacity: 1 }], { duration: 1000 });
      anim.pause();
      anim.currentTime = 500;
      (window as any).anim = anim;
    });
    expect(await read()).toEqual(['0.5', 200]);
    await page.evaluate(() => ((window as any).anim.currentTime = 250));
    expect(await read()).toEqual(['0.25', 150]);
    await page.evaluate(() => (window as any).anim.cancel());
    expect(await read()).toEqual(['1', 100]);
  });

  test('a paused CSS animation holds where its negative delay put it', async ({ page }) => {
    await page.setContent(
      '<style>@keyframes grow { from { width: 100px } to { width: 200px } }' +
        ' #box { height: 20px; animation: grow 4s linear -1s paused }</style><div id="box"></div>',
    );
    const state = await page.evaluate(() => {
      const box = document.getElementById('box')!;
      const anim = box.getAnimations()[0];
      return [anim.playState, box.getBoundingClientRect().width];
    });
    expect(state).toEqual(['paused', 125]);
    // Script seeking the CSSAnimation moves the box.
    const seeked = await page.evaluate(() => {
      const box = document.getElementById('box')!;
      box.getAnimations()[0].currentTime = 2000;
      return box.getBoundingClientRect().width;
    });
    expect(seeked).toBe(175);
  });

  test('getAnimations lists a transition right after the change that starts it', async ({ page }) => {
    await page.setContent(
      '<style>#box { width: 100px; height: 20px; transition: width 100s linear -50s }</style><div id="box"></div>',
    );
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    const listed = await page.evaluate(() => {
      const box = document.getElementById('box')!;
      box.style.width = '300px';
      const list = document.getAnimations();
      return [list.length, list[0].constructor.name, (list[0] as CSSTransition).transitionProperty];
    });
    expect(listed).toEqual([1, 'CSSTransition', 'width']);
  });
});
