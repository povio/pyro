# Document stylesheet insertion costs

Both comparisons used exact baseline `7a2b3b00c`. The first compared the accepted tree-order change `a92a105d0`; the second compared the sibling lookup optimization `fe6f3d382`. Each series ran eight fresh processes in ABBA/BAAB order on the same host. Each fixture had one excluded warmup and three measured samples per process. Each sample started with the indicated sheet count, performed 16 mutations and synchronously read the winning declaration after each mutation (twice for remove/reinsert). Append therefore grows the live set by 16. The table shows median in-page milliseconds for the 16 operations. Per-process CPU accounting is coarse for these short samples; raw CPU, wall time, UTC and observed cascade mismatches are in the JSONL receipts.

| Workload | Starting sheets | First baseline | Accepted | Second baseline | Optimized |
| --- | ---: | ---: | ---: | ---: | ---: |
| append | 4 | 0.60 | 0.60 | 0.60 | 0.60 |
| reorder | 4 | 0.25 | 0.20 | 0.20 | 0.20 |
| reinsert | 4 | 2.00 | 2.05 | 2.00 | 2.05 |
| append | 32 | 0.70 | 0.70 | 0.75 | 0.65 |
| reorder | 32 | 0.30 | 0.30 | 0.30 | 0.30 |
| reinsert | 32 | 2.30 | 2.45 | 2.40 | 2.35 |
| append | 128 | 1.50 | 1.40 | 1.50 | 1.10 |
| reorder | 128 | 0.60 | 0.60 | 0.60 | 0.60 |
| reinsert | 128 | 3.90 | 4.20 | 3.95 | 3.90 |

At 128 live sheets the initial remove/reinsert slowdown appeared in all four pairs (+7.7% to +12.8%). With sibling lookup, individual paired changes varied from −7.3% to +10.8%, with pooled medians 3.95→3.90 ms. At 128 sheets the optimized append was faster in all four pairs. All optimized append and remove/reinsert readbacks matched tree order. Connected-move readbacks were incorrect on the baseline and both candidates, so their timings describe the existing incorrect behavior. That defect is tracked separately.

Weighted host CPU over 15.650 seconds was 26.79% on base arms and 27.04% on the accepted arms; over the optimized 15.858-second series it was 30.45% on base arms and 26.95% on optimized arms. No census or external CI worker overlapped either run. Independent QA on the optimized executable matched all 423 previous outcomes (422 passes and the existing computed-map sort failure). Both native registration guards passed.
