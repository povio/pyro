# Independent Studio round 7 comparisons

Evidence base: `7f399bf8`. Batch 3 source: `8f6a67d823d9327fc00f8901662d160cf6bd8ed0`. Batch 4 source: `c56b9f5e3fda07c29a9d6ca4c13a104a0e4265dd`. The local candidate binaries match their archived SHA-256 receipts. Frozen and Chrome outcome archives match the frozen Studio cache byte for byte. See `frozen/provenance.md` for Chrome and build provenance.

The 41-URL supplements for both heads are complete and identical to batch 1 in every keyed outcome. Each has 27 strict pages, 577 distinct named passes, 874 distinct names and 41 complete/OK harnesses. Chrome alone passes 8 more pages and 42 more named assertions; neither candidate has a Chrome-only comparison gain, and no names are missing. No supplement repeats were indicated.

`python3 -B compare_followup.py RAW_DIRECTORY BATCH COHORT` consumes only an existing final report. The inherited check verifies the exact allowlist, disjointness from withheld, per-URL coverage, settings, and pin. Compressed directional sets are relative to the candidate, not the reference.

## Batch 3 main

All 8,082 URLs accounted for. The candidate passed 5,846 strict pages and 182,575 distinct named assertions, with 194,672 distinct names observed. Versus batch 1: +9/−0 strict pages, +33/−0 passes. Versus frozen: +12/−0 strict pages, +77/−0 passes. Both references have the same observed names, runner timeouts and hang as the candidate. One harness timeout changed to OK. There were no candidate losses requiring matched repeats.

Batch 3 only / Chrome only: 291 / 1,395 strict pages, 5,930 / 6,318 named passes, and 260 / 500 observed names. These are directional differences, not regressions against the prior build.

## Batch 4 main

All 8,082 URLs accounted for: 5,852 strict pages, 182,594 named passes and 194,672 observed names. Versus batch 3: +6/−0 pages, +19/−0 named passes. Versus batch 1: +15/−0 pages, +52/−0 passes. Versus frozen: +18/−0 pages, +96/−0 passes. No names were lost or added against any of these references. `css/css-transitions/non-rendered-element-001.html` improved from harness timeout to OK against batch 3; the other page statuses were unchanged. No candidate losses required repeats. Neither a duplicate full run nor any withheld URL was executed for this validation.

Batch 4 only / Chrome only: 291 / 1,389 strict pages, 5,930 / 6,299 named passes and 260 / 500 observed names. Both candidates retain the same 12 runner timeouts and one hang as frozen and batch 1.
