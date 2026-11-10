# Benchmarks and CDP recording

Reproducible harness for measuring any CDP browser binary (upstream Lightpanda or `lightningpanda`) with the
same Playwright version TOS pins (`playwright-core@1.59.1`). Three result classes are kept strictly apart:

1. **Upstream micro-smoke** (`results/upstream-2ad38773f/`): original Lightpanda on a local static site. It
   characterizes the reference binary only. It is **not** a TOS E2E baseline, and it is **not** a TOS parity result.
2. **TOS E2E parity/baseline**: real `tos-monorepo` FE E2E lanes. Pending; they run on a host with native
   PostgreSQL (see `docs/automation/TOS_BASELINE.md` once recorded).
3. **Rewrite performance**: matched, explicitly scoped workloads against release binaries. The HTTP CPU/memory
   runner below covers the integrated navigation path; full TOS comparisons remain dependent on compatibility.

## Setup

```sh
cd bench && bun install                                           # playwright-core 1.59.1, ws 8.21.0
(cd rss-sampler && cargo build --release)                         # macOS proc_pid_rusage sampler
```

The upstream binary is built from source; its provenance and exact commands are in
`results/upstream-2ad38773f/PROVENANCE.md`. Binaries are not committed.

## Run

```sh
node scripts/run-baseline.mjs --bin /abs/lightpanda --results /abs/out [--startup-runs 10] [--smoke-runs 5] [--warm-cpu] [--no-proxy]
node scripts/cdp-record-proxy.mjs --upstream ws://127.0.0.1:<port> --out /abs/cdp.jsonl [--listen <port>]
node scripts/cdp-summarize.mjs /abs/cdp.jsonl /abs/inventory.json [/abs/inventory.md]
```

The browser is started with the exact flags and environment TOS uses (`apps/fe/e2e/fixtures/lightpanda.ts`).
`serve --host 127.0.0.1 --port N --log-level error --experimental-features cors --load-resources stylesheet
--load-resources iframe`, with `LIGHTPANDA_DISABLE_TELEMETRY=true`.

## Exit status

`smoke.mjs` exits 1 unless every step passed (steps blocked by an earlier failure are reported as skipped);
`run-baseline.mjs` exits 1 if any smoke run or startup run failed. Results are written either way.

## Methodology and limitations

- Startup = spawn to the first `200` from `GET /json/version`, polled every ~2 ms with fresh connections
  (the TOS fixture polls every 10 ms, so its observed readiness can be up to ~10 ms later).
- RSS is sampled at 20 ms by `rss-sampler` (kqueue `NOTE_CRITICAL` timer; on the reference host plain sleeps woke
  after 40–170 ms), cross-checked against `ps` sampling and `/usr/bin/time -l` max RSS.
- Small samples (n=10 startup, n=5 smoke): p95 equals the max. Single host, single build.
- **CPU frequency scaling is not controlled.** On the reference M1, startup median was 108 ms with an idle machine
  and 60 ms with one core kept busy (`--warm-cpu`). Treat this as a limitation of the measurement. Report both
  series and compare engines only under the same mode.

## CDP performance suite (regressions and optimization baseline)

`cdp-perf.mjs` measures latency and throughput of realistic CDP workloads against one or more `serve`
binaries. Run it with `bun` (1.4+): the high-level client scenarios use `Bun.WebView` with its Chrome backend
attached to the endpoint. Under `node` only the raw CDP scenarios run; `cdp-perf-compare.mjs` diffs two results and flags regressions. Use it to check that a change does
not slow CDP down, and to see where time goes before optimizing.

```sh
cargo build --release                                        # from the repository root (JIT-entitled linker)
(cd bench && bun install && (cd rss-sampler && cargo build --release))
bun bench/scripts/cdp-perf.mjs --list                        # scenarios
bun bench/scripts/cdp-perf.mjs --bin $PWD/target/release/lightningpanda --out /tmp/cdp-perf/now.json
bench/scripts/cdp-perf-ab.sh origin/main                     # main vs. the working tree, interleaved (see below)
node bench/scripts/cdp-perf-compare.mjs run.json --base base --head head
node bench/scripts/cdp-perf-compare.mjs old.json new.json    # two separate runs (less reliable)
```

