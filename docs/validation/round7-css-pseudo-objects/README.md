# CSSPseudoElement lookup, identity and traversal

Source `277154f9b0c8378e7e91bc4d9462ef4077eff179` was tested against direct parent `56066285e0f8e336b900dc6c7027f8ad1e7f8384`, using pinned WPT a421f2fea and JSC 0e0dd7038a7a-6f4649e395e0. Lookup exposes handles for supported selectors, including generated content, nested markers, directional scroll buttons and named view-transition descendants. Handles retain their originating element and immediate parent. The lookup cache is realm-local and holds handles through weak references.

Independent QA compared all 53 named occurrences against the archived Studio Chrome 158 reference. The six ordinary pages went from 0/47 to 27/47 (+27/−0). All 12 ordinary assertions that pass signed Chrome now pass Pyro. The other 15 gains pass pinned WPT where this Chrome build fails. The event guard ran separately with testdriver and retained 0/6 (signed Chrome 6/6). All seven harnesses completed without restarts.

| Ordinary page | Passed |
| --- | ---: |
| Stable identity | 1/1 |
| Geometry edge cases, including lookup | 4/23 |
| Scroll-button object lookup and geometry | 11/12 |
| Selector text | 4/4 |
| Scoped view transitions | 2/2 |
| View-transition hierarchy | 5/5 |

The direct fixture matched live signed Chrome for identity, distinct owners, parent traversal, stylesheet removal, reparenting and detachment/reattachment. The native guard passed in 0.07 seconds after a 55.05-second test build; the one-job executable build took 66 seconds. The remaining 20 ordinary failures require geometry methods. Event dispatch and retargeting were outside this batch. [CSS Pseudo-Elements draft](https://drafts.csswg.org/css-pseudo-4/#CSSPseudoElement-interface)

## Representative cost

Eight fresh processes ran serially in ABBA/BAAB order. Each fixture had one excluded warm-up and five measured samples per process. Ordinary reads check the parent of each of 256 elements eight times. The candidate-only fixtures obtain 256 new before handles or perform 2,048 lookups with live handles retained for identity checks. Wall medians include the CDP evaluation. First-handle samples use fresh elements with an already loaded interface.

| Executable and operation | Median ms per sample |
| --- | ---: |
| pseudo-base-56066285e, ordinary | 0.382 |
| pseudo-head-277154f9b, ordinary | 0.405 |
| pseudo-head-277154f9b, coldHandles | 0.558 |
| pseudo-head-277154f9b, cachedHandles | 1.882 |

Host occupancy averaged 20.87% across the 1.957-second series. The ordinary paired measurements are submillisecond and are descriptive. [Per-arm host receipt](host.json)
