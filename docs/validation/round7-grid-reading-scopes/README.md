# Grid reading order across contents and slot scopes

Exact candidate `237a30897abefc4308ba401d7fea64d98c2875b5` starts from `d045ec7b84c004757e2aac7927d772932fb16452`. A display:contents wrapper participates at the earliest order of its generated children within grid-order navigation. Its descendants stay grouped for focus traversal. A boxless slot projects its assigned nodes, or its fallback children, into the enclosing reading scope. A slot generating a block retains its own scope. The implementation changes sequential focus navigation. It uses existing computed styles without modifying the renderer.

Independent QA compared 13 pinned WPT occurrences on four testdriver pages to exact parent and signed Chrome 158.0.8127.37. Pyro improved from 5 to 13/13 (+8/−0). Three more pages passed fully. All six gaps where Chrome passed closed (four slots, one contents and one cross-scope). The two new WPT passes where signed Chrome fails are the focusable tabindex contents owner and nested display:contents containers. The shadow-host page retains 2/2. Separately, neighboring reading focus retained 12/12, flex visual order 8/8 and declarations 40/40. All harnesses completed without restarts.

Direct keyboard traces improved from 22/30 to 26/30 agreement with live signed Chrome. Grid cases with boxless/block slots, wrappers and nested owners matched. Both Pyro revisions differ from Chrome in forward and reverse traversal of a flex-flow display:contents case and a source-order named-slot case. Both native guards passed in 0.09 seconds after a 104-second native build. The single-job executable build took 25 seconds.

## Real Tab cost

Each of eight serial fresh processes ran the same fixtures in ABBA/BAAB order. One warm-up and four measured samples per process were used. Each sample dispatched 32 forward and 32 reverse Tab steps, checked its first target and verified return to the start. Wall medians include CDP round trips. The source and candidate execute different traversal orders where the change corrects focus. Mean CPU figures use coarse process-tree counters.

| Fixture | Parent median ms | Candidate median ms | Parent / candidate CPU ms |
| --- | ---: | ---: | ---: |
| Ordinary DOM | 24.53 | 21.93 | 8.75 / 7.50 |
| Plain grid-order | 22.00 | 21.44 | 6.88 / 5.00 |
| Grid-order with contents | 22.02 | 21.22 | 5.62 / 5.62 |
| Grid-order with slot | 38.11 | 39.37 | 22.50 / 23.12 |

The slot workload rose by 1.26 ms per 64 steps. These medians describe the tested host and fixtures.

Host CPU averaged 39.28% over 7.593 seconds, 44.54% on baseline arms and 33.52% on candidate arms. [Per-arm host receipt](host.json)