### A/B comparison (`cdp-perf-ab.sh`)

`bench/scripts/cdp-perf-ab.sh BASE [HEAD] [-- cdp-perf options]` builds both sides and runs them interleaved.

- **Profile.** By default it builds with `perf` (`bench/scripts/perf-profile.toml`: release without LTO, 16 codegen
  units, incremental, no debug info), passed with `cargo --config` so that any commit can be built with the same
  settings. It is not the shipped code, so compare its base with its head and never with release numbers.
  `--shipping-profile` builds the real release profile, in a target directory per SHA; use it for final sign-off.
- **Reuse.** The `perf` profile has one reused checkout and target directory per side
  (`$LP_PERF_WORK/slot-a`, `slot-b`). Finished binaries are kept in `$LP_PERF_WORK/bin-<profile>-<sha>/`, and a
  commit with a matching binary is not rebuilt.
- **Provenance.** A shared target directory has silently produced identical binaries before
  (`results/build-provenance-audit`), so the checkout must be clean and at the SHA right before and after cargo, a
  crate whose sources changed since the slot's last build must not be reported fresh, the binary must carry
  `allow-jit`, and the run is refused when base and head are identical (SHA-256, or `__text` and both `__const`
  sections on macOS) although files the build reads differ. Integration tests and Markdown do not count as such files.
- **`--auto-filter`** picks the scenarios from the changed paths, taking the union: `lp-render` and `screenshot.rs`
  select `page`, `lp` and `pdf` (and remind you to run `bench/pdf/run.mjs`, since cdp-perf measures only paper-cut
  printing);
  `lp-webapi`, `lp-js` and `lp-jsc-sys` select `runtime`, `dom` and `lp`; `lp-net` selects `network` and
  `throughput`; anything else, including the `lp-cdp` engine and `vendor/`, selects
  `^(rtt|throughput|runtime|dom|page|target|network|lp|tos|pdf)\.`. Documentation and integration tests alone skip the run.
- **Shared machine.** It refuses to start while any cargo build is running and lists each with its owner (this
  repository or another project) and directory; `--force` overrides. It then waits for `/tmp/lp-bench.lock`, held
  until it exits. If a run was killed outright, `rmdir` the lock by hand.
- **Output.** `stages.txt` in the results directory records the time and load average of each stage. A load average
  above the CPU count prints a warning: do not trust wall-clock verdicts from such a run.
- **Instructions retired.** The `instructions/op` column (macOS; the script builds `rss-sampler`) counts what the
  browser process executed, so load barely moves it. It is informational and does not set the verdict. It misses
  whatever changes speed without changing the instruction count, such as code layout and cache behaviour.

Measured on d667459d vs 430345ba on an M1 Max with other builds running. Each build time is one sample, and the
load average differs between them, so read these as orders of magnitude:

| stage | old flow (release, target per SHA) | `perf` profile | load average |
|---|---|---:|---:|
| first build, empty cache | 3:25-5:23 per side | not faster: 10:07, 8:28 (before `vendor/` became cacheable) | 270-160 |
| cold build, warm sccache | | 4:24, 4:55 | 55-157 |
| rebuild, 430345ba and d667459d swapped between slots | 2:59, 2:47 (`--shipping-profile`, release rebuild of the workspace crates) | 0:47, 0:49 | 105-160 |
| rebuild, unused function added to `lp-cdp` | | 0:03 | 118 |
| binary already built | | 0:00 | 108 |
| dirty working tree, first build then next edit | | 9:40, 0:22 | 158-90 |

In one interleaved 9-round run of `^(page\.|lp\.)` with all four binaries (load average 148 falling to 97):

- **Absolute numbers:** `perf` against `release` was 0.91-1.34x in wall clock (mostly noise), and within 0.98-1.10x in
  instructions. The largest gap is `page.navigate.about:blank` (+7-10%); the `lp.*` flows are within 1%.
- **Instructions, head against base:** the two profiles agree that nothing changed: at most 2.1% on any scenario,
  and the spread between rounds was 1-2% on most `lp.*` rows, up to 17% on `page.navigate.*`.
