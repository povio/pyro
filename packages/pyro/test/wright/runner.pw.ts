// The test runner's API: fixtures, hooks, options, steps, test.info(), and
// expect's soft/poll/toPass/extend/asymmetric matchers. Plain Playwright code:
// it passes on stock Playwright's runner too.
import { expect, test as base } from './fixtures';

const log: string[] = [];

type Fx = { greeting: string; counter: { n: number }; auto: void; shout: string; dep: string };
const test = base.extend<Fx, { workerThing: { id: number } }>({
  greeting: ['hello', { option: true }],
  counter: async ({}, use) => {
    await use({ n: 0 });
  },
  workerThing: [async ({}, use) => use({ id: Math.random() }), { scope: 'worker' }],
  auto: [async ({}, use) => {
    log.push('auto');
    await use();
  }, { auto: true }],
  shout: async ({ greeting }, use) => use(greeting.toUpperCase()),
  dep: async ({ shout, counter }, use) => {
    counter.n++;
    await use(`${shout}!`);
    log.push('dep-teardown');
  },
});

// Overriding a fixture: the override gets the original under the same name.
const louder = test.extend<{ shout: string }>({
  shout: async ({ shout }, use) => use(`${shout}${shout}`),
});

let seenWorker: number | undefined;

test.describe('fixtures', () => {
  // One worker in order, on Playwright's runner too (the config is fully parallel).
  test.describe.configure({ mode: 'serial' });
  test.beforeEach(async ({ counter }) => {
    counter.n = 10;
    log.push('beforeEach');
  });
  test.afterEach(async () => {
    log.push('afterEach');
  });

  test('test fixtures, options, dependencies and auto fixtures', async ({ dep, counter, greeting, workerThing }) => {
    expect(greeting).toBe('hello');
    expect(dep).toBe('HELLO!');
    // beforeEach saw the same counter, then `dep` was set up after it.
    expect(counter.n).toBe(10 + 0 + (counter.n - 10));
    expect(log).toContain('auto');
    seenWorker = workerThing.id;
  });

  test('a worker fixture is shared across tests', async ({ workerThing }) => {
    expect(workerThing.id).toBe(seenWorker!);
  });

  test.describe('with use', () => {
    test.use({ greeting: 'bonjour' });
    test('test.use sets an option for its describe', async ({ shout }) => {
      expect(shout).toBe('BONJOUR');
    });
  });
});

louder('an override extends the original fixture', async ({ shout }) => {
  expect(shout).toBe('HELLOHELLO');
});

test.describe('hooks', () => {
  test.describe.configure({ mode: 'serial' });
  const order: string[] = [];
  test.beforeAll(async () => {
    order.push('beforeAll');
  });
  test.beforeEach(async () => {
    order.push('beforeEach');
  });
  test.afterEach(async () => {
    order.push('afterEach');
  });
  test.afterAll(async () => {
    expect(order).toEqual(['beforeAll', 'beforeEach', 'first', 'afterEach', 'beforeEach', 'second', 'afterEach']);
  });
  test('first', async () => {
    order.push('first');
  });
  test('second', async () => {
    order.push('second');
  });
});

test.describe('annotations', () => {
  test('test.skip() inside a test stops it', async () => {
    test.skip(true, 'not here');
    throw new Error('unreachable');
  });
  test('test.fail() expects the test to fail', async () => {
    test.fail();
    expect(1).toBe(2);
  });
  test('test.slow() triples the timeout', async () => {
    const before = test.info().timeout;
    test.slow();
    expect(test.info().timeout).toBe(before * 3);
  });
  test('test.info() describes the running test', async ({}, info) => {
    expect(info.title).toBe('test.info() describes the running test');
    expect(info.titlePath.slice(-2)).toEqual(['annotations', 'test.info() describes the running test']);
    expect(info.file).toMatch(/runner\.pw\.ts$/);
    expect(test.info()).toBe(info);
    await info.attach('note', { body: 'hello', contentType: 'text/plain' });
    expect(info.attachments.map((a) => a.name)).toContain('note');
    expect(info.outputPath('x.txt')).toContain('x.txt');
  });
  test('test.step runs its body and returns its value', async () => {
    const v = await test.step('compute', async () => 42);
    expect(v).toBe(42);
  });
});

test.describe('retries', () => {
  test.describe.configure({ retries: 2 });
  test('a failing attempt is retried', async ({}, info) => {
    if (info.retry < 1) throw new Error('flaky');
  });
});

test.describe('expect', () => {
  test('generic matchers and asymmetric matchers', async () => {
    expect({ a: 1, b: 'xyz', c: [1, 2, 3] }).toEqual({ a: expect.any(Number), b: expect.stringContaining('y'), c: expect.arrayContaining([2]) });
    expect([1, 2]).toHaveLength(2);
    expect(() => {
      throw new Error('boom');
    }).toThrow('boom');
    await expect(Promise.resolve(3)).resolves.toBe(3);
    expect(5).not.toBe(6);
  });
  test('expect.poll retries a value', async () => {
    let n = 0;
    await expect.poll(() => ++n, { intervals: [10] }).toBeGreaterThan(3);
    await expect.poll(async () => 'done').not.toBe('pending');
  });
  test('toPass retries a block', async () => {
    let n = 0;
    await expect(async () => {
      n++;
      expect(n).toBeGreaterThan(2);
    }).toPass({ intervals: [10] });
  });
  test('expect.extend adds matchers', async () => {
    const ext = expect.extend({
      toBeEven(received: number) {
        const pass = received % 2 === 0;
        return { pass, message: () => `expected ${received} ${this.isNot ? 'not ' : ''}to be even` };
      },
    });
    (ext(4) as any).toBeEven();
    (ext(5) as any).not.toBeEven();
  });
  test('a custom message prefixes the failure', async () => {
    const error = await (async () => {
      try {
        expect(1, 'the answer').toBe(2);
      } catch (e) {
        return e as Error;
      }
    })();
    expect(error?.message).toContain('the answer');
  });
  test('expect.soft fails the test at the end, not at once', async () => {
    test.fail();
    expect.soft(1).toBe(2);
    log.push('after soft');
    expect(log).toContain('after soft');
  });
});

test.describe('request', () => {
  test('the request fixture talks to baseURL', async ({ request }) => {
    const r = await request.get('/api/data.json', { params: { name: 'Ada' } });
    expect(r.ok()).toBe(true);
    await expect(r).toBeOK();
    expect(await r.json()).toEqual({ greeting: 'Hello, Ada' });
    expect(r.headers()['content-type']).toContain('application/json');
    const created = await request.post('/tos-api/positions', { headers: { authorization: 'Bearer e2e-token' }, data: {} });
    expect(created.status()).toBe(201);
    expect((await created.json()).number).toMatch(/^EXP-/);
    const denied = await request.post('/tos-api/positions', { data: {} });
    await expect(denied).not.toBeOK();
  });
  test('playwright.request.newContext', async ({ playwright, baseURL }) => {
    const ctx = await playwright.request.newContext({ baseURL, extraHTTPHeaders: { authorization: 'Bearer e2e-token' } });
    const r = await ctx.post('/tos-api/positions');
    expect(r.status()).toBe(201);
    await ctx.dispose();
  });
});
