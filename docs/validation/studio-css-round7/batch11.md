# Batch 11: loaded imports and hidden-subtree animations

Compared with batch 10, **5 strict pages and 7 named assertions gained, none lost**. Only six main URL outcomes changed: seven `Fail` → `Pass` transitions with identical assertion inventories and ordering. All seven now agree with pinned Chrome. All 41 supplement outcomes are identical to batch 10. No suite, repeat, or withheld URL was executed for this review.

Counts are directional memberships (**batch 11 only / reference only**), not net totals. Status columns count URLs; named and observed columns count distinct `(URL, assertion name)` pairs.

| Cohort / reference | Strict pages | Named passes | Observed | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 10 | 5/0 | 7/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Main / frozen | 54/0 | 259/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| Main / Chrome | 292/1354 | 5930/6136 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| Supplement / batch 10 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Main totals: 5,888 strict pages, 182,757 named passes, 194,673 observed; 7,873 complete OK, 95 complete error, 102 harness timeouts, 11 runner timeouts, one hang. Supplement: 27 strict pages, 577 passes, 874 observed, all 41 harnesses OK. Exact directional URL and assertion lists, including all status memberships, are in `batch11/difference-*.json.gz`.

## Attribution and loss check

- The seven-URL focused import evidence (`947f3c92`) accounts for two strict pages and two names: CSSImportRule media-list identity and unlinking an imported sheet after rule deletion. The integrated `86d9cf41` and focused `fcca8ecd` have identical stable patch IDs.
- The ten-URL final focused animation evidence (`12e5d59b`) accounts for two strict pages and three names: self- and ancestor-hide cancellation, and prevention of startup in a hidden subtree. The integrated `1ee620d4` and focused `603fffa1` have identical stable patch IDs. These counts use the committed final focused receipts, not a similarly named cache experiment.
- Two further animation assertions passed only in the sense that the **gain was measured in the combined run**, not isolated by the focused comparison: `event-dispatch.tentative.html` (Active → Idle, display: none; page still not strict) and `stability/animation-event-destroy-renderer.html?animationstart` (cancel during event handling; page now strict). They are consistent with the animation change, not independently proven single-commit effects.
- Neither focused comparison lost a page, pass or observed name. The batch-10 comparison shows no loss or status movement on either cohort. Chrome-only outcomes remain parity gaps, not new regressions. No conclusion covers withheld URLs.

## Reproduction and timing

`python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch11` rebuilds from existing artifacts. It checks source `1ee620d451c093a2ddce8766172970ed0ce8275b`, its two-commit ancestry to batch 10, both executable digests, and the frozen verifier bytes. Candidate SHA-256: `9cc924676e94442da81d71354e1402e4dca3d78fadd9ab31145fec56cccab980`. The frozen verifier reconstructs all four runs from raw reports at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; the three references match their archived keyed outcomes. All sixteen batch-9 plans match their archived bytes and individual report memberships; their disjoint union is exactly 8,082 main URLs, separate from the 41 supplement and 39 withheld URLs. Chrome is the recorded stable reference, not a fresh run.

Build: **40.36 s**. Validation: **137.354 s**, earliest log creation to last report modification, excluding build. Overlapping main shard wall times: **58.218–75.218 s**, median **64.233 s**; supplement **0.463 s**. Four in-window whole-host CPU busy samples, in order: **31.31%, 10.32%, 24.60%, 4.38%** (median **17.46%**, mean **17.6525%**). These are point samples, not continuous or process-specific utilization. See `batch11/timing.json` for timestamps and load averages; `shasum -a 256 -c SHA256SUMS` from `batch11/` checks the archive offline.
