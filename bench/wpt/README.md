# WPT coverage gate

`.github/workflows/wpt.yml` runs our WPT subset (the directories in `lib.mjs` `DEFAULT_DIRS`, ~2,800 pages) on every
pull request and fails when coverage goes down. `baseline.json` records, per page, which subtests pass, whether the
page completed, and how long it took.

## How a run works

1. `jsc`: the patched JavaScriptCore from the Actions cache (`jsc.yml`). The gate tests the fork build because the
   system framework lacks window named properties and import maps: ~3,200 more subtests fail with it, for that reason
   alone.
2. `build`: `cargo build --profile release-fast` (dependencies cached by `Swatinem/rust-cache`, saved from `main`
   only). The browser and its JavaScriptCore are bundled (`scripts/release/bundle-jsc.sh`) and passed to the shards.
3. `wpt`: four macOS shards. Each one restores the pinned WPT checkout (`fetch-wpt.sh`: revision `wpt-revision.txt`,
   only the paths in `wpt-paths.txt`, ~85 MB, cached by both files), takes its share of the pages (longest first,
   balanced by the baseline's durations), runs it at `--jobs 4` (3 CPUs), and reruns once every page whose result
   differs from the baseline.
4. `gate` (Linux): merges the shards, checks them, writes the summary, and uploads a refreshed `baseline.json` as the
   `wpt-baseline` artifact when it changed.

A pull request **fails** when, after the rerun:

- a subtest that passes in the baseline does not pass (in both runs), or
- a page that completed in the baseline does not complete (timeout, hang, crash) in both runs, or
- a shard is missing, a planned page has no result, or a baseline page was not discovered (a file missing from the
  WPT checkout; `update --prune` drops pages on purpose).

**Improvements** (subtests that pass in both runs and not in the baseline) are reported, with a "baseline is stale"
warning; commit the refreshed baseline in the same pull request. Pushes to `main` and manual runs run every page;
pull requests skip the pages with nothing to protect (no passing subtest and no completion, or a completion only by
testharness's own timeout after 5 s or more) and give a page that completed at most 10x its baseline duration
(10 s minimum, the normal 15 s / 45 s timeout maximum) before the rerun, which uses the normal timeouts.

`flaky.json` lists pages, or single subtests, the gate does not check: `{"tests": {"<url>": {"reason": "...",
"subtests": ["<name>"]}}}`. Every entry needs a reason; keep it empty if possible.

## Baseline

The baseline is made in CI, never locally: download a run's `wpt-baseline` artifact and commit it.

```sh
gh run download <run-id> -n wpt-baseline -D bench/wpt     # writes bench/wpt/baseline.json
```

- **No baseline yet, or a new WPT revision** in `wpt-revision.txt`: the run bootstraps. It runs every page, reports
  without failing, and its artifact is the new baseline.
- **Improvements in a pull request**: the gate's artifact adds the confirmed new passes; commit it in that PR.
- **Accepting a loss** (a test that is wrong, a feature removed on purpose): `bun bench/wpt/gate.mjs update <shard
  results...> --replace` takes the results as they are; or add the page to `flaky.json` with the reason.

`gate.mjs update` only adds by default: a subtest that passed in the baseline still does. A page's duration changes
only in full runs and only if it moved by 2x and 1 s (`--durations` refreshes all), so a refresh touches few
lines. One page per line:

```
"dom/nodes/Node-appendChild.html": {"status":"complete","ms":570,"n":11,"fail":[]}
"html/dom/documents/dom-tree-accessors/Document.body.html": {"status":"timeout","ms":15000,"n":26,"pass":["..."]}
```

`fail` names the failing subtests of a completed page (all others passed); `pass` names the passing ones when that is
shorter or the page did not complete. Naming every pass would make the file ~38 MB instead of ~0.5 MB.

## Run it locally

```sh
(cd bench && bun install)
bench/wpt/fetch-wpt.sh ~/wpt                       # the pinned revision, ~15 s
cargo build --profile release-fast                 # with the patched JavaScriptCore (scripts/jsc/build.sh)
bun bench/wpt/gate.mjs run --wpt ~/wpt --bin target/release-fast/lightningpanda --out /tmp/wpt.json
```

That runs the whole gate plan (minutes) and reruns what differs. Faster loops:

- `--dirs dom,css/selectors` or `--match 'dom/nodes/Node-'`: only those pages (no missing-page check).
- `--retest /tmp/wpt.json`: only the pages whose result in an earlier run differs from the baseline.
- `--full`: every page, as `main` runs it.

`bun bench/wpt/gate.mjs diff A.json B.json` lists the pages whose status or passing subtests differ between two runs
(comma-separate shard files: `a0.json,a1.json`). To compare in CI, start a manual run with `runner-args` (for example
`--reuse-context`), download both runs' `wpt-shard-*` artifacts, and diff them.

`run.mjs` is the runner underneath (`--list` prints what would run; see its header). Every page gets a new browser
context, so a page's result does not depend on the page that ran before it in the same worker, which changes with
the shard split. Results do not depend on the WPT checkout's own `testharnessreport.js`: `serve.mjs` serves
`testharnessreport.js` from this directory, which exposes `window.report`.

## Timings

The slowest pages are slow by design or by a known engine gap, not by the runner:

- `dom/nodes/NodeList-static-length-getter-tampered-*.html` (6 long pages, 22-29 s each on a loaded machine): 50 x
  50,000 loops over a 100-node `NodeList`, ~1.3e8 indexed reads and `length` gets, each a call into our bindings. It
  is a JIT test; making it fast means indexed access the JIT can inline, not a runner change.
- `xhr/xmlhttprequest-timeout*.html` (37 variants, 5-20 s each, 266 s in all): the tests wait on their own 5 s timers
  and use CPU only briefly, which is why each shard runs one page more than it has CPUs.
- Pages ending at 10 s complete by testharness's own timeout; those with nothing passing are skipped by the gate.
- `encoding/legacy-mb-*/*-encode-form-*.html?...` occasionally never report on a loaded machine (45 s timeouts in
  2-5 of 584 pages per full local run): the gate's 10 s bound and the rerun absorb them.