- **Wall-clock verdicts:** no regression on either profile, but they agreed on only 4 of 8 scenarios. Every
  disagreement is an `unclear` with p > 0.38, on different scenarios per profile. That is load, not the profile.

Under this load use instructions to look for a change, and confirm a wall-clock claim with `--shipping-profile` on a
quiet machine.

Scenarios (`--filter REGEX` matches names or groups; `--quick` = 1 round at 20% iterations, for checking the
harness only):

- **connection**: WebSocket connect + `Browser.getVersion` + close. Each connection creates a browser thread and
  a JavaScriptCore engine, so this is the per-client setup cost.
- **protocol**: `Browser.getVersion` round trip (the WebSocket + dispatch floor), `Runtime.evaluate "1+1"`
  round trip, and 256 pipelined evaluates (throughput).
- **runtime**: large by-value results, `callFunctionOn` with a tiny and a ~38 KB function (Playwright re-sends
  its utilities), handle creation + `releaseObject`.
- **dom**: `querySelectorAll` and `textContent` over a 2000-row document, 1000-element create/append/remove,
  and Playwright's click preparation (`describeNode` + `getContentQuads`).
- **navigation**: `about:blank`, a small page, 10 scripts + 5 stylesheets, and a 2000-row document, each
  timed to `Page.loadEventFired`, with milestones (navigate reply, `frameNavigated`, DOMContentLoaded, load).
- **targets**: context + target create, attach, `Page`/`Runtime.enable`, close, dispose, with per-step laps.
- **network**: 20 parallel `fetch()` with `Network.enable`; one `Fetch.requestPaused` → `fulfillRequest`.
- **input**: `Input.dispatchMouseEvent` clicks and `Input.dispatchKeyEvent` typing, verified in the page.
- **pdf**: `Page.printToPDF` of a document without `.pz-page` elements, which is cut into pages. `pdf.paginate.text`
  is about 20 Letter pages of plain lines, `pdf.paginate.avoid` about 20 pages of `break-inside: avoid` boxes, and
  `pdf.paginate.shrink` a 1200 px wide document that takes a second layout to shrink to fit. The content is set once;
  a sample is one print, checked by its page count (within 2, so versions with other pagination rules compare). `bench/pdf/run.mjs` measures the other path: its ToS documents
  have `.pz-page`, which skips pagination altogether.
- **webview** (`Bun.WebView`, needs `bun`): open + navigate + close, `navigate` of each fixture page,
  `evaluate`, raw `view.cdp()`, selector `click` (with its actionability checks), `type`, and a whole form
  test flow with laps.
- **playwright** (opt-in with `--playwright`; `playwright-core` 1.59.1, the version TOS pins): `connectOverCDP`,
  new context/page, `goto`, `evaluate`, locator `textContent`, fill + click + response, `route.fulfill`, and a
  whole test flow with laps.

Every scenario checks its result (counts, values, events), so a broken feature fails instead of looking fast.

Method:

- Every scenario runs in a **fresh browser process**, once per round and binary, after a short warmup
  (10% of its iterations). One sample is one operation, timed in Node, so it includes the WebSocket hop.
- With several `--bin label=path` binaries the order **alternates every round** (AB, BA, ...), so host drift
  (thermal state, other load, DVFS) hits both equally. Compare binaries within one run whenever possible.
- `cpu/op` is browser CPU per operation (all threads), from `rss-sampler` on macOS (auto-detected when built)
  or `/proc` on Linux. On a loaded host it is steadier than wall time, and it shows where the browser itself
  spends time rather than the client or the socket.
- A round is the unit of evidence: samples within one process are correlated. `cdp-perf-compare.mjs` calls a
  scenario a **REGRESSION** only when the median of round medians is slower by at least `--threshold`
  (default 10%) *and* an exact Mann-Whitney U test on the round medians gives p < `--alpha` (default 0.05).
  With 5 rounds per side that needs near-complete separation; with fewer than 4 rounds nothing can reach
  it, so large changes are reported as `unclear`. Exit status is 1 on any regression or failed scenario.
- The run records the load average at start, per round and at end, and warns when the 1-minute load is above
  half the CPU count. The output JSON keeps every sample, so results can be re-analysed later.

