# WPT coverage gate

`.github/workflows/wpt.yml` runs our WPT subset (the directories in `lib.mjs` `DEFAULT_DIRS`, ~2,800 pages) on every
pull request and fails when coverage goes down. `baseline.json` records, per page, which subtests pass, whether the
page completed, and how long it took.

## How a run works

1. `prepare` (Linux, seconds): cache keys. The browser is keyed by every input of its build (the git object ids of
   `crates/`, `vendor/`, `Cargo.*`, `.cargo/`, `scripts/macos/`, `bundle-jsc.sh` and `wpt.yml`, the stable toolchain
   and the patched JavaScriptCore), so a pull request that changes none of them skips the next two jobs.
2. `jsc`: the patched JavaScriptCore (`jsc.yml`), only when neither it nor the browser is cached. The gate tests the
   fork build because the system framework lacks window named properties and import maps: ~3,200 more subtests fail
   with it, for that reason alone.
3. `build`: `cargo build --profile release-fast` (dependencies cached by `Swatinem/rust-cache`, saved from `main`
   only), bundled with its JavaScriptCore (`scripts/release/bundle-jsc.sh`) and saved to the cache.
4. `wpt`: four macOS shards. Each restores the pinned WPT checkout (`fetch-wpt.sh`: revision `wpt-revision.txt`,
   only the paths in `wpt-paths.txt`, ~85 MB, cached), takes its share of the pages (longest first, balanced by the
   baseline's durations) and runs it at `--jobs 4` (3 CPUs). Every page whose result differs from the baseline then
   runs at least twice more at the same `--jobs`, padded with copies when few pages differ, so a timing-sensitive
   page reruns under about the load it first ran under.
5. `gate` (Linux): merges the shards, checks them, writes the summary, and uploads a refreshed `baseline.json` as the
   `wpt-baseline` artifact when it changed and the gate passed or failed only for a stale or missing baseline.

Every run of a page counts: a subtest is **lost** if it passed in the baseline and in no run, and a page **broke** if
it completed in the baseline and in no run. A gain needs every run: a subtest passing in all of them, or a page
completing in all of them. So a flake that passes once neither fails the gate nor enters the baseline.

The gate **fails** when:

- a subtest is lost or a page broke (a regression);
- a completed page reports a different set of subtest names than the baseline recorded and no run reports the
  recorded set: which of its passes are gone is then unknown (see "Baseline");
- there are improvements the baseline does not record ("stale": commit the artifact, below);
- there is no baseline for this WPT revision while the base branch has one (a revision change, or a removed
  baseline): the run did every page twice; commit its artifact;
- the pull request's `baseline.json` or `flaky.json` drops coverage the base branch's protect (passes removed, a
  page no longer recorded as completing, a page removed, a `flaky.json` entry added or widened), unless the pull
  request has the **`wpt-accept-loss`** label. The label is read when the `gate` job runs: add it and re-run that job;
- a shard is missing, a planned page has no result, a baseline page was not run (a file missing from the WPT
  checkout; `update --prune` drops pages on purpose), or a shard skipped a page the skip rule does not skip.

Pushes to `main` and manual runs run every page. Pull requests skip the pages with nothing to protect (no passing
subtest and no completion, or a completion only by testharness's own timeout after 5 s or more) and give a page that
completed at most 10x its baseline duration (10 s minimum, the normal 15 s / 45 s timeout maximum) in its first run;
reruns use the normal timeouts. The summary lists the pages whose runs disagreed with each other: `flaky.json`
candidates.

With no baseline anywhere (the first runs), a run only reports; its artifact is the first baseline.

## flaky.json

What the gate does not check, each with a reason. Keys are page URLs, or patterns where `*` matches anything:

```js
{"tests": {
  "<url>": {"reason": "..."},                                  // the whole page
  "<url>": {"reason": "...", "subtests": ["<name>", "..."]},  // these subtests (the page must still report them)
  "encoding/*-encode-form-*": {"reason": "...", "allowIncomplete": true}
}}
```

`allowIncomplete` ignores a run of the page that did not complete; a completed run is checked as usual, and more
than 3 such pages without any completed run fail the gate (that is not a flake). Unknown fields are an error, so a
typo cannot silently mask a whole page. Flaky pages keep their baseline entries: `update` adds nothing for them.

## Baseline

The baseline is made in CI, never by hand: download a run's `wpt-baseline` artifact and commit it.

```sh
gh run download <run-id> -n wpt-baseline -D bench/wpt     # writes bench/wpt/baseline.json
```

- **Improvements, or a new WPT revision**: the gate fails until the artifact is committed in the pull request.
- **Accepting a loss** (a test that is wrong, a feature removed on purpose, a page whose subtest names changed):
  download the run's shard results and replace those pages, then label the pull request `wpt-accept-loss`:

  ```sh
  gh run download <run-id> -p 'wpt-shard-*' -D /tmp/wpt-run
  bun bench/wpt/gate.mjs update /tmp/wpt-run/*/*.json --replace --match '^dom/nodes/Node-foo\.html$'
  ```

  Or add the page to `flaky.json` with the reason (also needs the label).

`gate.mjs update` only adds: a subtest that passed in the baseline still does, a page whose runs are a regression
keeps its entry, and `--replace` needs `--match` or `--tests` so it cannot take a run's unrelated flakes with it. A
page's duration changes only in full runs and only if it moved by 2x and 1 s (`--durations` refreshes all). A run
that changes nothing leaves the file byte-identical (no artifact). One page per line, lists sorted:

```
"dom/nodes/Node-appendChild.html": {"status":"complete","ms":570,"n":11,"fail":[],"h":"611d0503"}
"html/dom/documents/dom-tree-accessors/Document.body.html": {"status":"timeout","ms":15000,"n":26,"pass":["..."]}
```

`fail` names the failing subtests of a completed page (all others passed) and `h` hashes the names it reported;
`pass` names the passing ones when that is shorter, the page did not complete, or its runs disagreed on the names.
Naming every pass would make the file ~38 MB instead of ~0.5 MB; `h` is what lets the gate tell that a page's
subtests changed, since a `fail` entry does not name its passes.

## Before making it a required check

The gate's false failures come from pages whose results depend on load. `flaky.json` starts with the six such pages
found in four full local runs and isolated reruns. To find the rest on the CI runners: start three manual runs
(`full`), one more with `runner-args: --jobs 1`, download their `wpt-shard-*` artifacts, and compare them:

```sh
bun bench/wpt/gate.mjs diff r1/*.json r2/*.json r3/*.json r4/*.json --suggest   # comma-separate a run's shards
```

Every page it lists goes in `flaky.json` with the reason (`--suggest` drafts the entries). Protect `bench/wpt/` and
`.github/workflows/wpt.yml` with CODEOWNERS: a pull request runs its own copy of the workflow and the gate scripts,
so the base-branch comparison catches accidental weakening of the baseline, not a changed gate.

## Run it locally

```sh
(cd bench && bun install)
bench/wpt/fetch-wpt.sh ~/wpt-lp                    # a new directory: the pinned revision, ~15 s
cargo build --profile release-fast                 # with the patched JavaScriptCore (scripts/jsc/build.sh)
bun bench/wpt/gate.mjs run --wpt ~/wpt-lp --bin target/release-fast/lightningpanda --out /tmp/wpt.json
```

`fetch-wpt.sh` only touches a directory it made (it moves that one to a new pinned revision in place); a WPT checkout
of yours is used as it is if it is at the pinned revision, and refused otherwise.

That runs the whole gate plan (minutes) and reruns what differs. Faster loops:

- `--dirs dom,css/selectors` or `--match 'dom/nodes/Node-'`: only those pages (no missing-page check).
- `--retest /tmp/wpt.json`: only the pages whose result in an earlier run differs from the baseline.
- `--full`: every page, as `main` runs it.

`bun bench/wpt/gate.mjs diff A.json B.json` lists the pages whose status or passing subtests differ between runs
(comma-separate shard files: `a0.json,a1.json`; give more than two sets to find what varies). To compare in CI, start
a manual run with `runner-args` (for example `--reuse-context`), download both runs' `wpt-shard-*` artifacts, and
diff them.

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
  2-5 of 584 pages per full local run, often 2-3 variants of one file together): the gate's 10 s bound and the
  reruns absorb them, since the page fails only if no run completes. If CI shows otherwise, an `allowIncomplete`
  pattern entry covers them.
