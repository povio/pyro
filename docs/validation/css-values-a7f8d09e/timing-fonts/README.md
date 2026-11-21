# End-of-round CSS Values topic

The final source is `21fd48c15418cfc292c11a378e18b6ea50b158b0`. Its parent comparison uses the previously measured `3e799f754` executable. The compressed reports contain all results from 236 pinned WPT pages. All three builds observed the same 4,890 named assertions.

| Build | Named passes | Fully passing pages |
| --- | ---: | ---: |
| Original round base, `a7f8d09e2` | 4,254 | 146 |
| Previous CSS Values topic, `3e799f754` | 4,592 | 171 |
| Final CSS Values source, `21fd48c15` | 4,626 | 179 |

This batch gained 34 passes, including 33 from the original Chrome-only inventory, with zero losses. The 12 typed attribute cases on `attr-all-types.html` now resolve timing declarations against the actual element. Additional gains cover context-dependent time expressions, registered palette weights, font-relative lengths after a web font arrives, and select widths expressed in line-height units.

From the original base the focused run gained 373 names and lost one, a net increase of 372. The topic has recovered 362 of the original 1,221 Chrome-only names. Studio's previous full run confirmed the first 299. The subsequent 63 recoveries are verified in focused differentials and await combined-corpus confirmation.

The outstanding pinned loss is subtest 4 on `if-cycle.html`. Chrome 158 and upstream WPT agree on the pinned result. The current editor’s draft makes the cyclic style query false and the engine selects its else branch. The original, parent and final reports keep the actual outcomes. [Pinned test, Chrome readbacks, and specification disposition](../if-cycle/README.md)

Element-dependent animation and transition declarations use the read-only Stylo cascade without the renderer’s timing suppression rule. Unrelated timing reads retain their existing path. A web font arrival refreshes root metrics and invalidates cached sampled-animation bases when styles have consulted font metrics. The recascade covers the whole tree in such documents, including when the arriving face is unrelated. Palette percentages resolve in the element context. Native select line-height adjustment follows computation of other properties using lh.

Parent and candidate WPT execution took 9.027 and 6.423 seconds respectively, excluding startup, with one browser worker and zero restarts or watchdog kills. Builds reused the isolated target, pinned JSC and two Cargo jobs. Three modified vendor patches passed round-trip verification. The final code and evidence are ready for the parent’s combined integration checkpoint.

Final native controls passed 17 browser CSS declaration tests, 28 renderer font-related tests and five timing unit tests. The final warm executable build took 10 seconds. Six executable build attempts took 318 seconds in total, including one compilation failure corrected before final validation. Approximately 15 minutes elapsed from the upstream cycle investigation through this batch and evidence handoff.
