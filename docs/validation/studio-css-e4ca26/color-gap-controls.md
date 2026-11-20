# Color, gap controls, and coverage expansion

Same Studio, stable Chrome, WPT revision and JSC key as README. At `0702fd16`, 24,028 named passes were stable-only. The largest related families were absolute positioning in grid/alignment/flex (3,360/997/713), calc-size (2,572), transform animation (819), gap decoration animation/parsing (769/569), system-font animation (588 on one page), and text-spacing-trim combinations (508). Directory and page clusters are triage leads, not proof that one change will fix each cluster.

- `baf8d6d5` carries both individually analogous missing color channels and the remaining analogous channel set during conversion for relative colors and mixing. On all 69 CSS Color URLs: 52 strict pages before and after, 11,911 → 12,154 named (+243/-0); stable 56 / 11,942. `color-computed-none.html` alone went from 336 to 577 named passes; stable passed 420. Many new passes are not stable-only gains. See `analog-focused.json.gz`.
- `0aa94130` implements discrete break, visibility-items, and overlap values in Stylo and mirrored axis shorthands. No painting claims. On the same 106 gap/column/cascade URLs: 58 → 67 strict pages (+9/-0), 3,742 → 3,811 named (+69/-0); stable 104 / 5,128. See `gap-controls-focused.json.gz` and the preceding `row-matched.json.gz`.
- The four earlier losses were rerun under the original six-job concurrency, with the 24 adjacent inventory URLs around each affected page, three paired trials, reversing launch order for pair 2. All four passed 3/3 on the candidate. The SVG assertion passed 3/3 on the predecessor; its three stylesheet assertions passed 2/3. The stylesheet failures are therefore observed on both builds. This does not explain the underlying races. See `concurrent-loss.json.gz` and the earlier isolated repeats.

## Previously uncovered inventory

Keep the historical 8,082-URL series intact. Of its 80 uncovered URLs, **41 now have a separate matched run**; the remaining 39 crash/security targets stay excluded. BOM-aware discovery reads markup without altering the served bytes. Opt-in `--worker-globals` uses real WPT wrappers and their declared globals and query strings; no expectations or WPT sources changed.

| Environment | URLs | LP strict / named | Stable strict / named |
|---|---:|---:|---:|
| UTF-16 HTML | 4 | 4 / 4 | 4 / 4 |
| Dedicated worker | 15 | 9 / 320 | 13 / 489 |
| Shared worker | 11 | 7 / 56 | 9 / 63 |
| Service worker | 11 | 7 / 56 | 9 / 63 |

Chrome returned harness errors with no real subtests on all 11 service-worker URLs under its ordinary disposable-profile launcher. Repeating all 41 in another disposable profile with only the WPT leaf certificate's public-key SHA-256 on Chrome's certificate exception list produced 41 OK harnesses, without changing system trust. `discovery-supplement.json.gz` preserves both initial engines' results; `chrome-wpt-cert-supplement.json.gz` preserves the certificate-specific trial used for the stable counts in this table. Neither trial replaces the original stable results. LP also had 41 OK harnesses. Supplemental totals: LP 27 / 436, stable 35 / 619. The 39 remaining exclusions, lack of matched H2, and limited testdriver support still apply.
