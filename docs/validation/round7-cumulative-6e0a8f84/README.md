# Cumulative CSS validation: 6e0a8f84

Against its exact first parent `8e20f43ba89e9e9dbb26bdbf3b3bed03aa810357`, the combined tree gained **23 strict main pages and 103 distinct named passes**, losing neither. Every gain is in the StylePropertyMap focus set. The supplement was unchanged on every metric. These are results for two positive allowlists, not the entire CSS suite.

| Cohort / reference | Strict pages +/− | Named passes +/− | Observed names +/− | Named status tuples +/− | Page states +/− |
|---|---:|---:|---:|---:|---:|
| Main / exact parent | 23/0 | 103/0 | 1/0 | 104/103 | 1/1 |
| Main / frozen | 82/0 | 819/21 | 2/0 | 841/839 | 3/3 |
| Main / Chrome | 300/1334 | 5968/5635 | 260/498 | 11197/11435 | 150/150 |
| Supplement / exact parent | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / Chrome | 0/8 | 0/42 | 0/0 | 42/42 | 0/0 |

Directions mean candidate-only/reference-only. Main: 5,916 strict pages, 183,296 distinct passing names and 194,674 observed names. Supplement: 27 strict pages, 577 passing names and 874 observed names. Strict requires complete, harness OK, at least one real assertion and all real assertions passing. The full outcome maps retain occurrence order even though the table counts distinct (URL, name) keys.

- All 3,142 keyed occurrences across the 43 focus pages match `beb4eaa25dd6d75c9393e5941ec18250969fdc12` on the respective previous, candidate and Chrome arms. All 103 gains pass Chrome. Two subsequent candidate runs of these 43 normally completing pages matched the primary outcomes exactly, including ordered named statuses. No baseline rerun.
- The only other change was `css/css-scroll-anchoring/abspos-contributes-to-static-parent-bounds.html`: the previous timeout omitted a failing assertion; this candidate completed and recorded the failure. This is coverage, not a new pass or a lost pass. Its prior controls failed on both builds; the page was not selected for fresh controls.
- The three previously reproduced HDR failures remain failures, not new losses: all three passed frozen; two pass Chrome and one fails Chrome. `inherited.json.gz` retains their exact keys and control outcomes. Raw primary data were not replaced by controls.

## Provenance and load

Exact source `6e0a8f847fba9a4d55816f93ffbe5155f9f5ed17`, tree `38e85b17a6e71848b709844c3e44e0faca6af057`. Clean checkout for the build; isolated APFS copy-on-write clone of the finished comparator target, rebuilt with `pyro-build --features pyro/render`, two Cargo jobs. The read-only candidate executable SHA-256 is `cdb297969010a92f4a68040bd7a81a9378ef4b767a5e81c23fbe5d7b1dd488e5`. Build wall 226.34 s, user 438.36 s, system 7.07 s, max RSS 2,745,450,496 bytes. The comparator outcome maps matched the published `eeca23fd6bbe965ab560ec50dac9a852d6c6ebaf` blobs byte-for-byte. Frozen and Chrome reference maps, allowlists and 24 partitions matched `7c4f42242ae76c2d5555f420c268b44cfa7c9266`.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` (tracked checkout clean), patched JSC key `0e0dd7038a7a-6f4649e395e0` (revision and patches matched), Chrome stable `158.0.8127.37` (installed framework digest verified; reused outcomes, no fresh Chrome run). Exact hashes are in `identities.log` and `provenance.log`.

Primary measurement on November 21, 2026: **20:43:08–20:48:29 UTC (321 s)**, after the comparator's primary ended at 20:41:06 UTC. Frozen runner tree `7efc9828957ae18dbd8c8c7df59cba3c59524633` with one local safety adjustment (`runner.patch`): record a lost browser connection instead of re-queuing the affected test. Four staggered runners maximum, two workers each, then a separate worker-globals supplement. Every planned and observed URL matched the archived 8,082 main and 41 supplement; no overlap with the 39 withheld. No security or native-fault replay. One browser restart, no watchdog kills. Whole-host busy averaged 16.49% (6.02–37.31%, 21 samples); peak single runner-command RSS 262,619,136 bytes. Recorded CPU seconds: browser 136.93, runner 70.313, server 0. These are not isolated performance comparisons.

## Receipt verification

Run `shasum -a 256 -c SHA256SUMS` in this directory. `all-*.json.gz` stores both directions for all five metrics; `attribution.json.gz`, `losses-*.json.gz` and `focus-occurrences.json.gz` separate the intended changes and other differences. `raw-*.json.gz` are full untouched primary reports. `control-*.json.gz` are separately identified repeats and their differences.

To regenerate comparisons, decompress the primary raw reports into an output directory and place the 24 decompressed `urls-*.json.gz` manifests from `7c4f4224:docs/validation/studio-integrated-4126821de` beside them. Populate a frozen directory with that commit's `refs/` contents and `verify.py`; populate `OUTPUT/previous` with `outcomes-{main,supplement}.json.gz`, `prior-hdr-3.json.gz`, `all-main-vs-previous.json.gz` and `controls.json` from `eeca23fd:docs/validation/round7-cumulative-8e20f43b`. Run `python3 -B compare.py OUTPUT OUTPUT/previous FROZEN FOCUS`, using `docs/validation/round7-stylepropertymap` at `beb4eaa2` for `FOCUS`. To verify controls, decompress the two raw control reports into `OUTPUT`, copy `control-urls.json` there, and run `python3 -B controls.py compare OUTPUT FROZEN FOCUS`.
