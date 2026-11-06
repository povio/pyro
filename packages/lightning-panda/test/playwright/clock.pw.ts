// page.clock: fake Date and timers, paused, fast-forwarded and run, kept
// across navigations. Passes on stock Playwright too.
import { expect, test } from './fixtures';

const T0 = new Date('2024-02-02T08:00:00Z').getTime();

test.describe('clock', () => {
  test('install, pauseAt and fastForward', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 3_600_000);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 3_600_000);
    expect(await page.evaluate(() => new Date().toISOString())).toBe('2024-02-02T09:00:00.000Z');
    await page.evaluate(() => {
      (window as any).fired = [];
      setTimeout(() => (window as any).fired.push('5s'), 5_000);
      setTimeout(() => (window as any).fired.push('2m'), 120_000);
    });
    await page.clock.fastForward('00:30');
    expect(await page.evaluate(() => (window as any).fired)).toEqual(['5s']);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 3_600_000 + 30_000);
    await page.clock.fastForward('02:00');
    expect(await page.evaluate(() => (window as any).fired)).toEqual(['5s', '2m']);
  });

  test('runFor fires every timer in order, intervals included', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await page.evaluate(() => {
      const w = window as any;
      w.log = [];
      const id = setInterval(() => w.log.push(`tick${Date.now() % 10_000}`), 1000);
      setTimeout(() => w.log.push('once'), 2500);
      setTimeout(() => clearInterval(id), 3500);
    });
    await page.clock.runFor(5000);
    await expect(page.clock.pauseAt(T0)).rejects.toThrow(/past/);
    expect(await page.evaluate(() => (window as any).log)).toEqual(['tick2000', 'tick3000', 'once', 'tick4000']);
  });

  test('a paused clock holds timers back until it runs', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 60_000);
    await page.setContent('<p id="p">waiting</p><script>setTimeout(() => (p.textContent = "done"), 1000)</script>');
    await expect(page.locator('#p')).toHaveText('waiting');
    await page.clock.runFor(1000);
    await expect(page.locator('#p')).toHaveText('done');
  });

  test('setFixedTime keeps Date fixed while timers run', async ({ page }) => {
    await page.clock.setFixedTime(T0);
    await page.goto('/simple');
    expect(await page.evaluate(() => Date.now())).toBe(T0);
    expect(await page.evaluate(() => new Promise((r) => setTimeout(() => r(Date.now()), 20)))).toBe(T0);
  });

  test('setSystemTime moves the time; the clock keeps it across navigations', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await page.clock.setSystemTime(T0 + 86_400_000);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 86_400_000);
    await page.goto('/simple');
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 86_400_000);
    await page.clock.resume();
    const later = await page.evaluate(() => new Promise<number>((r) => setTimeout(() => r(Date.now()), 50)));
    expect(later).toBeGreaterThanOrEqual(T0 + 86_400_000 + 50);
    expect(later).toBeLessThan(T0 + 86_400_000 + 5_000);
  });

  test('fastForward skips a timer an earlier one cleared and fires an interval once', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await page.evaluate(() => {
      const w = window as any;
      w.log = [];
      setTimeout(() => {
        w.log.push('a');
        clearTimeout(w.b);
      }, 100);
      w.b = setTimeout(() => w.log.push('b'), 200);
      setInterval(() => w.log.push('i'), 300);
    });
    await page.clock.fastForward(1000);
    expect(await page.evaluate(() => (window as any).log)).toEqual(['a', 'i']);
  });

  test('ticks drive timers; setSystemTime and setFixedTime only move Date', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await page.clock.setSystemTime(T0 + 86_400_000);
    await page.clock.setFixedTime(T0 - 86_400_000);
    await page.evaluate(() => setTimeout(() => ((window as any).fired = Date.now()), 100));
    await page.clock.runFor(500);
    expect(await page.evaluate(() => (window as any).fired)).toBe(T0 - 86_400_000);
    expect(await page.evaluate(() => Date.now())).toBe(T0 - 86_400_000);
    // install sets the time again and ends the fixed time.
    await page.clock.install({ time: T0 });
    await page.clock.runFor(1000);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1000);
  });

  test('performance.now and Event.timeStamp follow ticks, not the wall time', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    const p1 = await page.evaluate(() => performance.now());
    await page.clock.setSystemTime(T0 + 86_400_000);
    expect(await page.evaluate(() => performance.now())).toBe(p1);
    await page.clock.setFixedTime(T0 - 86_400_000);
    expect(await page.evaluate(() => performance.now())).toBe(p1);
    await page.clock.runFor(500);
    expect(await page.evaluate(() => performance.now())).toBe(p1 + 500);
    expect(await page.evaluate(() => new Event('x').timeStamp)).toBe(p1 + 500);
  });

  test('a timer set during a tick waits at least 1ms', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await page.evaluate(() => {
      const w = window as any;
      w.n = 0;
      const loop = () => {
        w.n++;
        setTimeout(loop, 0);
      };
      setTimeout(loop, 0);
    });
    await page.clock.runFor(10);
    expect(await page.evaluate(() => (window as any).n)).toBe(11);
  });

  test('negative ticks are rejected', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await expect(page.clock.runFor(-1)).rejects.toThrow(/Negative ticks are not supported/);
    await expect(page.clock.fastForward(-1)).rejects.toThrow(/Cannot fast-forward to the past/);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1000);
  });

  test('every method installs the clock when needed', async ({ page }) => {
    await page.goto('/simple');
    await page.clock.pauseAt(T0);
    expect(await page.evaluate(() => Date.now())).toBe(T0);
    await page.evaluate(() => setTimeout(() => ((window as any).fired = true), 1000));
    await page.clock.runFor(1000);
    expect(await page.evaluate(() => (window as any).fired)).toBe(true);
  });

  test('runFor rejects with the first exception a timer threw, and still runs the rest', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    const errors: Error[] = [];
    page.on('pageerror', (e) => errors.push(e));
    await page.evaluate(() => {
      setTimeout(() => {
        throw new Error('boom1');
      }, 10);
      setTimeout(() => {
        throw new Error('boom2');
      }, 20);
      setTimeout(() => ((window as any).after = true), 30);
    });
    await expect(page.clock.runFor(100)).rejects.toThrow(/boom1/);
    expect(await page.evaluate(() => (window as any).after)).toBe(true);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1100);
    expect(errors).toEqual([]);
  });

  test('Intl.DateTimeFormat and AbortSignal.timeout follow the clock', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 3_600_000);
    expect(
      await page.evaluate(() => {
        const f = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
        return [f.format(), f.formatToParts().find((p) => p.type === 'hour')!.value];
      }),
    ).toEqual(['09:00', '09']);
    await page.evaluate(() => ((window as any).signal = AbortSignal.timeout(50)));
    expect(await page.evaluate(() => (window as any).signal.aborted)).toBe(false);
    await page.clock.runFor(50);
    expect(await page.evaluate(() => [(window as any).signal.aborted, (window as any).signal.reason.name])).toEqual([true, 'TimeoutError']);
  });

  test('frames already in the page get the clock', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(`<iframe name="inner" srcdoc="<p id=p>in frame</p>"></iframe>`);
    await expect(page.frameLocator('iframe').locator('#p')).toHaveText('in frame');
    await page.clock.install({ time: T0 });
    await page.clock.pauseAt(T0 + 1000);
    const frame = page.frames().find((f) => f !== page.mainFrame())!;
    expect(await frame.evaluate(() => Date.now())).toBe(T0 + 1000);
  });

  test('a page that has not navigated yet gets the clock in its first document', async ({ context }) => {
    await context.clock.install({ time: T0 });
    await context.clock.pauseAt(T0 + 1000);
    const page = await context.newPage();
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1000);
    await page.clock.runFor(500);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1500);
  });

  test('a running clock keeps up with real time across navigations', async ({ page }) => {
    await page.clock.install({ time: T0 });
    await page.goto('/simple');
    await page.clock.runFor(1000);
    await new Promise((r) => setTimeout(r, 300));
    await page.goto('/simple');
    const now = await page.evaluate(() => Date.now());
    expect(now).toBeGreaterThanOrEqual(T0 + 1300);
    expect(now).toBeLessThan(T0 + 10_000);
  });

  test('window.performance is [Replaceable]', async ({ page }) => {
    await page.goto('/simple');
    expect(
      await page.evaluate(() => {
        const original = window.performance;
        const fake = { now: () => 42 };
        (window as any).performance = fake;
        const own = Object.getOwnPropertyDescriptor(window, 'performance')!;
        return [window.performance === fake, performance.now(), own.writable && own.enumerable && own.configurable, original !== fake];
      }),
    ).toEqual([true, 42, true, true]);
  });

  test("a timer's exception while the clock runs goes to the console, not pageerror", async ({ page }) => {
    await page.goto('/simple');
    await page.clock.install({ time: T0 });
    const pageErrors: string[] = [];
    const consoleErrors: string[] = [];
    page.on('pageerror', (e) => pageErrors.push(e.message));
    page.on('console', (m) => m.type() === 'error' && consoleErrors.push(m.text()));
    await page.evaluate(() =>
      setTimeout(() => {
        throw new Error('boomRT');
      }, 10),
    );
    await expect.poll(() => consoleErrors.length).toBe(1);
    expect(consoleErrors[0]).toContain('boomRT');
    await page.waitForTimeout(100);
    expect(pageErrors).toEqual([]);
  });

  test('a new document replays a rejected pauseAt to its time', async ({ page }) => {
    await page.goto('/simple');
    await page.clock.pauseAt(T0 + 1000);
    await expect(page.clock.pauseAt(T0)).rejects.toThrow(/past/);
    expect(await page.evaluate(() => Date.now())).toBe(T0 + 1000);
    await page.goto('/simple');
    expect(await page.evaluate(() => Date.now())).toBe(T0);
  });

  test('the fakes keep the native shapes', async ({ page }) => {
    await page.goto('/simple');
    await page.clock.install({ time: T0 });
    expect(
      await page.evaluate(() => {
        const message = (f: () => unknown) => {
          try {
            f();
            return 'ok';
          } catch (e) {
            return (e as Error).message;
          }
        };
        return [
          Date.length,
          message(() => (setTimeout as any)()),
          message(() => (setInterval as any)()),
          message(() => (setTimeout as any)(undefined, 0)),
          message(() => (requestAnimationFrame as any)()),
        ];
      }),
    ).toEqual([7, 'Callback must be provided to timer calls', 'Callback must be provided to timer calls', 'ok', 'Callback must be provided to requestAnimationFrame calls']);
  });

  test('errors carry the clock.<method> prefix and the in-page error name', async ({ page }) => {
    await page.goto('/simple');
    await page.clock.install({ time: T0 });
    const message = (p: Promise<unknown>) => p.then(() => 'ok', (e: Error) => [e.constructor.name, e.message.split('\n')[0]]);
    expect(await message(page.clock.runFor('1:2'))).toEqual(['Error', "clock.runFor: Clock only understands numbers, 'mm:ss' and 'hh:mm:ss'"]);
    expect(await message(page.clock.setSystemTime('not a date'))).toEqual(['Error', 'clock.setSystemTime: Invalid date: not a date']);
    expect(await message(page.clock.fastForward(3e9))).toEqual(['Error', 'clock.fastForward: Error: Cannot fast-forward to the past']);
    expect(await message(page.clock.runFor(-1))).toEqual(['Error', 'clock.runFor: TypeError: Negative ticks are not supported']);
  });
});
