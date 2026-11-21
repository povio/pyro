# Font-relative arithmetic in registered numeric custom properties

Independent QA used the shared combined build at exact base `bd247dd67babbff29fb87017f9a6bededfb68ea2`, pinned WPT a421f2fea, JSC 0e0dd7038a7a-6f4649e395e0 and the archived Studio Chrome 158.0.8127.37 reference. The selection covered five ordinary pages and 133 named occurrences.

The initial experiment `6afd37e0c` deferred numeric expressions containing font-relative lengths until the element's font was available. That correctly resolved 18 standalone calculations but changed the basis of 30 values consumed by font-size-adjust, font-weight, font-stretch, font-style, font-feature-settings and font-variation-settings. Twenty-seven of those lost passes also passed Chrome. That candidate was rejected.

Revised source `959768c2b12cdf13dfe68cd58a482bc423e4a0dd` resolves selected numeric inputs to winning font-affecting declarations using inherited font metrics. Independent numeric calculations use the computing element. Substitution follows selected variable references and used fallbacks; existing font-size and line-height cycle processing runs before these inputs. Native controls cover inheritance, an unregistered variable, re-computation after font-size changes, variable indirection, selected and unused fallbacks, and a genuine registered-length/font-size cycle.

| Named pass set | Exact parent | Rejected experiment | Revised candidate | Studio Chrome 158 |
| --- | ---: | ---: | ---: | ---: |
| Font-property unit cycles (95) | 68 | 56 | 86 | 74 |
| Four adjacent pages (38) | 36 | 36 | 36 | 38 |

All 18 gains versus the parent pass in signed Chrome, and all 30 prior regressions recovered. The revised target page preserves 12 baseline passes where signed Chrome fails and has nine shared failures. The adjacent pages retain two baseline failures involving the serialization precision of unregistered font-size values. Both native guards passed in 0.07 seconds following a 40.82-second test build; the one-job executable build took 120 seconds.

CSS Values 4 specifies parent metrics for font-relative lengths in font-affecting properties. CSS Properties and Values API Level 1 explicitly describes cycle edges for length and length-percentage registrations. Generalizing those edges to numeric results from unit arithmetic is still under discussion in issue 1080. [CSS Values 4](https://drafts.csswg.org/css-values-4/#font-relative-lengths) · [Properties and Values API](https://drafts.css-houdini.org/css-properties-values-api-1/#dependency-cycles-via-relative-units) · [CSS Houdini issue 1080](https://github.com/w3c/css-houdini-drafts/issues/1080)

## Cascade cost

One serial ABBA/BAAB run used eight fresh browser processes, one excluded warm-up and five measured samples per fixture per process. Each sample wrote and read 64 elements eight times. Wall medians include the 512 writes, 512 reads and CDP evaluation. CPU means are per sample from process-tree counters.

| Fixture | Parent median ms | Candidate median ms | Parent / candidate mean CPU ms |
| --- | ---: | ---: | ---: |
| ordinary | 3.81 | 3.81 | 4.0 / 4.5 |
| independent | 4.98 | 4.97 | 5.0 / 4.5 |
| fontInput | 5.36 | 5.54 | 5.0 / 5.5 |

The 2.991-second run averaged 26.61% host CPU, with 26.38% on parent arms and 26.85% on candidate arms. Neither CI nor the five-page QA run overlapped. The active font-input fixture took 0.18 ms more per 512 writes and reads. Short-run process-tree CPU counters have coarse resolution. [Per-arm host receipt](host.json)
