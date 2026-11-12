# Concurrency at scale

To scrape many pages at once you need to know where the parallelism comes from. In Pyro it comes from connections: each CDP connection is a browser thread with its own JavaScript engine, and pages on one connection take turns on that thread. This page explains the model, shows a worker pool with one job per identity at a time, and covers timeouts, retries, memory and measuring.

## The model

```
pyro serve
├── connection 1  ── browser thread + JavaScriptCore engine
│     ├── context A (alice)  ── page
│     └── context B (bob)    ── page
├── connection 2  ── browser thread + engine
│     └── context C          ── page, page
└── ...up to --cdp-max-connections
```

| Level | What it is | Limit |
| --- | --- | --- |
| Server | One `pyro serve` process. | Run as many as you like, on different ports. |
| Connection | One WebSocket client: `chromium.connectOverCDP()`, `chromium.launch()`, `Pyro.connect()`, `puppeteer.connect()`. One OS thread and one JavaScript engine. | `--cdp-max-connections`, default 16. |
| Browser context | An isolated session: cookies, storage, cache, its own network client. | 256 per connection. |
| Page | A tab in a context. | 256 per connection. |

What follows from it:

- **Pages on one connection share a CPU core.** They run concurrently, interleaved on one thread, which is fine while they wait for the network. For CPU-heavy pages (large single-page apps), open more connections, about one per core.
- **The connection limit queues, it does not refuse.** At `--cdp-max-connections` live connections the server stops accepting new ones until one closes; the rest wait in the kernel's backlog. HTTP requests to `/json/version` take a slot too, so a health check waits while the server is full. Raise the limit to what you need: `pyro serve --cdp-max-connections 64`.
- **Closing a connection disposes everything in it.** All contexts and pages of a connection are closed when it closes.
- **In fork mode every connection is a process**, forked from a template that booted your profiles once. Each `pyro.page()` of the native SDK opens its own connection, so the connection limit is the limit on pages open at once. See [Profiles booted once (fork)](/scraping/fork-profiles).

## Memory

Every open page holds a full copy of its app: its DOM, its JavaScript heap, its styles. Memory grows with the pages you keep open, not with the pages you have visited:

- Closing a context or page frees its memory, and the server returns freed memory to the operating system right away. A 20-page loop on a 2 MB app measured a plateau of 275 MB with this release and 323 MB without it. `PYRO_RELEASE_ON_CLOSE=0` turns the release off, for comparisons only.
- Close each context when its job is done. A job that leaves its context open leaks a whole page.
- In fork mode, a page shares its memory with the template until it writes to it, and its process exits when the connection closes. Nothing accumulates across jobs.
- With `--prepare` or SDK profiles, each pooled page is one more copy of the app, booted ahead. `--prepare-refill after-close` caps the pool at its size.

Measure your own sites; app size dominates everything else.

## A worker pool with identities

This pool runs jobs on N connections to one server. Each job names the stored profile it runs as ([Stored profiles](/scraping/profiles)), and an identity never runs two jobs at once, so a site never sees one account in two places at the same moment. Failed jobs are retried with backoff.

Start the server, then run the pool:

```sh
pyro serve --port 9222 --cdp-max-connections 32
```

```ts
// pool.ts
import { chromium, type Browser } from '@povio/pyro/wright';
import { existsSync, readFileSync } from 'node:fs';

const ENDPOINT = process.env.PYRO_ENDPOINT ?? 'http://127.0.0.1:9222';
const WORKERS = Number(process.env.WORKERS ?? 4);
const MAX_ATTEMPTS = 3;
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

type Job = { profile: string; url: string; attempts: number };
type Result = { profile: string; url: string; names: string[]; ms: number };

const BASE = 'http://127.0.0.1:4500';
const queue: Job[] = [];
for (let p = 1; p <= 5; p++) for (const profile of ['alice', 'bob']) queue.push({ profile, url: `${BASE}/products?page=${p}`, attempts: 0 });

const busy = new Set<string>(); // identities with a job running
const results: Result[] = [];
const failed: { job: Job; error: string }[] = [];

function contextOptions(profile: string) {
  const sidecar = `profiles/${profile}.identity.json`;
  const identity = existsSync(sidecar) ? JSON.parse(readFileSync(sidecar, 'utf8')) : {};
  const { locale, timezoneId, proxy, proxyBypass, ...options } = identity; // these need CDP or a server flag
  return { ...options, storageState: `profiles/${profile}.json` };
}

/** The next job whose identity is free. */
function take(): Job | undefined {
  const i = queue.findIndex((job) => !busy.has(job.profile));
  return i < 0 ? undefined : queue.splice(i, 1)[0];
}

async function runJob(browser: Browser, job: Job): Promise<Result> {
  const started = performance.now();
  const context = await browser.newContext(contextOptions(job.profile));
  try {
    const page = await context.newPage();
    page.setDefaultTimeout(10_000);
    await page.goto(job.url, { timeout: 20_000 });
    if (page.url().includes('/login')) throw new Error(`profile ${job.profile} is signed out`);
    const names = await page.locator('.product .name').allTextContents();
    return { profile: job.profile, url: job.url, names, ms: Math.round(performance.now() - started) };
  } finally {
    await context.close(); // frees the page's memory on the server
  }
}

async function worker() {
  const browser = await chromium.connectOverCDP(ENDPOINT); // one connection: one browser thread
  try {
    while (queue.length || busy.size) {
      const job = take();
      if (!job) {
        await sleep(20); // every remaining job's identity is busy elsewhere
        continue;
      }
      busy.add(job.profile);
      try {
        results.push(await runJob(browser, job));
      } catch (e) {
        job.attempts++;
        if (job.attempts < MAX_ATTEMPTS) {
          await sleep(500 * 2 ** job.attempts); // back off before retrying
          queue.push(job);
        } else failed.push({ job, error: (e as Error).message });
      } finally {
        busy.delete(job.profile);
      }
    }
  } finally {
    await browser.close(); // disconnects; the server keeps running
  }
}

const t0 = performance.now();
await Promise.all(Array.from({ length: WORKERS }, worker));
const seconds = (performance.now() - t0) / 1000;
console.log(`${results.length} jobs, ${failed.length} failed, ${(results.length / seconds).toFixed(1)} jobs/s`);
for (const f of failed) console.error('failed', f.job.url, f.error);
```

