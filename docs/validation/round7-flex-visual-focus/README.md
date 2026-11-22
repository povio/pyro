# Visual flex keyboard traversal

Tested source `f09bb0d66456ab282c1a95ab4fa1f8044aa4d29e` starts from exact integrated parent `f065eb497630b342f6e57e88085c222623a0636e`. After the final flex layout pass, a default-no-op Taffy callback exposes an iterator of the generating layout node, line and item indices. Blitz consumes it only for a flex-visual container. The renderer maps actual layout nodes to DOM owners where unambiguous. Focus traversal uses logical line and item order with wrap and main-axis reversal. The metadata is recorded before fragmentation; line breaking, sizing and placement remain the engine's existing computations.

Independent QA on pinned WPT a421f2fea and JSC 0e0dd7038a7a-6f4649e395e0 measured 4→8/8, +4/−0. The ordering page rose from 1 to 4/4 and fragmentation from 2 to 3/3. Wrapped fragmentation retained 1/1. Signed Chrome 158.0.8127.37 passes all eight. Separately rerun controls retained 12/12 reading-flow focus and 40/40 CSS declarations. All harnesses completed with zero restarts. QA repeated the three-page head run to recover its raw output after a filename collision; the two named comparisons were identical.

Both navigation directions matched live Chrome in all 16 direct traces, up from four on the parent. The fixtures exercised ordinary, reversed, wrapped and wrap-reverse cases, anonymous text items, fragmented items and live style changes. The seven geometry snapshots were identical between parent and candidate. All seven differ from Chrome in exact rectangles; this change does not establish geometry parity.

The first candidate failed to compile because the computed wrap value is a set of flags. The correction checks the WRAP_REVERSE flag and supports coexistence with balance. Both native focus guards passed in 0.08 seconds after a 50.02-second native build. The one-job executable build took 84 seconds. Both vendor patches were verified against the direct parent.

## Same-host cost

Eight serial fresh processes ran in ABBA/BAAB order with one excluded warm-up and four timed samples per fixture. Traversal fixtures sent 32 forward then 32 reverse Tab steps and validated return to the start. Layout fixtures changed a flex container's width and synchronously read its height 48 times. The baseline does not implement the new visual order, so its traversal performs different work. Ordinary layout is matched. Wall medians include CDP overhead and process-tree CPU means are coarse.

| Fixture | Parent median ms | Candidate median ms | Parent / candidate CPU ms |
| --- | ---: | ---: | ---: |
| Ordinary Tab traversal | 20.37 | 20.44 | 6.88 / 6.25 |
| Reverse flex visual traversal | 19.37 | 19.90 | 5.00 / 6.25 |
| Wrapped reverse flex visual traversal | 20.12 | 20.16 | 5.62 / 5.62 |
| Ordinary flex layout resizing | 5.99 | 5.77 | 5.62 / 5.62 |
| Opt-in flex-visual layout resizing | 5.96 | 5.80 | 5.62 / 5.00 |

Host occupancy averaged 38.78% across 6.542 seconds; baseline arms averaged 37.46% and candidate arms 40.16%. Differences are descriptive for these fixture and load conditions. [Per-arm host receipt](host.json)
