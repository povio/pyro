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

  // Tailwind v4's transition utilities: var() timing functions and durations.
  const TAILWIND = `<style>
    @property --tw-ease { syntax: "*"; inherits: false; }
    @property --tw-duration { syntax: "*"; inherits: false; }
    :root { --default-transition-timing-function: cubic-bezier(0.4, 0, 0.2, 1); --default-transition-duration: 150ms; }
    div { transition-property: opacity; }
    #a { transition-timing-function: var(--tw-ease, var(--default-transition-timing-function)); transition-duration: 150ms; }
    #b { transition-timing-function: var(--default-transition-timing-function); transition-duration: 150ms; }
    #c { transition-timing-function: ease-in; transition-duration: var(--default-transition-duration); }
    #d { transition-timing-function: var(--nope); transition-duration: var(--tw-duration, var(--default-transition-duration)); }
    .on { opacity: 0.2; }
  </style><div id="a">a</div><div id="b">b</div><div id="c">c</div><div id="d">d</div>`;

  test('a transition gets the computed values of var() timing functions and durations', async ({ page }) => {
    await page.setContent(TAILWIND);
    const rows = await page.evaluate(async () => {
      const ids = ['a', 'b', 'c', 'd'];
      for (const id of ids) {
        const el = document.getElementById(id)!;
        void el.offsetWidth;
        el.classList.add('on');
        void getComputedStyle(el).opacity;
      }
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      return ids.map((id) => {
        const el = document.getElementById(id)!;
        const cs = getComputedStyle(el);
        const list = el.getAnimations().map((a) => `${a.constructor.name} ${(a.effect as KeyframeEffect).getTiming().easing} ${(a.effect as KeyframeEffect).getTiming().duration}`);
        return [cs.transitionTimingFunction, cs.transitionDuration, list];
      });
    });
    expect(rows).toEqual([
      ['cubic-bezier(0.4, 0, 0.2, 1)', '0.15s', ['CSSTransition cubic-bezier(0.4, 0, 0.2, 1) 150']],
      ['cubic-bezier(0.4, 0, 0.2, 1)', '0.15s', ['CSSTransition cubic-bezier(0.4, 0, 0.2, 1) 150']],
      ['ease-in', '0.15s', ['CSSTransition ease-in 150']],
      // var(--nope) without a fallback: invalid at computed-value time, the initial `ease`.
      ['ease', '0.15s', ['CSSTransition ease 150']],
    ]);
  });

  test('getComputedStyle reports transitions and animations as Chromium serializes them', async ({ page }) => {
    await page.setContent(`<style>
      @keyframes spin { to { transform: rotate(1turn) } }
      #s { transition: opacity 150ms cubic-bezier(.4,0,.2,1) 10ms; }
      #l { transition-property: opacity, transform, color; transition-duration: 1s, 2s; transition-timing-function: step-start; }
      #n { animation: spin 1s linear infinite; }
      #v { --d: 2s; transition: color var(--d) ease-out; animation-name: spin; animation-duration: var(--d); }
      </style><div id="s"></div><div id="l"></div><div id="i" style="transition: opacity 150ms ease-in"></div>
      <div id="n"></div><div id="v"></div><div id="x"></div>`);
    const read = (id: string) =>
      page.evaluate((id) => {
        const cs = getComputedStyle(document.getElementById(id)!);
        return [cs.transition, cs.transitionProperty, cs.transitionDuration, cs.transitionTimingFunction, cs.transitionDelay, cs.transitionBehavior, cs.animation, cs.animationName, cs.animationDuration, cs.animationIterationCount];
      }, id);
    expect(await read('s')).toEqual(['opacity 0.15s cubic-bezier(0.4, 0, 0.2, 1) 0.01s', 'opacity', '0.15s', 'cubic-bezier(0.4, 0, 0.2, 1)', '0.01s', 'normal', 'none', 'none', '0s', '1']);
    expect((await read('l')).slice(0, 5)).toEqual(['opacity 1s steps(1, start), transform 2s steps(1, start), color 1s steps(1, start)', 'opacity, transform, color', '1s, 2s', 'steps(1, start)', '0s']);
    expect((await read('i'))[0]).toBe('opacity 0.15s ease-in');
    expect((await read('n')).slice(6)).toEqual(['1s linear infinite spin', 'spin', '1s', 'infinite']);
    expect(await read('v')).toEqual(['color 2s ease-out', 'color', '2s', 'ease-out', '0s', 'normal', '2s spin', 'spin', '2s', '1']);
    // Nothing declared: the initial values (transition-property's is `all`).
    expect(await read('x')).toEqual(['all', 'all', '0s', 'ease', '0s', 'normal', 'none', 'none', '0s', '1']);
  });

  test('an easing no engine can compute leaves the other elements listed', async ({ page }) => {
    await page.setContent(`<style>
      #a { transition: opacity 100s var(--bad, steps(0)); }
      #b { transition: opacity 100s ease-out; }
      .on { opacity: 0.5 }</style><div id="a"></div><div id="b"></div>`);
    const out = await page.evaluate(async () => {
      for (const id of ['a', 'b']) {
        const el = document.getElementById(id)!;
        void el.offsetWidth;
        el.classList.add('on');
      }
      void document.body.offsetWidth;
      return ['a', 'b'].map((id) => {
        try {
          return document.getElementById(id)!.getAnimations().map((a) => (a.effect as KeyframeEffect).getTiming().easing).join();
        } catch (e) {
          return String(e);
        }
      });
    });
    // #a's transition declaration is invalid at computed-value time: no transition.
    expect(out).toEqual(['', 'ease-out']);
  });
});
