# CSS interactivity and HTML inertness

Exact parent `95976a331bb1870999ab41f7d87cfc8d8c40d206` and the two candidates used pinned WPT a421f2fea, JSC 0e0dd7038a7a-6f4649e395e0 and the archived signed Studio Chrome 158 profile. Independent QA matched all 24 named occurrences across eight testdriver pages.

The first source, `e99bd9ceb`, rose from 12 to 22/24 (+10/−0). The UA stylesheet exposes HTML inertness and lets a modal dialog reset inherited inertness. The style cache tracks host-language inertness separately from inherited CSS interactivity. Hit eligibility, programmatic focus eligibility and find checks use the derived inert state. A diagnostic showed that find skipped the inert text under a visible paragraph but still matched its duplicate under a script element. It also showed a modal button beneath an inert ancestor computed to auto while focus was still rejected by later checks.

Corrected source `7631b990cfc8a27e0b96b947cd44bf7d9fe33af1` reached 23/24 (+11/−0 versus the exact parent). Eight of nine originally missing UA computed-value outcomes now pass, and all three originally missing interaction outcomes (click, focus and find) pass. The native and independent direct probes verify that find skips nonrendered duplicates, modal focus persists through two animation frames and an outside focus attempt, and explicit author auto can override inherited author CSS inertness. Two previously passing WPT assertions for CSS descendant auto remain passes in Pyro and failures in signed Chrome. The adjacent focus cohort retained 12/12. Both native guards passed in 0.16 seconds after a 58.65-second test build. The corrected one-job executable build took 55 seconds.

The remaining WPT failure concerns genuine fullscreen within an HTML inert ancestor. The fullscreen request succeeded, but the descendant retained inherited inertness and focus/click/find remained blocked. The existing fullscreen style adapters do not represent fullscreen state in this source. Neither candidate changed fullscreen state, paint order or editing commands.

## Representative costs

Eight serial browser processes ran in ABBA/BAAB order. For each of three fixtures, each arm ran one warm-up and four measured samples. Each sample attempted 64 focus and find operations, then dispatched 32 trusted pointer moves. The ordinary and explicit-auto fixtures produced 64 successful focuses, 64 matches and 32 delivered moves on both executables. In the author-inert fixture the parent allowed all three operations, while the candidate suppressed all of them. These times therefore include the cost difference of performing versus suppressing the actions.

| Fixture | Parent median ms | Corrected median ms | Parent / corrected mean CPU ms |
| --- | ---: | ---: | ---: |
| Ordinary | 20.22 | 12.56 | 11.25 / 7.50 |
| Author CSS inert | 20.67 | 7.13 | 9.38 / 1.88 |
| Descendant explicit auto | 11.69 | 12.37 | 7.50 / 8.75 |

The explicit-auto case took 0.68 ms more per mixed sample. These fixtures did not exercise fullscreen behavior.

The 6.412-second window averaged 79.06% host CPU, 80.47% on baseline arms and 77.05% on candidate arms. The medians are descriptive for this workload and load distribution. [Per-arm host measurements](host.json)
