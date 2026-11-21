# Studio CSS validation: integrated e090242d

This measures `e090242d6c73f7f9e9bded0b045f3489fb35f0bd`, tree `0d438cfa94075a3b6e242bb9cdb2e7f1afdee63a`. It is **not** Studio batch 16 (`55bca240`). The previous measured integration is `4126821deb2a31e52eed6e73006543e502396963`; the round freeze is `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Other work landed between these commits; only the outcomes identified in the two independent focused receipts are mapped to HDR and geometry.

## Results

| Cohort | Strict pages | Named passes | Observed names | Versus previous: pages; passes; names | Versus freeze | Versus Chrome |
|---|---:|---:|---:|---|---|---|
| Main | 5,893 | 183,169 | 194,674 | +10/−0; +154/−3; +1/−0 | +59/−0; +692/−21; +2/−0 | +299/−1,356; +5,968/−5,762; +260/−498 |
| Supplement | 27 | 577 | 874 | 0/0; 0/0; 0/0 | 0/0; 0/0; 0/0 | +0/−8; +0/−42; 0/0 |

All 99 focused HDR gains and all 12 geometry gains were previous-Fail → candidate-Pass and pass in the Chrome reference. Computed, inheritance, and interpolation became strict HDR passes. Parsing lost three pinned names; both matched pairs repeated previous-Pass → candidate-Fail for each. Chrome also fails the singleton `mix(no-limit 1%)` rejection assertion, but passes the literal all-zero and nested all-zero rejection assertions. The 12 geometry names are six flow-root and six grid horizontal widths on `scrollWidthHeight-negative-margin-002.html`; this is not a flex or containment recovery. The remaining 43 gained names and seven newly strict pages are integrated observations, not assigned to either focused patch. The one additional observed name is `dynamic-range-limit` on `all-prop-initial-xml.html`. No per-URL runner/harness state changed against the previous integration. All supplement status sets match both previous and freeze.

Primary elapsed **294 s**, 20:23:46–20:28:40 UTC on November 21, 2026. Main runner wall times 41.258–54.312 s, supplement 0.606 s. One main restart, zero watchdog kills. The 31 whole-host CPU samples averaged 23.49% busy (2.84–67.20%); not a process-specific utilization estimate. The conservative four-runner cap left capacity for other Studio work and CI.

## Method

Built from a clean, detached local clone into an empty private Cargo target using `CARGO_BUILD_JOBS=3 pyro-build --features pyro/render`. The `wptiter` build took 288 s. Binary SHA-256 `875e7a882a87953a4f3c2ac590a095ff2d8a264eac8c24a38fb9bf1809d848b9`; static patched JSC `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`. The frozen runner matches the integrated runner byte-for-byte. The pinned WPT checkout (`a421f2fea5eb7f91e0db399e44b0fb0779ee1603`) had no tracked changes. The pinned Chrome stable 158.0.8127.37 outcomes are reused, **not** new Chrome runs. Their bytes match the independently checked Git receipts at `7c4f4224:docs/validation/studio-integrated-4126821de/refs`; installed framework hash is recorded in `identities.log`.

The primary run reused all 24 archived positive main shard lists from that same evidence commit, with 8,082 distinct URLs in total. The 41 supplemental URLs ran separately with worker globals; none of the 39 withheld URLs was scheduled. The three memberships were checked for disjointness before launch. Each positive manifest was checked against its actual plan and results, including query strings, under `--kind testharness --dirs css --server wptserve --testdriver --jobs 2`. Batch 16 had completed before the primary run; four runners maximum, eight logical page slots. Candidate rows are not replaced with repeat results.

## Evidence and replay

`raw-*.json.gz` are complete per-shard runner reports. `all-*-vs-*.json.gz` preserve both directions for strict pages, passing and observed `(URL, name)` keys, `(URL, name, status)` triples, and `(URL, runner status, harness status)` triples. `outcomes-*.json.gz` also preserve duplicate names and their ordered statuses. A strict pass requires a complete page, OK harness, at least one actual named assertion, and all such assertions passing. `focused-map.json.gz` contains every focused HDR gain/loss and geometry gain, with observed statuses on all four engines/revisions. `repeat-*` reports are separate matched runs in previous/candidate/candidate/previous order. Focused outcomes are checks on attribution, not extra gains added to the totals.

Verify `shasum -a 256 -c SHA256SUMS`. To replay, put the previous bundle from `7c4f4224` in `previous/`, its `refs/*` and `verify.py` in `frozen-evidence/`, and this bundle in sibling `candidate/`. Decompress the candidate `raw-*.json.gz`, `urls-*.json.gz` and `repeat-*-{previous,candidate}-*.json.gz` reports into `candidate/` (keep `.json`). Run `python3 -B previous/analyze.py candidate`, `python3 -B candidate/compare.py candidate previous frozen-evidence <e090242d checkout>/docs/validation`, and `python3 -B candidate/controls.py candidate previous frozen-evidence`.
