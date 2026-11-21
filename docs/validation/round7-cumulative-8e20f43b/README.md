# Combined CSS testharness census: 8e20f43b

Source `8e20f43ba89e9e9dbb26bdbf3b3bed03aa810357`, tree `33d87f63e4892577a46650ced6aff785572292f9`. This evidence topic adds no runtime changes. No earlier exact-tree receipt or live run was found. Measurements are independent of Studio batch 18. The primary started after the e090 primary and controls finished; no baseline census or Chrome rerun was performed.

## Outcomes

Candidate main: 5,893 strict pages, 183,193 distinct passing `(URL, name)` keys and 194,673 observed keys. Candidate supplement: 27 strict, 577 passing, 874 observed. Pairs below are **candidate-only / reference-only**, not net changes. Status includes the assertion result; page state includes both runner and harness status.

| Cohort and reference | Strict | Named pass | Observed | Named status | Page state |
|---|---:|---:|---:|---:|---:|
| Main, exact e090 | 0/0 | 24/0 | 0/1 | 24/25 | 1/1 |
| Main, frozen | 59/0 | 716/21 | 2/1 | 738/737 | 4/4 |
| Main, Chrome | 299/1356 | 5968/5738 | 260/499 | 11299/11538 | 151/151 |
| Supplement, exact e090 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement, frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement, Chrome | 0/8 | 0/42 | 0/0 | 42/42 | 0/0 |

All 24 new passes are exactly the flex scroll-width names checked at `cb87ba371320af134406ebd945bb92d080430204`: frozen and previous Fail, candidate and Chrome Pass. Previous/candidate/candidate/previous controls returned Fail/Pass/Pass/Fail for all 24. No strict page or passing name was lost versus e090. One previously **failing** scroll-anchoring assertion went unobserved when its page timed out. All four matched controls completed normally with that assertion failing on both builds. The raw missing observation and page-state change remain in the census. The three previously reproduced HDR losses still fail on both e090 and this candidate; Chrome fails one and passes two. They are not new regressions.

## Measurement

- Frozen WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, no tracked modifications. Runner Git tree `7efc9828957ae18dbd8c8c7df59cba3c59524633`. Patched JSC archive SHA-256 `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`. Verified installed Chrome 158.0.8127.37 and framework hash; reused its immutable outcomes and frozen LP outcomes byte-for-byte from `7c4f42242ae76c2d5555f420c268b44cfa7c9266`.
- Exact e090 source `e090242d6c73f7f9e9bded0b045f3489fb35f0bd`, tree `0d438cfa94075a3b6e242bb9cdb2e7f1afdee63a`. Read both outcome maps from immutable evidence `a8cf461e607fb596ac76babc391ed598cab7a675`; matched control binary SHA-256 `875e7a882a87953a4f3c2ac590a095ff2d8a264eac8c24a38fb9bf1809d848b9`.
- Fresh isolated target, three Cargo jobs, `pyro-build --features pyro/render`; build 271.54 s wall, 782.02 s user, 30.74 s system, maximum RSS 2,760,900,608 bytes. Candidate SHA-256 `bccd51b5f6fc193639d21bb1a51c6c44c0f2c6370ed6e2617e55028cb18c0141`.
- Exactly 8,082 archived main URLs in 24 shards and 41 supplement URLs with worker globals. The 39 withheld URLs were excluded before scheduling. All per-shard actual plans and result keys were verified. No native-fault or security replay. Frozen `--kind testharness --dirs css --server wptserve --testdriver --jobs 2`, four runners maximum, private free-picked ports. Controls used only the two positive-list URLs in `repeat-urls.json`.
- Primary wall 426 s (20:34:00–20:41:06 UTC, November 21, 2026), including startup recovery. An initial command assembly failure dispatched no tests; shards 00 and 03 then had a browser-start failure before executing any page and were run sequentially after the other 22. Both startup logs retained. Successful main shard wall times 41.256–54.185 s, supplement 0.601 s. One browser restart and zero watchdog kills in successful primary reports.
- Runner-reported total CPU seconds: 140.06 browser, 74.47 runner, 0.00 server. Peak successful runner-command RSS 261,980,160 bytes (not a concurrent aggregate). On the 18-core, 36 GiB host, 41 whole-host samples during the primary averaged 23.47% busy (6.00–98.47%); memory-pressure free percentage 80% before launch and 81% after. See per-run logs and `resources.log` for measurements, not isolated host attribution.

## Verification

Check `shasum -a 256 -c SHA256SUMS` here. Unpack `raw-*.json.gz` into a scratch output directory, and the 24 `urls-*.json.gz` manifests from `7c4f4224:docs/validation/studio-integrated-4126821de` into that same directory. Use `refs/*` and `verify.py` from that revision as `frozen/`; use both `outcomes-*.json.gz` and `repeats-main.json.gz` from `a8cf461e:docs/validation/studio-integrated-e090242d` as `previous/`. Run `python3 -B compare.py OUTPUT previous frozen PATH/TO/docs/validation/round7-flex-width` using the focused receipt at `cb87ba37`. `all-*.json.gz` retain every exact directional set; `outcomes-*` keep ordered duplicate statuses. `focus-24`, `prior-hdr-3`, `losses-*`, and `controls.json` separate attribution, inherited losses, coverage, and repeats. Full primary and repeat reports are preserved independently.