Inner CDP stages: build with `cargo build --release --features perf-trace` (use its own `CARGO_TARGET_DIR`) and
add `--perf-trace`. The browser then times each stage of every command (`io.ws_read`, `io.ws_decode`, `cdp.parse`, `cdp.dispatch <Method>`, `cdp.params`, `js.eval` / `js.call_function`, `remote.describe`,
`json.serialize`, `outbox.push`, `host.flush`, `io.outbound_queue` (time a frame waits for the socket) and `io.ws_write`), and the report
lists every stage costing at least 10 ns per operation, with the timer floor measured on that host. Stages
are inclusive and timing adds a few tens of ns per stage, so compare traced builds only with traced builds.
Normal builds compile the timers out (`crates/lp-cdp-protocol/src/perf.rs`).

`--fork` starts `serve --fork` and reads CPU from every descendant of `serve`, since connections run in forked
children. Without it, setting `LIGHTNINGPANDA_FORK=1` still runs fork mode, but CPU reads as about zero.

Reading it for optimization: compare `rtt.Browser.getVersion` (protocol floor) with the other `rtt`/`runtime`
rows to see the cost of the browser-thread hop and JS work; the navigation milestones show whether time goes
to fetching/committing (navigate reply), parsing (DOMContentLoaded) or subresources (load); `target.lifecycle`
laps show which step of page creation dominates. Running an upstream Lightpanda binary as a second `--bin`
shows where the rewrite is behind. For in-process costs without sockets, see
`cargo run --release -p lp-cdp --example cdp_microbench`.

## Matched CPU and retained-memory workload (macOS)

`compare-resources.mjs` alternates upstream/rewrite release binaries on one host. Each fresh process warms
one HTTP page, idles for 10 seconds, navigates 50 times, creates/navigates/disposes 20 contexts, then waits
2 seconds after disposal. Every navigation asserts status, URL, exact response bytes and an inline-script
checksum. This deliberately limited workload excludes DOM interaction, subresources, TLS and TOS flows.
It supports measuring the integrated HTTP path; it cannot establish whole-browser performance parity.

```sh
node scripts/compare-resources.mjs \
  --upstream /abs/upstream-release --upstream-sha FULL_40_CHAR_SHA \
  --rewrite /abs/rewrite-release --rewrite-sha FULL_40_CHAR_SHA \
  --sampler /abs/rss-sampler --out /abs/new-results-directory \
  --samples 3 --navigations 50 --contexts 20 --idle-ms 10000 --retained-ms 2000
```

Use source SHAs corresponding to the actual builds and retain their build recipes. Build every compared SHA from a
clean checkout into its **own fresh, empty `CARGO_TARGET_DIR`**. A shared target directory can silently reuse
stale artifacts when new worktrees have older file times: one comparison produced two identical binaries that
way (`results/build-provenance-audit/`). Before timing, check that the binaries' code differs as expected; to
verify an existing binary against its source, compare `scripts/macho-section-hash.py` output with a fresh build
(whole-file hashes are not reproducible across target directories). Results include executable
SHA-256, source SHA declarations, sampler/harness/fixture hashes, host, flags, version response, sample order,
raw phase snapshots, native traces, errors and direct exit/signal status. Output directories must be new.
Any failed behavioral assertion makes the run fail and keeps its evidence; do not aggregate failures as successes.

- CPU is measured for the browser PID, including all its threads, excluding harness and helper processes.
  Node CPU (Playwright plus the local HTTP fixture) is reported separately for each phase; sampler cumulative
  CPU is also retained. There is no TOS harness in this workload.
- `proc_pid_rusage` supplies Mach-time CPU counters. The sampler converts with `mach_timebase_info`, using
  wide intermediate arithmetic. Apple's [task accounting](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/task.c)
  and [rusage export](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/bsd_kern.c) define this path.
  Do not assume a 1:1 nanosecond timebase on Apple Silicon. A local cross-check against `getrusage(RUSAGE_SELF)`
  during a 250 ms CPU workload is part of sampler validation.
- `browserOneCorePercent` = (user + system CPU time) / actual sampled wall time × 100, without dividing by
  host core count. Phase CPU windows bracket snapshot helper launches; `workWallMs` separately measures the
  actual workload. Snapshot helper overhead is outside reported Node CPU, and may slightly lengthen browser
  observation windows.
