# WPT coverage gate

`.github/workflows/wpt.yml` runs our WPT subset (the directories in `lib.mjs` `DEFAULT_DIRS`, 3,541 pages) on every
pull request and fails when coverage goes down. `baseline.json` records, per page, which subtests pass, whether the
page completed, and how long it took. The same gate runs over WPT's reftests in the same directories and in 31 CSS layout
directories (18,156 pages, 62% of WPT's 29,212 reftests; `reftest-baseline.json`): see [Reftests](#reftests), which
also compares pyro's results with Chrome's.
[What runs](#what-runs) lists the testharness pages by directory. And it runs over WPT's crashtests (2,031 pages,
`crashtest-baseline.json`): see [Crashtests](#crashtests).

## How a run works

1. `prepare` (Linux, seconds): cache keys. The browser is keyed by every input of its build (the git object ids of
   `crates/`, `vendor/`, `Cargo.*`, `.cargo/`, `scripts/macos/` and `wpt.yml`, the stable toolchain
   and the patched JavaScriptCore), so a pull request that changes none of them skips the next two jobs.
2. `jsc`: the patched JavaScriptCore (`jsc.yml`), only when neither it nor the browser is cached. The gate tests the
   fork build because the system framework lacks window named properties and import maps: ~3,200 more subtests fail
   with it, for that reason alone.
3. `build`: `cargo build --profile release-fast` (dependencies cached by `Swatinem/rust-cache`, saved from `main`
   only), the patched JavaScriptCore linked in, saved to the cache.
4. `wpt`: one job on `lp-m1` (the matrix takes more shards: each takes its share of the pages, balanced by the
   baseline's durations). It restores the pinned WPT checkout (`fetch-wpt.sh`: revision `wpt-revision.txt`, only the
   paths in `wpt-paths.txt`, about 290 MB, cached) and runs the pages longest first at `--jobs 4`, the load the
   baseline was recorded under. It was four shards until 2026-11-15: the runner takes one job at a time, so they ran
   one after another and each paid about 25 s of setup. Every page whose result differs from the baseline then
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

## What runs

Both kinds run the same 18 directories: the ten testharness directories and the eight paint directories
(`lib.mjs` `PAINT_DIRS`). `discover()` takes every page that loads `/resources/testharness.js` with a script
element: `.any.js` and `.window.js` files (their window variant) and markup files (`.html`, `.htm`, `.xhtml`,
`.xht`, `.xml`, `.svg`). At the pinned revision that is 3,541 pages:

| directory | pages | testdriver | wptserve name | Python handler | no window global |
|---|---:|---:|---:|---:|---:|
| dom | 643 | 86 | 6 | 6 | 1 |
| html/dom | 246 | 2 | 9 | 5 | 1 |
| encoding | 1,258 | 0 | 1 | 4 | 0 |
| url | 48 | 0 | 0 | 1 | 0 |
| fetch/api/basic | 9 | 0 | 8 | 21 | 0 |
| xhr | 146 | 1 | 10 | 198 | 0 |
| css/selectors | 394 | 78 | 3 | 0 | 0 |
| uievents | 17 | 52 | 0 | 0 | 0 |
| shadow-dom | 185 | 129 | 0 | 0 | 0 |
| webstorage | 51 | 0 | 3 | 0 | 0 |
| css/css-backgrounds | 128 | 0 | 4 | 0 | 0 |
| css/css-text-decor | 48 | 0 | 0 | 0 | 0 |
| css/filter-effects | 37 | 0 | 0 | 0 | 0 |
| css/compositing | 15 | 0 | 0 | 0 | 0 |
| css/css-masking | 63 | 0 | 2 | 0 | 0 |
| css/css-color | 69 | 0 | 0 | 0 | 0 |
| css/css-transforms | 105 | 3 | 0 | 0 | 0 |
| css/CSS2 | 79 | 0 | 0 | 0 | 0 |
| **all** | **3,541** | 351 | 46 | 235 | 2 |

Left out, counted by reason in the run's `excluded`: pages that load testdriver, names that need wptserve
(`.sub.`, `.https.`, `.h2.`, `.www.`), pages that name a Python handler (`.py`) outside an HTML comment, and
`.any.js` files with no window global. Worker variants never run. A `.py` in a comment does not count (html/canvas's
generated pages name their generator there). Generated pages that name no handler themselves but need wptserve are
kept out by path: `referrer-policy/`, `content-security-policy/` and anything loading
`/common/security-features/`, `html/syntax/speculative-parsing/` and `html/semantics/**/moving-between-documents/`
(`SERVER_PAGES` in `lib.mjs`; none of these directories runs today).

The 728 pages added to the first 2,813 (2026-11-15):

- 541 testharness pages of the eight paint directories (parsing, computed values, interpolation, hit tests, CSS2
  geometry). They run as wptrunner runs every page, in an 800x600 viewport; the ten older directories keep the
  1280x720 their baseline was recorded with.
- 187 `.xml`, `.svg` and `.xht` pages that discovery used to drop without a count: css/selectors/old-tests 166
  (9 more load testdriver), dom 18, css/filter-effects 3.

In a local run of the 728 (not a baseline: CI records that), 727 completed, none crashed or hung the browser, 25,751
of 39,889 subtests passed and three runs agreed on every page. `dom/nodes/Element-firstElementChild-entity.svg`
times out (its internal DTD subset declares an entity).

The browser has Ahem for every page (`PYRO_FONTS`, as the reftests), as wptrunner installs it. `serve.mjs` answers
`/resources/WebIDLParser.js` with `resources/webidl2/lib/webidl2.js`, as wptserve routes it: before that every
idlharness page failed its one subtest (the eight older idlharness pages pass 2,309 subtests with it). A subtest name
that holds this server's origin (interpolation pages name the `url()` values they resolved) is recorded with
wptserve's origin, `http://web-platform.test:8000`, since the port differs from run to run.

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

## Merging main into a branch

Most branches touch the same few files, so a merge of main used to conflict in them every time. `.gitattributes` and
two scripts make those conflicts mechanical:

- `vendor/*/README.pyro.md` and `crates/pyro-render/tests/integration.rs` merge with git's `union` driver: the lines
  both sides added are kept, ours first. Read the result. A line both sides edited comes out twice, two sections both
  sides appended at the same place lose the blank line between them, and a line one side removed or renamed comes back
  when the other side added a line next to it (in `integration.rs` a `mod` line for a file that is gone: the build
  fails on it).
- `baseline.json` and `reftest-baseline.json` merge page by page (`merge-baseline.mjs`). A page only one side changed
  takes that side's entry (a loss accepted there stays accepted). A page both sides changed keeps every pass of either
  side; when two entries cannot be combined (`fail` entries of different subtest names), that page's two lines are
  left as a conflict. The gate checks the result as usual.
- `vendor/*/UPSTREAM.patch` still conflicts. `scripts/vendor/regen-patch --all` rewrites it from the merged sources
  and checks that `git apply` on the upstream reproduces them. It rebuilds the upstream from the patch committed on
  main's side of the merge (else at the merge base), so the conflict markers do not matter. A file that is as one side
  committed it keeps that side's section byte for byte: only the files both sides edited get new sections. It refuses
  a crate while a file in it has conflict markers, is unmerged in the index (a modify/delete conflict has no markers)
  or is untracked (a `*.orig` a merge tool left: delete it; a new file: `git add` it).

```sh
git fetch origin &&
  s=$(git show origin/main:scripts/vendor/setup-merge-drivers.sh) && sh -c "$s" &&  # the drivers, from any branch
  git -c attr.tree=origin/main merge --no-commit origin/main  # main's .gitattributes, on an older branch too (git 2.43+)
# Conflicts left in sources: resolve each and `git add` (or `git rm`) it; the UPSTREAM.patch files can stay conflicted.
# `git diff --name-only --diff-filter=U` lists what is left. Then:
scripts/vendor/regen-patch --all && git add vendor/*/UPSTREAM.patch && git commit --no-edit
```

Stage by name, not with `git add -u`: that marks every unmerged file resolved, conflict markers and all, and takes
unrelated edits into the merge. regen-patch exits 1 while a vendored crate has a conflict left, and `git commit`
refuses while any file is unmerged. Setting the drivers up once per clone is enough (the first line does it every
time, harmlessly); regen-patch warns in a merge when the baseline driver is missing (git then merged the baselines as
text, which can drop passes without a conflict).

`scripts/vendor/regen-patch --check --all` checks the patches without writing anything; CI runs it on every change to
`vendor/` against the revision before it (`.github/workflows/vendor-patches.yml`). `--fresh` regenerates every section
instead of keeping the committed ones. blitz-paint's, parley_engine's and stylo's sections are macOS `diff -u` output:
elsewhere the regenerated sections can align differently (they still apply).

## Linux and macOS

The testharness gate moves to GitHub's Linux runners (wpt.yml `build-linux`, `wpt-linux`, `gate-linux`; ubuntu-latest
x64, three shards), off the two lp-m1 Macs: testharness compares no pixels. Its baseline is `baseline-linux.json`,
bootstrapped from a CI run's `wpt-linux-baseline` artifact (never edited by hand). Until that file is committed the
Linux jobs only report and the macOS `wpt`/`gate` jobs keep gating; once it is committed, the macOS testharness jobs
stop. Reftests, crashtests and the client job stay on lp-m1.

`platform-diff.mjs baseline.json baseline-linux.json` lists every page whose results differ between the platforms
(also in `gate-linux`'s summary and its `wpt-platform-diff` artifact). A page that passes less on Linux is recorded,
with the reason, in `platform-split.json` (`{ "<url>": "<reason>" }`); `--strict` fails on any that is not, so
moving the gate never drops coverage silently.

## Before making it a required check

The gate's false failures come from pages whose results depend on load. `flaky.json` started with the six such pages
found in four full local runs and isolated reruns, plus two from the first CI runs (three runs without a baseline,
each running every page twice). Six of the eight, all in `html/dom/render-blocking`, were not flakes of the page
and are no longer listed: their scripts and stylesheets ask for `?pipe=trickle(d1)`, which `serve.mjs` ignored
(below). Three more pages the CI runs flagged were not flakes of the page and were never listed:
`dom/nodes/Node-isEqualNode-xhtml.xhtml` (a browser bug: an XML document's content was attached before its scripts
ran, fixed), `xhr/abort-during-navigation.window.html` (`delay.py` answered at once, below) and a long XHR page the
runner recorded as timed out when the browser was replaced under it (below). To find the rest on the CI runners:
start three manual runs (`full`), one more with `runner-args: --jobs 1`, download their `wpt-shard-*` artifacts, and
compare them:

```sh
bun bench/wpt/gate.mjs diff r1/*.json r2/*.json r3/*.json r4/*.json --suggest   # comma-separate a run's shards
```

Every page it lists goes in `flaky.json` with the reason (`--suggest` drafts the entries). Protect `bench/wpt/` and
`.github/workflows/wpt.yml` with CODEOWNERS: a pull request runs its own copy of the workflow and the gate scripts,
so the base-branch comparison catches accidental weakening of the baseline, not a changed gate.

## Run it locally

```sh
(cd bench && bun install)
bench/wpt/fetch-wpt.sh ~/wpt-pyro                    # a new directory: the pinned revision, ~15 s
cargo build --profile release-fast                 # with the patched JavaScriptCore (scripts/jsc/build.sh)
bun bench/wpt/gate.mjs run --wpt ~/wpt-pyro --bin target/release-fast/pyro --out /tmp/wpt.json
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

`serve.mjs` is a static server, so discovery leaves out the pages that name a wptserve Python handler (`.py`). It
cannot see a handler that only a page's support file requests, and a handler served as a file answers at once with
its source. The one such handler in the subset is answered as wptserve answers it (`HANDLERS` in `serve.mjs`):
`xhr/resources/delay.py`, which the XHR timeout pages and `xhr/abort-during-navigation.window.html` wait on.

Static files take wptserve's pipes (`?pipe=`, `pipes.mjs`; `bun test bench/wpt/pipes.test.mjs`): `trickle`,
`header`, `status`, `slice` and `gzip`, not `sub`. `trickle` is the one pages in the subset depend on:
`html/dom/render-blocking` loads its scripts and stylesheets with `trickle(d1)` (headers at once, the body a second
later), so that they are still loading when the page looks; `xhr/abort-after-timeout.any.html` needs a response
that outlasts its timeout. Until 2026-11-16 the pipe was ignored and the file sent at once: whether such a load had
finished when the page looked was a race with the page's frames (six pages were in `flaky.json` for it, and
`remove-attr-unblocks-rendering.optional.html` failed three CI runs in a row). The `header` pages in the subset
also need another origin (`get-host-info.sub.js`), and the one `gzip` page fails either way. Reftest and crashtest
discovery still leaves out every page that names a `pipe=`.

A page that spins the browser's main thread is recorded as `hang` 15 s after its timeout, and the browser is
replaced (`css/selectors/invalidation/has-complexity.html` in every full run; pull requests skip it). The pages
that were running in that browser run again: their waits fail with the connection, which used to be recorded as a
timeout with no result, for a page that was about to complete too.

## Timings

`run.mjs` prints progress every 100 pages or 30 s. A watchdog replaces the browser when a worker is stuck on a page
for its limit plus 75 s once the page has started (not while it waits for a solo page or a restart; outside the per-page guard: a context that never closes, a browser that never starts) and
logs the page (`watchdogKills` in the results). `WPT_TRACE=FILE` appends `start`/`end` lines per page, to find a hang
afterwards. The one apparent hang of a wptserve sample (2026-11-17, 213 pages at `--jobs 2`, no output for 11 min)
was the old progress interval: the same pages in chunks of 20 took 670 s for 160 of them, and none hung.

The slowest pages are slow by design or by a known engine gap, not by the runner:

- `dom/nodes/NodeList-static-length-getter-tampered-*.html` (6 long pages, 22-29 s each on a loaded machine): 50 x
  50,000 loops over a 100-node `NodeList`, ~1.3e8 indexed reads and `length` gets, each a call into our bindings. It
  is a JIT test; making it fast means indexed access the JIT can inline, not a runner change.
- `xhr/xmlhttprequest-timeout*.html` (37 variants, 5-22 s each, 355 s in all): the tests wait on their own 5 s timers
  and on `delay.py`'s delayed responses and use CPU only briefly, which is why each shard runs one page more than it
  has CPUs.
- Pages ending at 10 s complete by testharness's own timeout; those with nothing passing are skipped by the gate.
- `encoding/legacy-mb-*/*-encode-form-*.html?...` occasionally never reported in full local runs (45 s timeouts in
  2-5 of 584 pages per run, often 2-3 variants of one file together). That is what a browser replacement did to the
  pages running beside the page that hung (above), before the runner ran them again; whether all of them were that
  is not known. The gate's 10 s bound and the reruns absorb the rest, since a page fails only if no run completes.
  If CI shows otherwise, an `allowIncomplete` pattern entry covers them.

## Reftests

WPT tests painting with reftests: a test page and one or more references that must render the same (`<link
rel=match>`) or differently (`rel=mismatch`). `run.mjs --kind reftest` runs them and `gate.mjs ... --kind reftest`
gates them as above: each reftest is reported as one subtest, `reftest`, so a reftest that passed in
`reftest-baseline.json` must still pass in some run of it, an unrecorded pass fails as stale, and `reftest-flaky.json`
(the format of `flaky.json`) lists what is not checked. Only passes are protected: a failing reftest's completion
carries no coverage, so it may time out instead (an animation page whose `reftest-wait` outlasts a loaded runner did,
four runs in a row) and a timed-out one that now completes and fails is no gain. Adding or widening a
`reftest-flaky.json` entry, or dropping a baseline pass, needs the `wpt-accept-loss` label. The refreshed baseline is
the `wpt-reftest-baseline` artifact:

```sh
gh run download <run-id> -n wpt-reftest-baseline -D bench/wpt     # writes bench/wpt/reftest-baseline.json
```

### What runs

`lib.mjs` `discoverReftests()` classifies files as WPT's manifest does (`tools/manifest/sourcefile.py`): a markup file
(`.html`, `.htm`, `.xht`, `.xhtml`, `.xml`, `.svg`) with a `match` or `mismatch` link is a reftest, unless it is a
manual, visual, crash or print test, a testharness page, or under a `resources`, `support` or `tools` directory. The
directories are `REFTEST_DIRS`: the paint directories with all of CSS2 (it costs about 50 s of the run), the five
testharness directories that hold reftests (501: selector invalidation, `dir=auto` and bidi, shadow trees), and the 31
layout directories (`LAYOUT_DIRS`, 8,314: flexbox, grid, sizing, position, tables, text, fonts, inline, overflow and
the smaller style directories). At the pinned revision they hold 18,156 reftests, 62% of the 29,212 in WPT's
manifest (32% before the two additions of 2026-11-15). In the first 13 directories wpt.fyi lists 9,855: the same
9,842, and the 13 left out below for wptserve or testdriver.

| directory | reftests | reference files | no reference | crash tests | manual | print | wptserve | testdriver |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| css/css-backgrounds | 732 | 406 | 104 | 14 | 0 | 0 | 1 | 0 |
| css/css-text-decor | 337 | 251 | 13 | 9 | 65 | 0 | 0 | 1 |
| css/filter-effects | 372 | 241 | 0 | 23 | 0 | 0 | 0 | 0 |
| css/compositing | 60 | 56 | 11 | 6 | 0 | 0 | 0 | 0 |
| css/css-masking | 494 | 238 | 4 | 6 | 0 | 0 | 8 | 0 |
| css/css-color | 308 | 148 | 0 | 3 | 0 | 0 | 0 | 0 |
| css/css-transforms | 797 | 311 | 4 | 28 | 1 | 0 | 0 | 0 |
| css/CSS2 | 6,241 | 2,099 | 2,314 | 32 | 4 | 43 | 1 | 0 |
| css/selectors | 341 | 157 | 1 | 20 | 2 | 0 | 0 | 1 |
| html/dom | 127 | 111 | 2 | 3 | 1 | 0 | 0 | 0 |
| shadow-dom | 25 | 21 | 0 | 43 | 0 | 0 | 0 | 1 |
| dom | 4 | 3 | 65 | 44 | 0 | 0 | 0 | 0 |
| encoding | 4 | 4 | 34 | 2 | 0 | 0 | 0 | 0 |
| **the 13 above** | **9,842** | 4,046 | 2,552 | 233 | 73 | 43 | 10 | 3 |
| css/css-flexbox | 1,020 | 546 | 9 | 29 | 8 | 3 | 0 | 0 |
| css/css-sizing | 566 | 119 | 0 | 8 | 0 | 0 | 3 | 0 |
| css/css-position | 248 | 100 | 0 | 28 | 0 | 3 | 2 | 0 |
| css/css-tables | 169 | 85 | 0 | 42 | 0 | 4 | 0 | 0 |
| css/css-display | 222 | 55 | 92 | 3 | 0 | 0 | 0 | 0 |
| css/css-align | 64 | 47 | 1 | 0 | 0 | 0 | 0 | 0 |
| css/css-box | 42 | 7 | 0 | 0 | 0 | 0 | 0 | 0 |
| css/css-overflow | 697 | 455 | 21 | 51 | 0 | 0 | 0 | 3 |
| css/css-images | 474 | 248 | 4 | 6 | 0 | 0 | 8 | 0 |
| css/css-values | 171 | 83 | 2 | 22 | 1 | 2 | 41 | 0 |
| css/css-variables | 183 | 20 | 0 | 4 | 0 | 0 | 0 | 0 |
| css/css-cascade | 44 | 18 | 0 | 7 | 1 | 7 | 0 | 0 |
| css/css-conditional | 172 | 32 | 0 | 63 | 0 | 0 | 1 | 0 |
| css/css-nesting | 17 | 8 | 0 | 6 | 0 | 0 | 0 | 0 |
| css/css-lists | 198 | 161 | 2 | 17 | 0 | 0 | 0 | 0 |
| css/css-pseudo | 279 | 233 | 1 | 26 | 7 | 0 | 0 | 0 |
| css/css-content | 66 | 56 | 0 | 6 | 0 | 0 | 0 | 0 |
| css/css-logical | 6 | 6 | 0 | 0 | 0 | 0 | 0 | 0 |
| css/cssom | 12 | 9 | 0 | 19 | 0 | 0 | 0 | 0 |
| css/cssom-view | 11 | 7 | 2 | 4 | 0 | 0 | 0 | 0 |
| css/mediaqueries | 58 | 9 | 3 | 1 | 0 | 0 | 0 | 0 |
| css/css-syntax | 1 | 1 | 4 | 3 | 0 | 0 | 0 | 0 |
| css/css-shadow | 113 | 17 | 0 | 3 | 0 | 0 | 0 | 1 |
| css/css-transitions | 6 | 5 | 0 | 13 | 55 | 0 | 0 | 0 |
| css/css-animations | 36 | 25 | 0 | 46 | 66 | 0 | 0 | 0 |
| css/css-properties-values-api | 7 | 3 | 0 | 7 | 0 | 0 | 0 | 0 |
| css/css-grid | 826 | 1,161 | 4 | 43 | 0 | 1 | 0 | 2 |
| css/css-text | 1,621 | 1,059 | 26 | 41 | 2 | 0 | 4 | 0 |
| css/css-fonts | 378 | 333 | 4 | 10 | 0 | 2 | 6 | 0 |
| css/css-inline | 373 | 170 | 0 | 7 | 0 | 3 | 0 | 0 |
| css/css-borders | 234 | 116 | 1 | 3 | 0 | 0 | 0 | 0 |
| **layout directories** | **8,314** | 5,194 | 176 | 518 | 140 | 25 | 65 | 6 |
| **all** | **18,156** | 9,240 | 2,728 | 751 | 213 | 68 | 75 | 9 |

In a local run of the 501 added on 2026-11-15 (not a baseline), every page completed and 376 passed (css/selectors
294, html/dom 59, shadow-dom 16, dom 3, encoding 4); Chrome passes 481.

`css/css-grid/grid-lanes/` is left out by path (`lib.mjs` `REFTEST_SKIP`, counted in `excluded`): 911 reftests of
masonry layout, which pyro does not implement and of which Chrome passes 120. A failing page still runs on every pull
request (only one that times out with nothing passing is skipped), so they would cost about 5% of the reftest run and
protect nothing. Two css-fonts tests name a reference that WPT does not have (`reference not in the checkout`). The
layout directories' testharness pages are not run: 4,771 of them can run, more than the 3,541 pages of the whole
testharness run today.

A local run of the layout directories (2026-11-15, one pass at 3 jobs on a loaded machine, not a baseline; the
baseline comes from CI) found no crash, no hang and no browser restart. 5,203 of the 8,314 pass; Chrome passes 7,164:

| directory | reftests | pyro (local) | Chrome | did not complete |
|---|---:|---:|---:|---|
| css/css-flexbox | 1,020 | 919 | 1,006 | timeout 2 |
| css/css-sizing | 566 | 358 | 541 |  |
| css/css-position | 248 | 110 | 247 |  |
| css/css-tables | 169 | 116 | 160 |  |
| css/css-display | 222 | 107 | 149 | timeout 1, error 1 |
| css/css-align | 64 | 45 | 61 |  |
| css/css-box | 42 | 2 | 42 |  |
| css/css-overflow | 697 | 207 | 410 | timeout 4 |
| css/css-images | 474 | 252 | 376 | timeout 3 |
| css/css-values | 171 | 140 | 165 |  |
| css/css-variables | 183 | 182 | 179 |  |
| css/css-cascade | 44 | 36 | 44 |  |
| css/css-conditional | 172 | 144 | 169 | timeout 3 |
| css/css-nesting | 17 | 12 | 17 |  |
| css/css-lists | 198 | 111 | 125 |  |
| css/css-pseudo | 279 | 107 | 251 |  |
| css/css-content | 66 | 34 | 62 |  |
| css/css-logical | 6 | 1 | 6 |  |
| css/cssom | 12 | 8 | 9 |  |
| css/cssom-view | 11 | 9 | 11 |  |
| css/mediaqueries | 58 | 42 | 56 |  |
| css/css-syntax | 1 | 1 | 1 |  |
| css/css-shadow | 113 | 23 | 94 | error 2 |
| css/css-transitions | 6 | 5 | 6 | timeout 1 |
| css/css-animations | 36 | 26 | 33 |  |
| css/css-properties-values-api | 7 | 6 | 7 |  |
| css/css-grid | 826 | 673 | 814 |  |
| css/css-text | 1,621 | 1,115 | 1,328 | timeout 1 |
| css/css-fonts | 378 | 192 | 323 | timeout 10 |
| css/css-inline | 373 | 75 | 287 |  |
| css/css-borders | 234 | 145 | 185 |  |
| **layout directories** | 8,314 | 5,203 | 7,164 | timeout 25, error 3 |

The 25 timeouts are pages whose `reftest-wait` is never removed, for features pyro lacks: `font-palette` and
`@font-palette-values` (10), scroll markers (4), embedded SVG documents that change (3), container queries on
pseudo-elements and cues (3), and `flex-item-z-ordering-003/-004`, `display-change-object-iframe`,
`pseudo-element-transform` and `text-fit/selection-highlight-painting`. They pass nothing, so pull requests skip them.
The 3 errors are one pyro bug, not a crash: `Page.captureScreenshot` answers `the page's tree has a repeated key` on
`css/css-display/display-contents-shadow-dom-1.html`, `css/css-shadow/reslot-text-inheritance.html` and
`css/css-shadow/shadow-fallback-dynamic-004.html` (all three change a shadow tree's slotting after load). The support files these pages ask for
and do not get are missing in WPT too (`/fonts/DoulosSIL-R.woff`, `/fonts/GentiumPlus-R.woff2`,
`css/css-pseudo/highlight-cascade/support/selections.js` and ten others).

Left out, counted by reason in the run's `excluded`: reference files (named or linked as one, no links of their own),
markup with no reference (CSS2's visual tests, support pages), crash tests, manual tests (by name, or by CSS flags
such as `interact`), print reftests, pages whose page or references need what `serve.mjs` lacks (`.sub` substitutions,
`pipe=`, Python handlers), and testdriver. Testharness pages in these directories are not reftests: the testharness run takes
them ([What runs](#what-runs)). A reference that has references of its own (155 tests) is compared one level deep, as
wptrunner does at this revision (`ReftestTest.from_manifest`); it is a test of its own as well, so the chain is covered
link by link. `about:blank` references are supported.

### How a reftest is judged

As wptrunner does (`executors/base.py`, `test-wait.js`), in `reftest.mjs`:

- The browser runs as wptrunner runs Chrome (`chrome.py`, `--install-fonts`): text is drawn without antialiasing
  (Chrome's `--enable-blink-features=NoFontAntialiasing`; pyro's `PYRO_TEXT_ANTIALIASING=off`, aliased glyphs only),
  and Ahem is installed (`PYRO_FONTS=<wpt>/fonts/Ahem.ttf`, pyro's `--font`: the checkout's font file, nothing goes
  into the system's font folders, locally or on the runners). `reftest.mjs` `reftestEnv` sets both; a pyro without
  these options ignores them.
- Test and references render in an 800x600 CSS px viewport at device scale factor 1 (a `<meta name=viewport-size>`
  sets another size), each in the browser under test.
- The screenshot is taken after the load event, `document.fonts.ready` (after a forced layout: pyro requests a web font
  when layout first needs it) and two animation frames. With `class="reftest-wait"` on the root element, the runner
  fires `TestRendered` at it and waits for the class to go, two frames after each change of the root's attributes, up
  to the page's timeout (`timeout` with the message `reftest-wait was not removed`). The frames take real time, as
  Chrome's 60 Hz frames do for WPT's runner: each is a `requestAnimationFrame` callback 16.7 ms after the previous
  wait began (pyro runs frames back to back after each client command, which would capture a page's animations
  within a millisecond of their start).
- Two renders are equal when no pixel differs in R, G or B. `<meta name=fuzzy content="[ref:]maxDifference=a-b;
  totalPixels=c-d">` (names optional; a single value n means n-n) allows renders whose largest channel difference
  and number of differing pixels fall in the inclusive ranges, or that do not differ at all when a range starts at 0.
- A test passes when it equals at least one of its `match` references (if it has any) and none of its `mismatch`
  references. The subtest's message gives each comparison: `== ref.html: 1234 px differ, max 255`.
- References are cached by URL and viewport (48 renders per runner), as wptrunner caches screenshots.
- pyro answers `Page.captureScreenshot` with `format: "rgba"` (no encoding); another browser's PNG is decoded.
- A page whose browser died under it runs again alone. If it loses the browser again it is recorded as `crash` (at
  the pinned revision `css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg` crashes pyro); the pages that
  ran beside it get their own results.

`REFTEST_SHOTS=DIR` writes the renders of every failing test and its references to DIR as PNG.

### CI

The `reftest` job runs `gate.mjs run --kind reftest` in one shard on the self-hosted `lp-m1`, one page per CPU, and
`reftest-gate` checks the result on Linux, as `wpt` and `gate` do. They share the `prepare` and `build` jobs (the
browser, built on `lp-m1`) and the WPT checkout (`wpt-paths.txt` now lists the reftest directories and their support
files; about 290 MB with its git directory). Manual runs take `suites` (`reftest` leaves the testharness shards and the client job out),
`reftest-runner` (`macos-15`: four shards on GitHub's hosted runners, billed as macOS minutes) and `reftest-args` (for
example `--dirs css/CSS2 --no-rerun` to time a directory).

The layout directories (8,314 reftests) about double the reftest job, and more shards cannot shorten it: `lp-m1`
takes one job at a time, so shards there run one after another and each adds its setup. Its run step took 129 to
132 s for 9,842 pages in three runs (job 154 to 160 s); with 18,156 pages it is about 240 s (est., at the same 13 ms
of wall time per page; job about 4.5 min). The testharness run became one job at the same `--jobs 4` to pay for most
of it: four jobs of 67 to 73 s (272 to 286 s) become one of about 200 s (est.: 25 s of setup, four times 43 s of
pages). A pull request's jobs on `lp-m1` go from about 8.2 min to about 8.8 min (est.), without the crashtest job.
The first run with the new pages costs about 2.5 min more, once: each new page that passes runs again, and the 25
pages that time out take 15 s each before the gate skips them.

The first 2026-11-15 additions needed no more shards (est., from CI's means of about 85 ms per reftest and 270-320 ms per
testharness page): 501 reftests are about 43 s of page time, 5 to 10 s of the one shard at 8 jobs; 728 testharness
pages are about 200-235 s of page time, 12 to 15 s on each of the four shards at 4 jobs (locally they took 35 s of page
time, plus one 15 s timeout that pull requests skip). The first run after pages are added runs each new page that
passes once more, so it costs about twice that.

Measured on the pull request that added the job (all 9,341 reftests; a run without a baseline runs every page twice):

| runner | shards x jobs | one pass | job, with setup | two passes agreed |
|---|---|---:|---:|---|
| `macos-15` (GitHub: M2 Pro, 5 vCPUs, 14 GB, macOS 15.8) | 2 x 5 | 53 s, 65 s | 2:04, 2:22 | every page |
| `macos-15` | 3 x 5 | 34-37 s (70 s with the crash) | 1:28-2:35 | every page |
| `lp-m1` (M1, 8 CPUs, 8 GB, macOS 26.7) | 1 x 8 | 72 s (9,300 pages), 115-131 s with the timeouts' tail | 4:55 | all but 5 pages, which a runner bug (since fixed) recorded as crashes |

Since 2026-11-18 `lp-m1` also includes `worker-4-studio` (Mac Studio, M5 Max, 18 cores, 36 GB). Its runner `.env` sets `PYRO_CI_CPUS=4`, and every CPU-sized `--jobs`/`-j` in the workflows reads `PYRO_CI_CPUS` before `hw.ncpu`, so it does about an M1 runner's share; it runs at nice 10.
| `macos-15`, a pull request's gate run (baseline committed) | 2 x 5 | 35 s, 40 s | 0:52, 0:55 | (2 pages rerun) |

The 11 pages that time out (15 s each) and the crash run last in a run without a baseline; a pull request's run skips
them (nothing to protect), so its pass costs about what the first 9,300 pages do. Within each bootstrap run on
`macos-15` the two passes agreed on every page, the runs differed only on two transition pages (below), and `lp-m1`'s
results matched theirs on every other page but the two that runner bug recorded as crashes. The job ran on `macos-15`
until every macOS job moved to `lp-m1`: there it adds 2 to 5 minutes to each run's turn on a runner that runs one job
at a time and already queues the four testharness shards and the client job of every pull request, where `macos-15`
ran two jobs of 1 to 2.5 minutes beside them (2 to 5 runner minutes per run, billed as macOS minutes). The baseline
was first recorded on `macos-15`; the runners agreed, but a timing-dependent page can still tell them apart.

`lp-m1` has no `zstd`, so its Actions cache entries are gzip archives; a runner with `zstd` looks for another cache
version and misses them. The reftest job restores the WPT checkout and the browser with a `PATH` that hides `zstd`,
so both runners share the entries `lp-m1` saves.

Flaky: `css/css-color/animation/opacity-animation-ending-correctly-001.html` and `-002.html` wait in `reftest-wait`
for a 50 ms transition's end. -001 timed out in both passes of one of four runs; in the first gate run both timed out
in their first run (10 s bound) and passed in all five reruns. They are not in `reftest-flaky.json` (that needs the
`wpt-accept-loss` label): a run in which one times out every time fails the gate. The entry they would need:
`"css/css-color/animation/opacity-animation-ending-correctly-*": {"reason": "...", "allowIncomplete": true}`.

### pyro vs Chrome

`chrome-compare.mjs` compares pyro's reftest results (the committed baseline, or a run's shard files) with Chrome's on
[wpt.fyi](https://wpt.fyi), per directory, and lists the reftests Chrome passes and pyro fails by feature: the work
queue for paint fixes. wpt.fyi has no Chrome stable run at the pinned revision; the script takes the stable run
nearest to it in WPT's history (GitHub's compare API: `GH_TOKEN` raises the rate limit), with Firefox and Safari stable
at that run's revision. `--canary` uses Chrome's experimental run at the exact revision instead, `--run ID` any run.

```sh
GH_TOKEN=$(gh auth token) bun bench/wpt/chrome-compare.mjs [shard files] [--md out.md] [--json out.json]
```

At the pinned revision (pyro: `reftest-baseline.json`; Chrome 157.0.8103.38 stable, wpt.fyi run 5478394868006912 at
WPT 1eecaa0c30, 10 commits before the pin; Firefox 159.0.1 and Safari 27.2 stable at the same revision):

| directory | reftests | pyro | Chrome | Firefox | Safari | Chrome passes, pyro fails | pyro passes, Chrome fails |
|---|---:|---:|---:|---:|---:|---:|---:|
| css/css-backgrounds | 732 | 485 (66.3%) | 630 (86.1%) | 665 (90.8%) | 624 (85.2%) | 237 | 92 |
| css/css-text-decor | 337 | 115 (34.1%) | 256 (76.0%) | 288 (85.5%) | 148 (43.9%) | 146 | 5 |
| css/filter-effects | 372 | 175 (47.0%) | 340 (91.4%) | 319 (85.8%) | 262 (70.4%) | 173 | 8 |
| css/compositing | 60 | 28 (46.7%) | 55 (91.7%) | 49 (81.7%) | 41 (68.3%) | 30 | 3 |
| css/css-masking | 494 | 212 (42.9%) | 460 (93.1%) | 468 (94.7%) | 421 (85.2%) | 253 | 5 |
| css/css-color | 308 | 272 (88.3%) | 300 (97.4%) | 298 (96.8%) | 284 (92.2%) | 29 | 1 |
| css/css-transforms | 797 | 439 (55.1%) | 791 (99.2%) | 789 (99.0%) | 772 (96.9%) | 354 | 2 |
| css/CSS2 | 6241 | 4694 (75.2%) | 6114 (98.0%) | 6103 (97.8%) | 5972 (95.7%) | 1470 | 50 |
| **all** | 9341 | 6420 (68.7%) | 8946 (95.8%) | 8979 (96.1%) | 8524 (91.3%) | 2692 | 166 |

A reftest passes in pyro when pyro renders the test like its reference; it can pass while pyro paints both wrong (a
reference drawn with the same feature), which `bench/fidelity/paint.ts` checks against Chromium instead. The work
queue, reftests Chrome passes and pyro fails, by feature (`chrome-compare.mjs` lists every one):

Paint (the paint directories and CSS2's backgrounds, borders, colors, visufx, ui, zindex, zorder, stacking-context): 1334.

| feature | reftests | examples |
|---|---:|---|
| clip-path | 158 | `clip-path-clip-nested-twice.svg`, `clip-path-clip-rule-003.svg`, `clip-path-clip-rule-004.svg` |
| transforms: 2D | 117 | `css-transform-animate-translate-implied-y.html`, `css-transforms-transformlist.html`, `svg-document-styles-001.html` |
| transforms: 3D | 114 | `3d-rendering-context-and-inline.html`, `3d-scene-with-iframe-001.html`, `3dtransform-and-filter-no-perspective-001.html` |
| SVG filters (url(), fe*) | 86 | `effect-reference-add-hw-001.html`, `effect-reference-displacement-negative-scale-001.html`, `effect-reference-feimage-001.html` |
| transform-origin, transform-box | 81 | `content-box-mutation-001.html`, `cssbox-content-box-001.html`, `cssbox-content-box-002.html` |
| animations, transitions | 79 | `background-color-animation-custom-property.html`, `background-color-animation-custom-timing-function-reverse.html`, `background-color-animation-custom-timing-function.html` |
| text-decoration | 68 | `ruby-text-decoration-01.html`, `text-decoration-inset-002.html`, `text-decoration-inset-003.html` |
| border-image | 64 | `border-image-002.html`, `border-image-003.html`, `border-image-004.html` |
| text-emphasis | 63 | `text-emphasis-color-property-001.html`, `text-emphasis-color-property-001a.html`, `text-emphasis-color-property-001b.html` |
| mask | 62 | `clip-fixed-pos-transform-descendant-001.html`, `bad-mask-image-svg.html`, `mask-clip-1.html` |
| borders | 54 | `background-position-subpixel-at-border.tentative.html`, `gradient-repeat-spaced-with-borders.html`, `ttwf-reftest-borderRadius.html` |
| backgrounds (other) | 45 | `background-color-body-propagation-004.html`, `background-color-body-propagation-007.html`, `background-color-body-propagation-008.html` |
| backdrop-filter | 40 | `backdrop-filter-3d-transform-perspective.html`, `backdrop-filter-backdrop-root-backdrop-filter.html`, `backdrop-filter-backdrop-root-clip-path-2.html` |
| background-clip, background-origin | 36 | `background-clip-border-area-and-blend-mode.html`, `background-clip-content-box-with-border-radius-002.html`, `background-clip-content-box-with-border-radius-003.html` |
| box-shadow | 35 | `box-shadow-041.html`, `box-shadow-042.html`, `box-shadow-border-radius-001.html` |
| background-attachment | 29 | `background-attachment-350.html`, `background-attachment-353.html`, `background-attachment-local-hidden.html` |
| background-position, background-repeat | 24 | `background-position-right-in-body.html`, `subpixel-position-center.tentative.html`, `background-repeat-round-2.html` |
| mix-blend-mode | 23 | `mix-blend-mode-blended-element-overflow-scroll.html`, `mix-blend-mode-blended-element-with-transparent-pixels.html`, `mix-blend-mode-blended-with-3D-transform.html` |
| colors (other) | 20 | `color-svg-visited.html`, `deprecated-sameas-018.html`, `deprecated-sameas-019.html` |
| border-radius | 17 | `background-attachment-fixed-border-radius-offset.html`, `background-image-centered-with-border-radius.html`, `border-radius-001.xht` |

17 more paint features have 119 between them. CSS2's other subdirectories account for the other 1,358: selectors 380
(339 of them `first-letter-punctuation-*`), tables 199, normal-flow 127, lists 120 (counters and the `*-applies-to`
pages), generated-content 105, text 101, bidi-text 55, positioning 48 and fewer elsewhere.

## Coverage

`coverage.json` counts every test in WPT's own manifest (`MANIFEST.json`, version 9, as wpt.fyi publishes it per
revision) at the pinned revision, by kind (the manifest's item type) and area (`css/<spec>`, `html/<section>`,
`svg/<section>`, else the top-level directory). Each test is `gate` (CI runs it today), `runnable` (this runner's
discovery takes it, but no CI plan runs it yet) or not run for a reason (`notRun`: discovery's reason, or the kind has
no runner: print reftests, manual, visual, wdspec, aamtest, test262). `total = gate + runnable + sum(notRun)`; every
URL counts once (variants separately). The status site reads it for its "not run" denominators.

```sh
bench/wpt/fetch-wpt.sh --all ~/wpt-full                     # the whole revision, about 1.1 GB, ~90 s
bun bench/wpt/coverage.mjs --wpt ~/wpt-full                 # downloads the manifest from wpt.fyi once (40 MB)
```

At a421f2fe, of 132,204 manifest items: testharness 39,989 (gate 3,537, runnable 19,495), reftest 29,212 (gate
18,158, runnable 9,640), crashtest 2,105 (gate 2,031). Not run: testharness worker variants 4,320, wptserve names
7,048, Python handlers 2,748, testdriver 2,701; reftests needing wptserve 418, testdriver 83, masonry 911; and
test262 53,814, manual 3,047, visual 2,714, wdspec 685, print reftests 448, aamtest 190. `discoveredNotInManifest`
lists what discovery takes that the manifest classifies otherwise (45 `-crash` pages as testharness, 4 SVG reftests):
none of them is in a gate directory but four crashtests the testharness gate has always run. Top-level `tools/`,
`resources/` and `conformance-checkers/` hold no tests (`lib.mjs` `NON_TEST_TOP`).

`run.mjs --dirs all` runs every directory of a whole checkout (`allDirs()`: the kind's own directories first, so their
pages keep their rules, then the fewest others that cover the rest). Testharness pages outside the ten older
directories run in wptrunner's 800x600. It finds 23,077 testharness pages and 27,802 reftests: the `gate` and
`runnable` counts above, plus the 49 in `discoveredNotInManifest`. A local sample of 1 in 40 of the pages no gate runs
(2026-11-17, `--jobs 2`, load 33, CI browser of a028229b; not a baseline) found no crash, no hang and no browser
restart: testharness 487 of 492 completed (5 timeouts, 285 s wall, 1.06 s per page), reftests 233 of 242 completed
(9 timeouts, 108 s wall, 0.70 s per page). Timeouts are about half the page time.

## wptserve

`run.mjs --server wptserve` (and `gate.mjs run --server wptserve`) serves the pages with WPT's own server instead of
`serve.mjs` (`wptserve.mjs`): every origin a test names (`web-platform.test`, `www`/`www1`/`www2` and the IDN
subdomains, `not-web-platform.test`; http, https with WPT's test certificates, ws, wss; no h2), the Python handlers,
substitutions and all pipes. Discovery then keeps the pages that need them: at a421f2fe testharness 23,077 -> 31,590
pages, reftests 27,802 -> 28,219, crashtests 2,031 -> 2,076 (with `--dirs all`). Testdriver pages stay out.

- wptserve binds 127.0.0.1 (`server_host`, `check_subdomains: false`), so no hosts file is needed. The browser
  reaches the names through a forward proxy (`startProxy()`) that every context gets as its `proxyServer`: it
  connects any host a request names to 127.0.0.1 on the same port, what wptrunner's `--host-resolver-rules` does for
  Chrome. Contexts ignore certificate errors (WPT's test CA).
- Every port is picked free per run; subtest names that hold one are recorded with wptserve's default port (8000,
  8001, 8443, 8444, 8888, 8889), as `serve.mjs`'s origin already is.
- `testharnessreport.js` is ours (a file alias in the generated config), as wptrunner serves its own.
- It needs Python 3.9+ with `tools/serve/requirements.txt` (today `cryptography`): `fetch-wpt.sh --venv DIR` makes
  `DIR/.git/pyro-venv`, which `wptserve.mjs` uses (`WPT_PYTHON` overrides). macOS's `/usr/bin/python3` is 3.9.6.
- It covers what `serve.mjs` does by hand: `/resources/WebIDLParser.js`, `?pipe=trickle`, `xhr/resources/delay.py`.
- Pages load from the origin their name asks for, as wptrunner loads them: `.https.` and `.serviceworker.` over https,
  `.www.` from the www subdomain.
- https through wptserve needed a pyro-net fix (2026-11-17): its responses carry no `Content-Length` and end when
  the connection closes, and Python's TLS closes without `close_notify`, which pyro read as a cut body (every https
  load failed with `error reading a body from connection`). Such an end now reads as the end of the stream, as in
  Chromium. In a sample of 212 of the pages wptserve adds (1 in 40, `--jobs 2`): before, 27 failed to load and 25
  timed out (6.6 s per page); after, 202 completed, 10 timed out, none failed to load, 113 of 118 `.https.` pages
  completed, 3.4 s per page, 366 s wall, no browser restart. 51 pages passed every subtest, 1,535 of 2,716
  subtests passed.

The PR gate keeps `serve.mjs` and its plan until a CI run records baselines for the whole suite.

## Whole suite

`.github/workflows/wpt-full.yml` runs everything nightly (01:17 UTC, and by hand): `gate.mjs run --dirs all
--server wptserve --testdriver` for each kind, 4 testharness, 3 reftest and 1 crashtest shards on lp-m1 (or
macos-15), with the newest main build's `wpt-browser`. Testharness shards run 16 pages side by side (the pages mostly
wait: the browser used 12-14% of one core at 4), reftests and crashtests one per CPU. Scheduled runs rerun every
page, record the whole suite's baselines (`wpt-full-baselines`: one directory per kind, one file per top-level WPT
directory) and, on main, export them to the status site (`status` job: the commit's run on `wpt-results` becomes
the gates' pages plus the whole suite's). A scheduled run queues behind a running manual one (only pull request runs
cancel). Committed whole-suite baselines live in `bench/wpt/full/<kind>/`; a main push's export reads them too.
Nothing gates on them yet.

The first nightly (44215242259, `--jobs 4`, no reruns): testharness 4 shards of ~1 h (2.5 h wall on lp-m1's two
machines), reftests 12-14 min per shard, crashtests 6 min. Of the testharness page time, 41% went to pages that did
not complete and 80% to pages with no passing subtest (referrer-policy 130 min, wasm 38, html/semantics 34,
content-security-policy 33, mixed-content 29). Reftests: webvtt alone 35 min (138 of 168 time out). 8 testharness
hangs, 1 renderer panic in a reftest screenshot, 1 crashtest panic. 88 testharness pages in a row failed with
`ERR_PROXY_CONNECTION_FAILED`: the machine ran out of ephemeral ports.
`html/editing/the-hidden-attribute/beforematch-scroll-to-text-fragment.html` (and the scroll-to-text-fragment pages
that share `stash.js`) polls `stash.py` in a loop with no delay for a popup's results that never come in pyro,
~1,000 requests a second for its 45 s; each opens a new connection from wptserve to its stash manager (a Python
`multiprocessing` manager over TCP), and macOS's 16,384 ephemeral ports (49152-65535, TIME_WAIT 30 s) run out: about
24,500 sockets in TIME_WAIT from that page alone. Pyro reuses its one connection to the proxy (3,644 requests on 1
connection). The runner now waits until a connect works again and runs the failed page again. Widening the port range
on the runners (`sysctl net.inet.ip.portrange.first=16384`) would remove the cause. Reftest shard 0 lost its results to an artifact-service timeout; uploads run again.

At `--jobs 16` with reruns (run 44235329366, testharness shard 3 of 4): 13.5 min per pass, 28 min for both (51 min
for one pass at 4), crashtests 11.5 min for both. Against the nightly's 4-job pass of the same shard: 18 pages
passed fewer subtests (1,284 of 422,251), nearly all `wasm/core/simd` pages that timed out under the load; `wasm/core/`
pages get the long timeout since. Estimated wall for a scheduled run with reruns on lp-m1's two machines: ~85 min
(4 x 28 testharness, 3 x ~16 reftest, 12 crashtest).

The committed `bench/wpt/full/` comes from run 44235329366 (`--jobs 16`, reruns; 2026-11-18), `gate.mjs update
--missing-shards` over its CI shard results: testharness shard 0 of 4 never finished (below), so its pages (8,887 of
35,562) have no entry until a nightly records them; nothing gates on these files, and each nightly records its
baselines from scratch. Shards: testharness 28-33 min each, reftests 15-17 min, crashtests 11.5 min (both runs each).
Recorded: testharness 26,675 pages, 26,196 completed, 10,956 passing every subtest, 1,280,632 of 1,444,333 subtests;
reftests 21,630 of 28,303 pass; crashtests 2,035 of 2,105 pass. 15 MB in all (testharness 11 MB, largest
`editing.json` 3.3 MB).

Testharness shard 0 of that run held its runner for 2 h: the stash-polling page exhausted the ports (that run had
the browser-restart workaround, not the drain-wait), the restart could not listen on a port (`EADDRNOTAVAIL`), and
with no browser every worker waited; the watchdog fired once and never again. Since then a failed listen retries for
2 minutes, the watchdog acts again while a worker stays stuck, a run with no page finished for 10 min ends itself
with the rest recorded as `runner stalled` (`WPT_STALL_MS`), and shard jobs time out after 60 min.

A pull request that changes the workflow, `wptserve.mjs`, `fetch-wpt.sh` or the testdriver files runs its `smoke`
job instead (a few wptserve and testdriver pages on lp-m1).

## testdriver

`--testdriver` (with `--server wptserve`) runs the pages that load `/resources/testdriver.js`. wptserve serves our
`testdriver-vendor.js` in place of the checkout's empty one (a file alias, as wptrunner serves Chrome's). It sets
`test_driver_internal.in_automation` and sends each call to the runner through a binding on every context
(`testdriver.mjs`), which performs it with Playwright's input APIs over CDP, as wptrunner performs WebDriver commands:

- `click` (and with it `bless`): a mouse click at the element's in-view center, after scrolling it into view.
- `send_keys`: focus, then the keys typed one by one (WebDriver's Private Use Area codes as key names, modifiers held
  until pressed again or the end).
- `action_sequence` (`new test_driver.Actions()`): tick by tick, pointer (mouse; touch and pen as mouse), key, wheel
  and pause sources; element origins become the element's center.
- Elements in frames: the frame's content-box offset in the page is added.

`set_permission` sends `Browser.setPermission` for the document's origin on the page's session (its browser context).
`get_computed_role` and `get_computed_label` read the element's node from Playwright's accessibility engine
(`locator.ariaSnapshot()`: role and name of its first line; nothing in the tree gives `""`), finding the element by a
`data-pyro-testdriver` attribute held for the call only (pyro's CDP has no Accessibility domain). Presses of the same
button at the same spot within 500 ms count up (`clickCount`), as Chrome's WebDriver counts a double click.
`delete_all_cookies` clears the browser context's cookies (`cookies/value/value-ctl.html`: 0 -> 63 of 66).
Everything else (`get_named_cookie`, BiDi, virtual authenticators, payment and wallet modes, ...) rejects as "not
implemented", as testdriver.js does with no vendor. At a421f2fe 3,206 of the 3,462 files that load testdriver call
only `click`, `send_keys`, `Actions` or `bless`; the next by use are all implemented too: `set_permission` (165
calls), `get_computed_role` (114), `get_computed_label` (77), `delete_all_cookies` (41). A sample of 50 testdriver
pages (1 in 80, before `delete_all_cookies`): 48 completed, 12 passed every subtest. With `--dirs all --server wptserve --testdriver`: testharness 35,562 pages,
reftests 28,303, crashtests 2,105 (every crashtest in the manifest). In a first local check, 6 of 10 testdriver pages
in uievents, css/selectors and html/user-activation passed every subtest. Of the other four, three fail on pyro
gaps (no `navigator.userActivation`; `mouseenter.composed` is true; no mouseover when layout moves an element
under a still pointer) and `uievents/mouse/cancel-mousedown-in-subframe.html` times out; `auxclick_event.html` passes
since click counting. Six wai-aria/accname/html-aam pages: 46 of 52 subtests pass.

## Crashtests

A WPT crashtest is a page that once crashed a browser: it passes when the browser loads it, paints it and is still
there. `run.mjs --kind crashtest` runs them and `gate.mjs ... --kind crashtest` gates them as it gates reftests: each
crashtest is reported as one subtest, `crashtest`, so a crashtest that passed in `crashtest-baseline.json` must still
pass in some run of it, an unrecorded pass fails as stale, and `crashtest-flaky.json` (the format of `flaky.json`)
lists what is not checked. Only passes are protected, as for reftests. Adding or widening a `crashtest-flaky.json`
entry, or dropping a baseline pass, needs the `wpt-accept-loss` label; a page that crashes, panics or hangs pyro is
never listed there or recorded as passing: it stays a failing entry until pyro is fixed. The refreshed baseline is
the `wpt-crashtest-baseline` artifact:

```sh
gh run download <run-id> -n wpt-crashtest-baseline -D bench/wpt     # writes bench/wpt/crashtest-baseline.json
```

There is no baseline until a first run's artifact is committed: with none on the branch or its base, the gate only
reports (`bootstrap`), runs every page twice and uploads the baseline both passes agree on.

### What runs

`crashtest.mjs` `discoverCrashtests()` names crashtests as WPT's manifest does (`tools/manifest/sourcefile.py`,
`name_is_crashtest`): a markup file (`.html`, `.htm`, `.xht`, `.xhtml`, `.xml`, `.svg`) whose name ends in `-crash`
before its extension (`a-crash.html`, `a-crash.tentative.html`; not `a-crash-001.html`), or that lies under a
directory named `crashtests`, outside `resources`, `support` and `tools`. It walks the whole checkout
(`CRASHTEST_DIRS` is `.`; `--dirs css/css-multicol,dom` runs part of it), and `wpt-paths.txt` checks out every
crashtest in WPT by those two names, not only those in the directories the other kinds run: at the pinned revision
discovery finds the manifest's 2,105 crashtests exactly. Of the 2,031 that run, 226 lie in the testharness and
reftest directories; the other 1,805, with the 18 support files they load outside the paths already listed (found
from `serve.mjs`'s request log over a run of all of them, and from the URLs their sources name), add 8 MB in 1,925 files to the
checkout (180 MB with its git directory), which is why all of them run rather than the 226.

2,031 run. Left out, counted by reason in the run's `excluded`: 45 whose names need wptserve (`.sub`, `.https`,
`.h2`, `.www`), 27 that load testdriver and 2 that name a Python handler or `pipe=`. A page's results are counted
under its area (`css/css-multicol`, `html/semantics`, `dom`): the largest are css-multicol 164, editing 151,
html/semantics 135, mathml 126 and css-break 116.

### How a crashtest is judged

As wptrunner does (`docs/writing-tests/crashtest.md`, `executors/test-wait.js` with the class `test-wait`), in
`crashtest.mjs` and `run.mjs`:

- The browser and the viewport are the reftests' (800x600, Ahem installed).
- The page is loaded, and the runner waits for the load event, `document.fonts.ready` (after a forced layout) and
  two animation frames. With `class="test-wait"` on the root element it then fires `TestRendered` at it and waits
  for the class to go, two frames after each change of the root's attributes, up to the page's timeout (15 s, 45 s
  with `<meta name=timeout content=long>`): `timeout`, with `test-wait was not removed`, `the page did not load` or
  `the page kept navigating`.
- "The test passes if the load event is reached, and the browser finishes painting, without terminating." pyro lays
  out and paints on demand, so the runner then takes a screenshot and drops it: a page whose layout or paint fails
  is an `error` with the browser's message, not a pass.
- The runner then navigates the page to `about:blank`, so the document is torn down inside its own test and not
  under the next page of the worker.
- **Crash.** A page whose browser died under it runs again alone; if the browser dies again the page is recorded as
  `crash`, with the last lines of the browser's stderr (a Rust abort's message, `has overflowed its stack`). The
  pages that ran beside it get their own results.
- **Panic.** pyro catches Rust panics at its JavaScript bindings and on its render thread, so most panics do not end
  the process: the call throws, or the frame is not painted. The runner reads the browser's stderr; when a panic is
  printed while a page runs, the page runs again alone, and if one is printed again it is recorded as `panic` with
  the panic's location and message.
- **Hang.** A page that does not answer for 15 s past its timeout is recorded as `hang` and the browser is replaced.

Only a `complete` page carries the passing subtest. `CRASHTEST_STDERR=FILE` appends the browser's stderr to FILE;
with `RUST_BACKTRACE=1` that includes each panic's backtrace:

```sh
RUST_BACKTRACE=1 CRASHTEST_STDERR=/tmp/stderr.log bun bench/wpt/run.mjs --kind crashtest --wpt "$WPT_DIR" \
  --bin pyro=target/release-fast/pyro --jobs 1 --match 'documentElement-remove' --out /tmp/crash.json
```

### What it found

The first local runs (an M1 Max under load, `--jobs 3`, both passes of a bootstrap run agreeing on all but one
transition page): 1,967 of 2,031 passed. 12 pages failed for a defect in pyro, 11 of them fixed with the kind
(1,978 pass):

| pages | what happened | fix |
|---:|---|---|
| 5 | panic, `root_element()` on a document whose element the page removed (`documentElement.remove()`), in the capture and in the document's size | pyro-render reads `try_root_element()`: such a document is an empty canvas the size of the viewport |
| 2 | panic in `resolve_deferred_tasks`: a root element that is inline content (a `<br>` appended to the document) made the document node an inline root | blitz-dom: only a box is an inline root |
| 2 | no screenshot, `the page's tree has a repeated key`: every `<slot>` of one name (and every slot of a `slotAssignment: "manual"` root) rendered the host's children | pyro-dom's render tree gives them to the slot DOM's "find a slot" names |
| 1 | crash, stack overflow in usvg: a `<pattern>` cycle through `href` (`svg/crashtests/chrome-bug-565674533.html`) | `svg_refs.rs` breaks pattern cycles as it does clip path, mask and filter cycles |
| 1 | panic, `Duration::from_secs_f64` on an animation of `duration: 95903184853313357582278077554606` | timer delays from a page saturate (`delay_from_ms`) |
| 1 | hang, `custom-elements/many-slots-crash.html`: a custom element whose shadow tree holds another of itself recursed without end (Chrome ends it with a stack overflow `RangeError`); pyro runs a callback's reactions in the next batch, so it looped | the batch depth stands for HTML's reaction nesting: past 256 a callback reports a `RangeError` (`custom::run_reactions`); blitz-dom skips `:has()` invalidation without `:has()` rules |

The other 52 timed out without a crash. Round 4 (lane R4CR, 2026-11-18) fixed one more panic (a `::first-letter`
box inside a `::first-line` box had no originating element) and the many-slots hang, and made 30 of the timeouts
complete:
- view transitions: a lifecycle-only `startViewTransition` (20 pages);
- `MathMLElement`, SVG time control (`setCurrentTime` and the rest), `commitStyles` on SVG, `@position-try` in
  `insertRule`, srcdoc reload, `window.find`, and a `KeyframeEffect` target from another
  document.

Feature lane F3a (2026-11-18) completed 4 more: customized built-in elements (the two `abort-insertion` pages, which
also hung on a cycle the parser made), `document.designMode` (`designMode-caret-change`) and `selectstart` from
`selectAll` and `find()` (`selectall-and-find-svg-text-on-selectstart`), and `<object>` load and error events
(`container-queries/crashtests/chrome-bug-439886903-crash`); then focus and blur at Windows between frames
(`designMode-document-will-be-blurred`) and Trusted Types with `javascript:` navigation
(`navigate-to-javascript-url-007-crash`).

2,029 of 2,031 complete locally (with the font and Typed OM lanes; lane F3b: navigated-away iframe documents, IntersectionObserver across documents, `SharedWorker`, `<use>` instances; lane F2: scroll-driven animations). These 2 are **known incomplete**: each needs a feature pyro does not have. They stay
failing entries of the baseline, not flaky entries, and none of them crashes, panics or hangs:

| page | needs |
|---|---|
| `streams/readable-byte-streams/crashtests/from-cross-realm.html` | none in pyro: Chrome times out on it too (wpt.fyi); the popup's synchronous about:blank document it needs exists since lane F3b |
| `editing/crashtests/inserthorizontalrule-in-textarea-in-editor-and-undo-on-error-events.html` | none in pyro: it waits for an `error` event at an `<object>` without `data`, which Chromium fires and HTML does not (no data shows the fallback content; `<object>` load and error events exist since lane F3a) |

### CI

One job, `crashtest`, on the self-hosted `lp-m1` (as every macOS job): it restores the browser and the WPT checkout
the other jobs use, runs `gate.mjs run --kind crashtest` at one page per CPU, checks the result against
`crashtest-baseline.json` in the same job (the base branch's files and the `wpt-accept-loss` label as the other
gates read them) and uploads `wpt-crashtest-shard-0` and, when it changed, `wpt-crashtest-baseline`. It is one job
and not a run job plus a Linux gate job because the check takes a second and the job already has what it needs;
it is not sharded because a pull request's run is about 2,000 pages of some 90 ms each. Manual runs take `suites`
(`crashtest` runs this job alone) and `crashtest-args` (for example `--dirs css/css-multicol`).