```
$ bun pool.ts
10 jobs, 0 failed, 213.5 jobs/s
```

That figure is for a tiny local test page; real sites are bounded by their response times and by your rate limits. Notes on the design:

- **One connection per worker** gives each worker its own thread on the server. The workers are async functions in one Node.js or Bun process; that process only sends commands and is rarely the bottleneck.
- **A fresh context per job** isolates jobs: nothing a page stored leaks into the next job, and closing the context frees everything.
- **The `busy` set** keeps one job per identity. Drop it if your identities may run in parallel.
- **The signed-out check** turns an expired session into a failed job instead of scraping the login page. Combine it with `openProfile` from [Stored profiles](/scraping/profiles#a-profile-store) to sign in again.
- **Per-site limits.** For crawling sites you do not own, add a per-host limiter (at most N requests in flight and M per second per host) around `runJob`.

To spread load over several machines, run a server on each and give each worker an endpoint from the list.

## Native SDK and fork mode

The native SDK spreads pages over connections itself:

```ts
import { Pyro, css, goto } from '@povio/pyro';

const pyro = await Pyro.connect('ws://127.0.0.1:9222/', { connections: 4 });
const urls = Array.from({ length: 20 }, (_, i) => `http://127.0.0.1:4500/products?page=${(i % 5) + 1}`);
const titles = await Promise.all(
  urls.map(async (url) => {
    await using page = await pyro.page(); // on the connection with the fewest open pages
    return (await page.run([goto(url), css('h1').read('h')])).values.h;
  }),
);
pyro.close();
```

`Promise.all` over thousands of URLs opens thousands of pages at once. Put a limiter in front (a semaphore, or the queue pattern above) to keep the number of open pages near the number of connections times a small factor.

Against `pyro serve --fork`, each `pyro.page({ as })` is its own forked process with the profile's page already booted; `connections` does not matter there. Raise `--cdp-max-connections` to the number of pages you open at once. The fork example in [Profiles booted once](/scraping/fork-profiles#sign-in-once-fork-per-job) runs eight signed-in jobs at once.

## Timeouts

Set a deadline at every level, so one stuck page cannot hold a worker forever:

| Where | Setting |
| --- | --- |
| Playwright library, per call | `{ timeout }` on `goto`, `click`, `waitFor` and the rest |
| Playwright library, per page | `page.setDefaultTimeout(ms)`, `page.setDefaultNavigationTimeout(ms)` |
| Native SDK | `page.run(steps, { timeout })` (per step, default 5 s), or `timeout` on a single step |
| Whole job | your own: race the job against a timer, then close its context, or its connection if the page no longer answers |

When a job times out, close its context (or, in fork mode, its page). That frees the page and its memory.

::: warning A page stuck in JavaScript blocks its connection
Step and call timeouts stop waiting; they do not interrupt JavaScript that never returns. In a test with `evaluate('for (;;) {}')` and a 1 s step timeout, the run never replied and the connection's thread stayed at 100% CPU until the client disconnected. Closing the connection stopped it at once, and the server and its other connections were unaffected. So the last resort for a job that does not finish is to close its connection (`browser.close()`, `pyro.close()`, or the fork page) and open a new one. Keep the jobs of one connection few enough that losing it costs little.
:::

## Retries

- Retry network errors (`net::ERR_...`), timeouts and `5xx` responses, with exponential backoff.
- Do not retry `4xx` responses other than `429`; they will not change. On `429`, honor `Retry-After` and slow the whole host down, not just the job.
- Retry in a fresh context, never in the page that failed.
- Cap the attempts and record what failed, with the reason.

## Measuring

- **Time per job** (`performance.now()` around it) and jobs per second for the whole run.
- **Time in the browser:** the native SDK's `run()` reply has `ms`. The difference to your wall-clock time is network latency and your own code.
- **Server memory:** `ps -o rss= -p <pid>` gives the resident set in KB. On macOS, `footprint <pid>` is closer to what the system charges the process. In fork mode, measure the template and the forks separately; forks share pages with the template, so their RSS overstates their real cost.
- **CPU:** if the server's threads are at 100% of a core each, add connections or servers; if they idle while jobs are slow, the sites are the bottleneck, so more parallelism means more load on them.
- **SDK pool statistics:** `pyro.stats` counts pages taken from pools (`prepared`), pages that were still booting when taken (`preparedBooting`) and pages booted on demand (`fresh`).

Read [Performance](/guide/performance) for the numbers measured on the test suites, and [Running as a service](/scraping/service) for running the server long-term.