- Native memory is sampled every 20 ms; reported peaks can miss shorter spikes and are not lifetime maxima.
  Retained resident/physical-footprint values are measured after context disposal and a fixed quiet interval,
  without forced GC. They are not live-heap measurements or proof of a leak.
- Each sample is a fresh browser process but OS/file caches are uncontrolled and likely warm. One page warms
  the engine before phases. No busy-spin CPU warmer is used. Host contention and DVFS remain uncontrolled;
  alternate binary order, retain individual samples and report sample count. Never compare debug to release.

The sampler's six columns are epoch milliseconds, resident KiB, physical footprint KiB, cumulative user
nanoseconds, cumulative system nanoseconds and monotonic nanoseconds. Existing memory readers use the first
three columns. `rss-sampler PID - --once` emits one snapshot and fails explicitly if the PID cannot be sampled.

## Web Platform Tests

`bench/wpt/` runs a WPT subset over CDP with Playwright and reports passing subtests per directory. The same
runner works for any binary with a `serve --host --port` command line (lightningpanda, upstream Lightpanda, or a
shim around Chrome). Pull requests run it as a coverage gate against a committed baseline; see
[`wpt/README.md`](wpt/README.md).

```sh
(cd bench && bun install)
bench/wpt/fetch-wpt.sh /abs/wpt   # a new directory: the pinned upstream revision, only the paths the runner needs
bun bench/wpt/run.mjs --wpt /abs/wpt --bin lp=$PWD/target/release/lightningpanda --sample-encoding 10 --out /tmp/lp.json
bun bench/wpt/aggregate.mjs /tmp/lp.json [/tmp/other.json ...]                 # markdown table
```

`run.mjs` starts `serve.mjs` on a free port unless `--base` names a running one, and `serve.mjs` serves
`bench/wpt/testharnessreport.js` (it exposes `window.report`) in place of the checkout's, so any WPT checkout works.
`--list` prints what would run. The default directories are dom, html/dom, encoding, url, fetch/api/basic, xhr,
css/selectors, uievents, shadow-dom and webstorage. encoding/ has 1,258 pages, most of them long:
`--sample-encoding 10` keeps every 10th, so scale its subtests by about 10.

`serve.mjs` is a static server, not wptserve: it generates `.any.html`/`.window.html` wrappers and applies `.headers`
files. Tests that need Python handlers, testdriver, `.sub`/`.https`/`.h2` or a non-window global are excluded for every
binary (616 files at the pinned revision), and cross-origin hosts do not resolve. Compare binaries within one setup,
not against published WPT numbers. The denominator per page is the largest subtest count any binary reported, so a
crashed page counts its subtests as failures.

## Playwright feature suite

`bench/playwright-suite/` has 38 tests, one per Playwright feature a typical app E2E suite uses, against local
fixture pages. It connects with `connectOverCDP`, so any binary with a `serve --host --port` command line works.

```bash
cd bench/playwright-suite && bun install
TARGET=lp BIN=$PWD/../../target/release/lightningpanda bunx playwright test
LIGHTNINGPANDA_FORK=1 TARGET=lp BIN=... bunx playwright test    # fork template mode
TARGET=chrome bunx playwright test                              # Playwright's headless shell over CDP (macOS arm64 path, or set CHROME_BIN)
```

Results go to `results/$TARGET.json` (or `$OUT/$TARGET.json`). `baseline/visual.png` is the `toHaveScreenshot`
reference, taken with `TARGET=chrome-launch bunx playwright test --update-snapshots` on macOS.

## Page-break fixtures against Chrome (`pdf/fixtures/`)

Reproducible sets for how paper-cut `Page.printToPDF` documents break, with Chrome 156's results. `generators/` holds
one module per set, each exporting `cases` (name, HTML, print options); `chrome-156/` the measured summaries (per page:
word count, a hash of its sorted words, first and last word, distinct y), no PDFs. Names are `group|...`.

