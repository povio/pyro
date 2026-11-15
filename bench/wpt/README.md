# WPT coverage gate

`.github/workflows/wpt.yml` runs our WPT subset (the directories in `lib.mjs` `DEFAULT_DIRS`, ~2,800 pages) on every
pull request and fails when coverage goes down. `baseline.json` records, per page, which subtests pass, whether the
page completed, and how long it took. The same gate runs over WPT's reftests in the paint directories (9,341 pages,
`reftest-baseline.json`): see [Reftests](#reftests), which also compares pyro's results with Chrome's.

## How a run works

1. `prepare` (Linux, seconds): cache keys. The browser is keyed by every input of its build (the git object ids of
   `crates/`, `vendor/`, `Cargo.*`, `.cargo/`, `scripts/macos/` and `wpt.yml`, the stable toolchain
   and the patched JavaScriptCore), so a pull request that changes none of them skips the next two jobs.
2. `jsc`: the patched JavaScriptCore (`jsc.yml`), only when neither it nor the browser is cached. The gate tests the
   fork build because the system framework lacks window named properties and import maps: ~3,200 more subtests fail
   with it, for that reason alone.
3. `build`: `cargo build --profile release-fast` (dependencies cached by `Swatinem/rust-cache`, saved from `main`
   only), the patched JavaScriptCore linked in, saved to the cache.
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

## Before making it a required check

The gate's false failures come from pages whose results depend on load. `flaky.json` starts with the six such pages
found in four full local runs and isolated reruns, plus two from the first CI runs (three runs without a baseline,
each running every page twice). Three more pages the CI runs flagged were not flakes of the page and are not listed:
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

A page that spins the browser's main thread is recorded as `hang` 15 s after its timeout, and the browser is
replaced (`css/selectors/invalidation/has-complexity.html` in every full run; pull requests skip it). The pages
that were running in that browser run again: their waits fail with the connection, which used to be recorded as a
timeout with no result, for a page that was about to complete too.

## Timings

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
directories are `REFTEST_DIRS`: the paint directories and all of CSS2 (it costs about 50 s of the run). At the pinned
revision they hold 9,341 reftests. wpt.fyi lists 9,352 in them: the same 9,341, and the 11 left out below for wptserve
or testdriver.

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
| **all** | **9,341** | 3,750 | 2,450 | 121 | 70 | 43 | 10 | 1 |

Left out, counted by reason in the run's `excluded`: reference files (named or linked as one, no links of their own),
markup with no reference (CSS2's visual tests, support pages), crash tests, manual tests (by name, or by CSS flags
such as `interact`), print reftests, pages whose page or references need what `serve.mjs` lacks (`.sub` substitutions,
`pipe=`, Python handlers), and testdriver. Testharness pages in these directories (parsing, computed values) are not
reftests and are not run here. A reference that has references of its own (155 tests) is compared one level deep, as
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
files; 171 MB). Manual runs take `suites` (`reftest` leaves the testharness shards and the client job out),
`reftest-runner` (`macos-15`: two shards on GitHub's hosted runners, billed as macOS minutes) and `reftest-args` (for
example `--dirs css/CSS2 --no-rerun` to time a directory).

Measured on the pull request that added the job (all 9,341 reftests; a run without a baseline runs every page twice):

| runner | shards x jobs | one pass | job, with setup | two passes agreed |
|---|---|---:|---:|---|
| `macos-15` (GitHub: M2 Pro, 5 vCPUs, 14 GB, macOS 15.8) | 2 x 5 | 53 s, 65 s | 2:04, 2:22 | every page |
| `macos-15` | 3 x 5 | 34-37 s (70 s with the crash) | 1:28-2:35 | every page |
| `lp-m1` (M1, 8 CPUs, 8 GB, macOS 26.7) | 1 x 8 | 72 s (9,300 pages), 115-131 s with the timeouts' tail | 4:55 | all but 5 pages, which a runner bug (since fixed) recorded as crashes |
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

