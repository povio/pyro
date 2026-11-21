# Slotted elements and CSS sibling counts

At `3ba6dd3386c87566ab49403c2422d33e3e3dbe8c`, sibling-index() and sibling-count() for a slotted element use its original light DOM position. The snapshot records the total alongside the existing index, including siblings assigned elsewhere and children that are never rendered. Snapshots with only the older index retain their existing sibling lookup. Dynamic slot reassignment, insertion of an unassigned element and removal are covered in the native browser guard.

Independent QA compared the exact candidate to `0ded6d51761ad2d0be512b5f4e5fa4d2a64d914e` on 25 ordinary pinned WPT pages. **One gain, zero losses:** 136→137 of 140 named assertions, 21→22 fully passing pages. The only change was the assertion that host children have sibling-index() and sibling-count() based on DOM tree order. All names matched and no page changed harness status, restarted its browser or hit the watchdog. The omitted cases were not replayed. [Named differential](differential.json)

The base executable SHA-256 is `051ccc0687077d33a9198c49cfd2bf68e6df3b7190d8fd315ccf3b3067f032ab`; the candidate SHA-256 is `a9c8f9394c21af00758256d728bd6b790958d925dcc36fabc7cfcce71cc2b307`. Both were built from detached clean sources on macOS arm64 using WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` and JSC cache key `0e0dd7038a7a-6f4649e395e0`. One-job warm builds took 134 seconds for base and 91 seconds for candidate. The browser-native `assigned_nodes_count_all_light_tree_siblings_across_slot_changes` passed. [Frozen selection](tree-urls.json) · [Base output](parent.json.gz) · [Candidate output](candidate.json.gz)

## Performance

One browser worker evaluated six parents of 24 elements each, moving a child and reading computed sibling index/count for each parent three times. There were 20 timed samples per binary and fixture after warmups, in ABBA/BAAB order. Times below are per evaluation. Wall figures are medians; CPU figures are process-tree means with OS clock granularity.

| Fixture | Base wall (ms) | Candidate wall (ms) | Base CPU (ms) | Candidate CPU (ms) |
| --- | ---: | ---: | ---: | ---: |
| Ordinary DOM | 3.566 | 3.839 | 3.5 | 3.0 |
| One slot | 8.825 | 8.825 | 7.5 | 8.0 |
| Two slots | 8.903 | 9.043 | 8.5 | 8.5 |

Host busy time across the 3.522-second window averaged 63.77%, with 60.70% on base arms and 66.93% on candidate arms. CI and two census shards overlapped the measurement; independent QA did not. The ordinary DOM wall median increased 0.273 ms while its measured CPU declined; this short concurrent run cannot attribute that difference to the source change. [Timing samples](tree-cost.jsonl) · [Host overlap](host.json)

## Excluded experiment

A separate commit, `4c55e49fb377c8f3a098e7f1983883be259493ea`, recomputed same-target font-relative CSSOM declarations against a running transition. It gained the focused line-height transition WPT but returned a 300px CSSOM width for a flex item whose actual used width stayed 100px. In an unconstrained block, CSSOM reported 300px while geometry stayed at 400px. This branch contains only the tree-counting change and evidence. A general font-transition fix needs coordinated sampled style and layout while retaining after-change style for transition creation and reversal. [Used-value readbacks](font-transition-readback.jsonl)
