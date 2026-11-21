# Preferred stylesheet sets and disabled owners

Frozen ancestor: `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Parent `c5107aa5dc03a73425598682edf205d3506bf8b5` adds only evidence to the tested executable's source `dd19a5242aa96fbcb82a61e548a28a50814d0535`. Candidate source: `255eb2f4d8aa5953a5b9403e17daf86cba48ff90`. WPT `a421f2fea5eb`, Chrome `158.0.8127.37`, patched JSC `0e0dd7038a7a-6f4649e395e0`.

The two manifests contain disjoint completed, non-withheld archive URLs. Each JSON is the original matched runner output; runs used isolated wptserve ports and at most four jobs. No full-suite run.

| Focus | Parent pages; named; wall | Candidate pages; named; wall | Chrome pages; named; wall |
| --- | --- | --- | --- |
| `urls.json` | 6/10; 48/52; 99 ms | 10/10; 52/52; 123 ms | 10/10; 52/52; 835 ms |
| `titles.json` | 2/3; 3/4; 148 ms | 3/3; 4/4; 76 ms | 3/3; 4/4; 227 ms |

Five strict pages and five named subtests gained; none lost. All harnesses completed; matching URL/name/occurrence inventories, zero watchdog kills or restarts.

| Newly passing page | Newly passing subtest |
| --- | --- |
| `HTMLLinkElement-disabled-004.html` | HTMLLinkElement.disabled's explicitly enabled state doesn't persist on clones |
| `HTMLLinkElement-disabled-006.html` | HTMLLinkElement.disabled setter does nothing if the attribute isn't present already. |
| `stylesheet-title.html` | Preferred style sheet name |
| `preferred-stylesheet-order.html` | Preferred stylesheet where insertion order is reversed tree order |
| `style-sheet-interfaces-001.html` | disabled attribute getter/setter |

Native DOM integration (same four test filters): 2/4 parent, 4/4 candidate. The two added tests fail on the parent and pass on the candidate; both existing controls stay green. Native elapsed times including compilation: 3.31 s parent, 11.74 s candidate. Candidate browser build: 37.11 s.
