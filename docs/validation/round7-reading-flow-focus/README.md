# Reading-flow sequential focus navigation

Independent QA compared source `0952a65d21b35fcd1e3467d58127e247d3812fcf` with the exact integrated parent `1c2a606f9b54957594d02c569c1565f3aefa7eab` on pinned WPT a421f2fea and JSC 0e0dd7038a7a-6f4649e395e0. Four testdriver pages improved from 2/12 to 12/12 (+10/−0). Source-order gained five names, flex-flow four and grid-columns one. Normal retained both passes. The six separately measured declaration pages retained 40/40. Signed Chrome 158 passes every named outcome in both cohorts. All ten harnesses completed without a restart.

Navigation consults current computed styles when Tab or Shift+Tab is pressed. Source order, order-modified flex/grid items and resolved grid row/column starts are supported in the applicable block, flex and grid containers. The read-only grid adapter links the recorded placement to its generating layout node and maps it to a DOM owner when available. It does not infer identity from a child index. Grid placement and sizing algorithms are unchanged. Flex-visual traversal and a complete flattened-tree implementation are outside this batch.

## Direct keyboard checks

Sixteen of 18 candidate traces matched live signed Chrome 158.0.8127.37, up from four on the parent. Ordinary Tab order, positive tabindex across scope boundaries, nested reading containers, explicit and auto grid placement, CSS-order-modified grid items and a live flex style change all matched in both directions. The two named-slot traces differed on both Pyro binaries. The baseline visited a,c,b,d,end forward and d,b,c,a,start backward. The candidate applies reading order, visiting c,b,a,d,end forward and d,a,b,c,start backward. Chrome skips the slotted button b, visiting c,a,d,end and d,a,c,start. No previously matching trace became a mismatch.

Both native guards passed in 0.34 seconds after a 107-second native build. The initial source failed to compile because four computed-style accessors were unavailable in pyro-dom. The tested revision reads the fields directly and built in 108 seconds. The Taffy and Blitz vendor patches both verified against the parent.

## Real keyboard traversal cost

Eight fresh browser processes ran serially in ABBA/BAAB order. Each fixture used one warm-up and four measured samples per process. A sample dispatched 32 Tab and 32 Shift+Tab steps via CDP, with keydown and keyup for every step. Each sample returned to the starting button. The reading-container baseline performed its old DOM-order behavior. Medians include CDP round trips. CPU values are mean process-tree CPU per sample.

| Container | Parent median ms | Candidate median ms | Parent / candidate CPU ms |
| --- | ---: | ---: | ---: |
| Normal | 21.70 | 22.46 | 11.88 / 12.50 |
| Source order | 21.82 | 26.86 | 13.13 / 15.00 |
| Flex flow | 22.71 | 23.59 | 13.75 / 15.00 |
| Grid columns | 24.94 | 24.40 | 13.13 / 7.50 |

The largest observed increase was 5.04 ms per 64 steps for source-order. Normal traversal increased by 0.76 ms per 64 steps in this fixture.

The 16.515-second series ran on a busy host: mean CPU 88.24% on baseline arms and 87.69% on candidate arms. Editing and scoped-registry native builds overlapped the window, with IDB build/native work present for most of it. External CI, census and independent QA did not overlap. [Per-arm host receipt](host.json)
