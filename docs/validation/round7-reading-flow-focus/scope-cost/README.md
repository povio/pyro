# Reading-flow focus scope construction cost

Candidate `0cf1844a12906f9db732a192dd9ffce82f7dc610` starts from `f065eb497`. The measured comparator was the archived `0952a65d2` executable. Intervening integration changes affect editing, range and IndexedDB. The focus traversal source at the two baselines is identical.

When the ordered roots of a reading container are its actual DOM children, they are already focus scope owners. The candidate returns those element roots directly, avoiding another traversal and per-child computed-style lookup. Shadow and assigned roots with another DOM parent use the original walk. All state still expires before focus changes can run script.

Independent QA retained 12/12 real Tab assertions and 40/40 declaration assertions. All 22 direct forward/backward traces match the prior executable; 18 match live signed Chrome. The named-slot discrepancy and two display:contents traces also occur on the comparator. For display:contents, both Pyro binaries visited a,b,c,d,end forward; Chrome visited c,b,a,d,end. The nonfocusable-wrapper controls, nested containers and live style mutation matched in both directions. The two native guards passed in 0.09 seconds following a 50.77-second test build. The one-job executable build took 74 seconds.

## Paired real-key costs

Eight fresh serial processes ran in ABBA/BAAB order using the original reading-nav-cost.cjs. Each measured sample dispatched 32 Tab then 32 Shift+Tab steps and checked focus returned to the start. The values are median milliseconds per 64 real steps, including CDP round trips, with mean browser process-tree CPU milliseconds per sample in the last column.

| Container | Before | After | CPU before / after |
| --- | ---: | ---: | ---: |
| Normal | 20.97 | 21.43 | 7.50 / 6.88 |
| Source order | 24.15 | 21.01 | 6.88 / 6.88 |
| Flex flow | 22.35 | 20.92 | 6.25 / 5.62 |
| Grid columns | 22.12 | 21.35 | 5.62 / 5.00 |

Source-order wall time fell by 3.14 ms per 64 steps (13.0%); the mean process-tree CPU reading was unchanged. The normal control measured 0.46 ms slower in this series. These are descriptive measurements for this fixture.
