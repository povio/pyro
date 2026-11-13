# A batch of URLs to PDF

You have a list of URLs and want one A4 PDF per page: print styles applied, web fonts embedded, error pages skipped, and every file checked before you trust it. This recipe does that with the native SDK (`@povio/pyro`), a few pages at a time.

## Files

```
pdf/
  site.ts          test site: report pages with a web font and print-only CSS
  fonts/Brand.ttf  any TrueType font of yours
  batch.ts         the batch
  urls.txt         one URL per line
```

```sh
mkdir pdf && cd pdf
bun init -y
bun add @povio/pyro
```

Until `@povio/pyro` is published under that name, install it as [Installation](/guide/installation#try-today) describes.

The check at the end uses `pdftotext` from poppler: `brew install poppler` on macOS, `apt-get install poppler-utils` on Ubuntu.

## The test site

Each `/reports/<n>` page loads a font with `@font-face`, has a 60-row table (two A4 pages), and a line that `@media print` hides. Copy any `.ttf` font to `fonts/Brand.ttf`; the test used Big Caslon from macOS.

::: details site.ts
```ts
// site.ts: report pages to print, with a web font and print-only CSS.
// bun site.ts 4603
const PORT = Number(process.argv[2] ?? 4603);

const report = (id: number) => `<!doctype html><meta charset="utf-8"><title>Report ${id}</title>
<style>
  @font-face { font-family: Brand; src: url(/fonts/Brand.ttf) format('truetype'); }
  body { font-family: Brand, serif; }
  table { border-collapse: collapse; width: 100%; }
  td, th { border: 1px solid #999; padding: 4px 8px; }
  .noprint { color: #c00; }
  @media print { .noprint { display: none } }
</style>
<p class="noprint">Navigation bar: not for print</p>
<h1>Report ${id}</h1>
<p>Reference REG-${id}-be08b23e-89be-48b8</p>
<table>
  <tr><th>Item</th><th>Amount</th></tr>
  ${Array.from({ length: 60 }, (_, i) => `<tr><td>Line ${i + 1}</td><td>${(id * 100 + i).toFixed(2)}</td></tr>`).join('')}
</table>`;

Bun.serve({
  port: PORT,
  hostname: '127.0.0.1',
  fetch(req) {
    const url = new URL(req.url);
    const m = /^\/reports\/(\d+)$/.exec(url.pathname);
    if (m) return new Response(report(Number(m[1])), { headers: { 'content-type': 'text/html' } });
    if (url.pathname === '/fonts/Brand.ttf') return new Response(Bun.file('fonts/Brand.ttf'));
    return new Response('not found', { status: 404 });
  },
});
console.log(`site on http://127.0.0.1:${PORT}`);
```
:::

```sh
bun site.ts 4603
for i in 1 2 3 4 5 6 7 8; do echo "http://127.0.0.1:4603/reports/$i"; done > urls.txt
echo "http://127.0.0.1:4603/reports/missing" >> urls.txt   # a 404, to see a failure
```

## The batch

```ts
// batch.ts: bun batch.ts urls.txt out
// Prints every URL in the list to an A4 PDF, a few at a time, and checks each file's text.
import { Pyro, css, goto } from '@povio/pyro';
import { execFile } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { promisify } from 'node:util';

const [list = 'urls.txt', outDir = 'out'] = process.argv.slice(2);
const CONCURRENCY = Number(process.env.CONCURRENCY ?? 4);
const urls = (await readFile(list, 'utf8')).split('\n').map((l) => l.trim()).filter(Boolean);
await mkdir(outDir, { recursive: true });

// One connection per worker: each is its own browser thread, so pages lay out and print in parallel.
const pyro = await Pyro.launch({ connections: CONCURRENCY });

/** pdftotext -layout (poppler): the text a reader, or a search index, gets out of the PDF. */
async function pdfText(path: string): Promise<string> {
  return (await promisify(execFile)('pdftotext', ['-layout', path, '-'])).stdout;
}

async function printOne(url: string, index: number) {
  const started = performance.now();
  await using page = await pyro.page();
  await page.send('Emulation.setEmulatedMedia', { media: 'print' }); // @media print rules apply
  const nav = await page.run([goto(url)], { timeout: 15_000 });
  const status = nav.requests.find((q) => q.url === url)?.status;
  if (status !== 200) throw new Error(`HTTP ${status}`); // do not print error pages
  const r = await page.run([css('h1').read('title')], { timeout: 15_000 }); // waits until the content is there
  const { data } = await page.send('Page.printToPDF', {
    paperWidth: 8.27, // A4 in inches
    paperHeight: 11.69,
    marginTop: 0.4,
    marginBottom: 0.4,
    marginLeft: 0.4,
    marginRight: 0.4,
    printBackground: true,
  });
  const path = `${outDir}/${String(index + 1).padStart(3, '0')}.pdf`;
  await writeFile(path, Buffer.from(data, 'base64'));

  // Check the output: the title is there as text, print-only CSS took effect.
  const text = await pdfText(path);
  const title = r.values.title as string;
  if (!text.includes(title)) throw new Error(`${path}: title "${title}" missing from the PDF text`);
  if (text.includes('not for print')) throw new Error(`${path}: screen-only content was printed`);
  const pages = (text.match(/\f/g) ?? []).length;
  return { url, path, title, pages, ms: Math.round(performance.now() - started) };
}

// A small worker pool: CONCURRENCY jobs at a time, each failure recorded, none fatal.
const queue = urls.map((url, i) => ({ url, i }));
const done: Awaited<ReturnType<typeof printOne>>[] = [];
const failed: { url: string; error: string }[] = [];
const t0 = performance.now();
try {
  await Promise.all(
    Array.from({ length: CONCURRENCY }, async () => {
      for (let job; (job = queue.shift()); ) {
        try {
          done.push(await printOne(job.url, job.i));
        } catch (e) {
          failed.push({ url: job.url, error: (e as Error).message });
        }
      }
    }),
  );
} finally {
  pyro.close();
}
console.table(done.sort((a, b) => a.path.localeCompare(b.path)));
for (const f of failed) console.error('failed', f.url, f.error);
console.log(`${done.length} PDFs, ${failed.length} failed, ${Math.round(performance.now() - t0)} ms`);
process.exitCode = failed.length ? 1 : 0;
```

## Run it

```
$ bun batch.ts urls.txt out
pyro: CDP server listening on ws://127.0.0.1:55583/
┌───┬─────────────────────────────────┬─────────────┬──────────┬───────┬────┐
│   │ url                             │ path        │ title    │ pages │ ms │
├───┼─────────────────────────────────┼─────────────┼──────────┼───────┼────┤
│ 0 │ http://127.0.0.1:4603/reports/1 │ out/001.pdf │ Report 1 │ 2     │ 79 │
│ 1 │ http://127.0.0.1:4603/reports/2 │ out/002.pdf │ Report 2 │ 2     │ 82 │
│ 2 │ http://127.0.0.1:4603/reports/3 │ out/003.pdf │ Report 3 │ 2     │ 79 │
│ 3 │ http://127.0.0.1:4603/reports/4 │ out/004.pdf │ Report 4 │ 2     │ 81 │
│ 4 │ http://127.0.0.1:4603/reports/5 │ out/005.pdf │ Report 5 │ 2     │ 31 │
│ 5 │ http://127.0.0.1:4603/reports/6 │ out/006.pdf │ Report 6 │ 2     │ 34 │
│ 6 │ http://127.0.0.1:4603/reports/7 │ out/007.pdf │ Report 7 │ 2     │ 34 │
│ 7 │ http://127.0.0.1:4603/reports/8 │ out/008.pdf │ Report 8 │ 2     │ 31 │
└───┴─────────────────────────────────┴─────────────┴──────────┴───────┴────┘
failed http://127.0.0.1:4603/reports/missing HTTP 404
8 PDFs, 1 failed, 117 ms
```

The exit code is `1` because one URL failed. Check a file:

```
$ pdfinfo out/001.pdf | grep -E 'Tagged|Pages|Page size'; pdffonts out/001.pdf; pdftotext -layout out/001.pdf - | head -7
Tagged:          yes
Pages:           2
Page size:       595.92 x 841.92 pts (A4)

name                                 type              encoding         emb sub uni object ID
------------------------------------ ----------------- ---------------- --- --- --- ---------
LGLWLC+BigCaslon-Medium              CID TrueType      Identity-H       yes yes yes     11  0

Report 1
Reference REG-1-be08b23e-89be-48b8

                     Item                     Amount
 Line 1                              100.00
 Line 2                              101.00
 Line 3                              102.00
```

The page is A4 at Chrome's size (595.92 x 841.92 pt), the web font is embedded as a subset, and the text comes back out, including the long reference number.

## How the parts work

**Paper size.** `Page.printToPDF` takes inches: `paperWidth: 8.27, paperHeight: 11.69` is A4. In the Playwright library, `page.pdf()` passes its options to the same command, so use these names there too; Playwright's `format: 'A4'` and `margin` are ignored and give US Letter. See [the parameter table](/scraping/pdf#parameters).

**Print media.** Pages are printed with `screen` media unless you say otherwise, so `@media print` rules do not apply. `Emulation.setEmulatedMedia { media: 'print' }` turns them on, as Chrome does when it prints. A control run without that line failed every file with `screen-only content was printed`.

**Fonts.** Before writing the PDF, the browser waits up to 10 seconds for the web fonts and images the layout needs. Fonts the page loads with `@font-face` are embedded. Fonts it only names come from the machine's system fonts, so a PDF can look different on a server; on Linux install some (`fonts-dejavu-core`, `fonts-liberation`) or, better, load your fonts with `@font-face`. `pdffonts` lists what a file embeds.

**Waiting for the content.** `goto` returns at `load`. For pages that render their content after that, read an element that only exists once the content is there (here the `h1`) before printing. The read waits for it. For pages that load data with `fetch`, see [Data from single-page apps](/recipes/scraping-spa-data).

**Error pages.** The run's `requests` list has the status of the document request. The batch refuses anything but `200`, so a 404 page does not end up as a PDF that looks fine.

**Checking the output.** `pdftotext -layout` gives the text a reader or a search index gets. The batch checks that the title is there and that screen-only text is not. Add checks that fit your pages: a total, a row count, the last line. Use `-layout`: without it, `pdftotext` joins words broken at a hyphen across lines.

**Concurrency.** `Pyro.launch({ connections: N })` opens N browser threads, and `pyro.page()` takes the least busy one, so N pages lay out and print in parallel. With 40 URLs of the test site on an M1 Max (10 cores):

| `CONCURRENCY` | Time for 40 PDFs |
| --- | --- |
| 1 | 1213 ms |
| 4 | 400 ms |
| 8 | 288 ms |

Do not block the event loop in a job: an earlier version that ran `pdftotext` synchronously took 937 ms at 4 connections. Above the number of cores, more connections do not help. For sites you do not own, the site's limits matter more than yours.

## Your own HTML instead of URLs

For invoices or reports you generate, skip the web server: set the HTML on a page and print it. In the Playwright library, `await page.setContent(html)` then `page.pdf({ paperWidth: 8.27, paperHeight: 11.69, printBackground: true })`. Relative URLs have no base there; use absolute or `data:` URLs. See [PDFs from your own HTML](/scraping/pdf#pdfs-from-your-own-html).

## Next

- Every PDF option, page breaks and page boxes: [Screenshots and PDFs](/scraping/pdf).
- Running the browser on a server with fonts installed: [Running as a service](/scraping/service).
