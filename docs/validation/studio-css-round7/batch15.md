# Studio batch 15: transition reversals and a reproduced calc-size regression

`ce79ed2c` gains three strict pages but loses one named pass net versus its direct parent `8627d988`: three Fail → Pass and four Pass → Fail. Two matched four-URL repeats on each binary reproduce every changed assertion and every page status from the full runs. **The four losses are repeatable, not just a full-run timing fluctuation.** No additional tests were launched by the reviewer.

| Cohort / reference | Strict | Passes | Observed | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 14 | 3/0 | 3/4 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Main / frozen | 66/0 | 301/4 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| Main / pinned Chrome | 292/1342 | 5931/6099 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| Supplement / batch 14 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / pinned Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Counts are candidate-only/reference-only. The 8,082 main URLs have 5,900 strict pages, 182,795 passing names and 194,673 observed names. All 41 supplement URLs completed OK: 27 strict, 577 passing and 874 observed. The frozen verifier rebuilt both cohorts from the existing raw batch 14, batch 15, frozen and Chrome reports (Chrome 158.0.8127.37; WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`). All 17 ordered plans match batch 14 exactly; each disjoint main shard matches the archived positive shard membership. No withheld URL was run. Exact differences and compact outcomes are in `batch15/`.

The two focused gains at `3961664a` are the currentTime seek and replaced-effect reversal. All eight focused pages' named statuses match their respective full parent/candidate/Chrome outcomes. The third gain, `retargetted-transition-with-box-sizing.html`, is outside the focused set. All three pass in pinned Chrome. Four width interpolation assertions at progress 0.75 on `css/css-values/calc-size/animation/calc-size-interpolation-expansion.html` pass in batch 14, frozen and Chrome but fail in batch 15. The page was already non-strict, so these losses are invisible in strict-page totals. `batch15/status-changes-main.json` records all seven exact names and transitions. No observed-name or harness/runner status changes versus batch 14; no supplement changes. The archived repeats match each full run's complete outcomes on these four pages (zero restarts/watchdog kills).

The stable patch ID `c5d3a4d97f17d48b8490564ee8c9260bd43491b2` matches focused source `eefe04af`. All 17 reports identify the same 0555, 137,207,424-byte candidate; its saved SHA-256 and the independently hashed build output both equal `d920e1e9f0f17fae9afeadde712d9af666134f0b2be13a05aefcadac50a38eda`. Cargo: 52.49 s. Excluding build and repeats, elapsed time was 137.812 s from first log creation to final report modification. Overlapping main shard walls: 58.158–75.626 s, median 64.519 s; supplement 0.527 s. Whole-host CPU busy snapshots: 45.39%, 28.45%, 29.13%, 7.76% (mean 27.68%, median 28.79%); one-minute load fell from 6.68 to 4.95. These are not process CPU measurements.

Reconstruct from this directory with `python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch15`; verify saved receipts with `(cd batch15 && shasum -a 256 -c SHA256SUMS)`. The preceding batch 14 evidence is retained unmodified.
