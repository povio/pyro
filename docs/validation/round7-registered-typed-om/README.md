# Registered computed custom properties in CSS Typed OM

The exact parent was `c8ac2292ba898c0122853d2f192b79562914e2d9`, the tested source `6886fc523a2d4bc7c5c3dd0987b427da44010bae`. Independent QA matched all 423 occurrences across seven pinned WPT pages against the archived Studio Chrome 158.0.8127.37 reference. Passes rose from 390 to 421 (+31/−0). Of the 31 targeted signed-Chrome gaps on typedom.html and at-property-typedom.html, 30 closed. The additional gain, iterating a URL image as CSSImageValue, fails in the Studio Chrome run. All seven harnesses completed without a restart.

The computed map uses the type and list boundaries selected by Stylo, including for syntax alternatives. The native guard verifies first-item and full-list reads, relative units, iteration order, and token-stream behavior for specified, universal and unregistered properties. It passed in 0.08 seconds after a 73-second native build. The one-job executable build took 112 seconds.

The main typedom page reached 325/325. One of two at-property tests passes: the second stylesheet registration switching its syntax from length to integer still returns px instead of number. Separately, the preexisting custom-property sorting test on computed/iterable.tentative.html still fails. All other controls were unchanged.

## Representative cost

Eight fresh browser processes ran serially in ABBA/BAAB order. Each performed one warm-up and five measured samples per fixture, with 2,000 reads per sample. Baseline and candidate performed the same operations. The baseline exposes one token stream for the registered list; the candidate returns two numeric values. Wall medians below include the CDP evaluation and conversion of the result to a string. CPU figures are mean browser-process-tree CPU milliseconds per sample.

| Fixture | Base median ms | Candidate median ms | Base / candidate CPU ms |
| --- | ---: | ---: | ---: |
| Ordinary width read | 3.95 | 3.91 | 8.5 / 8.0 |
| Unregistered property read | 3.28 | 3.61 | 5.0 / 6.0 |
| Registered list read | 3.81 | 4.37 | 5.5 / 5.5 |

The raw rows include per-process UTC bounds and per-sample durations. These short-run differences describe this fixture and host.

From 21:37:06.869 to 21:37:09.862 UTC, whole-host occupancy averaged 23.87%, with 22.44% on the baseline arms and 25.42% on candidate arms. Registered-property QA had zero overlap. A separate paragraph QA run overlapped the probe according to its receipt timestamps. CI was idle and census was absent. [Per-arm host receipt](host.json)
