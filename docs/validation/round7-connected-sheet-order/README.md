# Connected document stylesheet moves

Production and native guard were tested at `71da71d348bdeff378eba0a4360069fd580a0252` on the published stylesheet-cost checkpoint `b98d812e0`. The before executable was `fe6f3d382`, whose production sources match that checkpoint. Moving a connected document stylesheet or its containing subtree now updates the cascade order using the already loaded sheet object.

Independent QA matched the four observations on signed Chrome 158.0.8127.37. The baseline produced second/second/second/second and both candidate and Chrome produced second/first/second/first (initial, same-parent move, cross-parent move, containing section move). The seven-page WPT ledger retained all 423 named results: 422 pass, one preexisting computed-map sorting failure. All seven harnesses completed without restart. The connected-move native guard and both registration guards passed.

## Same-host performance

The ABBA/BAAB series used four fresh processes per executable. Every process exercised all twelve fixtures with one warm-up and three measured samples. Timings below are median in-page milliseconds for 16 operations with cascade readbacks. Initial sheet counts are shown; append grows the count by 16. The accepted baseline often skipped the work needed to produce the correct result on connected same-parent moves. Counts are incorrect readbacks across the 12 measured samples per executable.

| Workload | Starting sheets | Base ms | Candidate ms | Incorrect reads, base / candidate |
| --- | ---: | ---: | ---: | ---: |
| append | 4 | 0.55 | 0.55 | 0 / 0 |
| reorder | 4 | 0.20 | 0.40 | 144 / 0 |
| reinsert | 4 | 1.90 | 1.95 | 0 / 0 |
| crossParent | 4 | 1.10 | 1.10 | 0 / 0 |
| append | 32 | 0.60 | 0.60 | 0 / 0 |
| reorder | 32 | 0.30 | 0.50 | 192 / 0 |
| reinsert | 32 | 2.40 | 2.30 | 0 / 0 |
| crossParent | 32 | 1.25 | 1.25 | 0 / 0 |
| append | 128 | 1.10 | 1.10 | 0 / 0 |
| reorder | 128 | 0.60 | 1.20 | 192 / 0 |
| reinsert | 128 | 3.80 | 3.85 | 0 / 0 |
| crossParent | 128 | 1.70 | 1.95 | 0 / 0 |

On an isolated cross-parent fixture, moving the first owner to a second container after the previous winner switched the computed value to first. Candidate and signed Chrome returned second/first/second/second; the baseline kept second throughout. Moving the sheet back and inserting it before the other sheet also matched Chrome. The supplemental check took place after the timed window.

The 18.355-second measurement window averaged 15.17% host CPU. Base-arm occupancy averaged 15.66% and candidate-arm occupancy averaged 14.68%. [Per-arm host receipt](host.json)