```sh
cd bench/pdf/fixtures                                  # needs pdftotext and Chrome at its default path
bun probe.mjs generators/fx-fr.mjs chrome out.json    # or a binary instead of chrome
python3 compare.py chrome-156/fr.json out.json [-v]    # per group: words on the same pages, page counts, positions
bun print-72.mjs /abs/lightningpanda                  # the 72: pages, lines and link rects, Chrome measured live
./run.sh /abs/lightningpanda /tmp/out [fr sub ...]     # every set (or those named), against chrome-156/
bun geometry.mjs /abs/lightningpanda out.json         # the 152 geometry matrix
python3 compare.py --geometry chrome-156/geometry.json out.json
```

| Sets | Generators | Cases | What varies |
|---|---|---|---|
| fractional spacers | `fx-fr` | 342 | a position's fraction before 20 px lines, 9 paper setups |
| sub-pixel groups | `fx-sub` | 1656 | line heights, accumulated fractions, nested margins, later pages, under 0.5 px overflow, forced breaks, floats |
| whole-px analogues | `fx-sub2`, `fx-sub3` | 86 | the same shapes in whole px, also parallel columns and tables |
| vertical writing | `fx-vert` | 24 | vertical blocks and documents |
| tall monolithic boxes | `fx-mt` | 512 | margin, border and padding above images, size containment, vertical boxes |
| page geometry | `fx-72` | 120 | paper, margin and scale on links, text, breaks, shrink to fit |
| orphans, widows | `fx-wo`, `fx-wo2`, `fx-wo3` | 125 | `orphans` and `widows` around cuts |
| avoid, shrink, sizes | `fx-av`, `fx-sh`, `fx-sr`, `fx-tw`, `fx-engine` | 381 | `break-inside: avoid`, overflowing documents, page sizes |
| gaps closed by #251 | `fx-b`, `fx-b2`, `fx-b3`, `fx-f`, `fx-h`, `fx-h1`, `fx-k`, `fx-m`, `fx-s`, `fx-v` | 1706 | monolithic boxes, `break-*: avoid`, forced breaks, line and box positions at a cut |
| `<br>` in flex and grid | `fx-br` | 631 | column and row flex, grid, a plain block: consecutive `<br>`s, text and spans around them, CSS on the `<br>`, preserved white space, line heights and fonts |

Page placement and text x of whole PDFs against Chrome:

```sh
bun page-placement.mjs <work dir> chrome-156/page-placement.json [/abs/lightningpanda]   # documents x papers x options, and the 300 dpi page size
bun pdf-deltas.mjs <chrome pdf dir> <lp pdf dir> out.json        # per PDF of a run.mjs run: x and y delta of 30 pt tiles at 600 dpi, pixel diff at 150 dpi
python3 page-stream-diff.py; python3 page-shift-check.py         # two lp-fixed runs (r-base, r-head): how each stream changed; pixels after undoing a page shift
bun text-x.mjs <work dir> chrome; bun text-x.mjs <work dir> /abs/lightningpanda   # fractional text x: paper-cut and `.pz-page`, with screenshots
python3 table-check.py chrome-156/tcell-x.json <probe of fx-tcell>   # auto tables with percentage, fixed, auto and empty cells: marker x and y
python3 width-check.py chrome-156/width-x.json <probe of fx-width>   # what paper-cut print lays out in: marker x and link rects
```

`pdf-resample.mjs` compares a page's error against Chrome at 150 dpi as poppler rasterizes it and box-averaged from a higher resolution, to tell a real move from a sampling effect.

`bun table-collapse.mjs chrome|/abs/lightningpanda out.json` measures cell boxes of collapsed-border tables (`chrome-156/table-collapse.json`).

`chrome-156/tos-deltas.json` holds the 44 ToS cases before and after (median, p95, max of per-tile deltas in pt; the search stops at 2.4 pt, and
tiles match by ink profiles, so regular text can alias: read medians and p95 first). `text-x-before.json` and `text-x-after.json` hold the text-x set, `text-x-width.json` the same on the paper-cut layout width.

Compare only runs from one Chrome version: the summaries say what 156 did on the machine that measured them (macOS,
its Arial). Word assignment is what the sets pin; a page's y differs by a constant between engines, because pdftotext
reads a different ascent from each PDF's font.
