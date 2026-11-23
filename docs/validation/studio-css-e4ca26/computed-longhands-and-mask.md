# Named computed longhands and mask resets

Parent `865773b5`. Same WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, Chrome stable 158.0.8127.37 and six-job inventories as `calc-size-nested-bases.md`. No expectation or corpus edits.

| Main (8,082 URLs) | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,455 | 176,358 | 194,331 |
| Candidate | 5,460 | 176,595 | 194,538 |
| Chrome | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive: 5/0 pages, 239/2 passes and 207/0 observed. Candidate/Chrome: 253/1,743 pages, 5,647/12,015 passes. Harness complete 0/1/2: 7,867/98/105; 11 timeouts, one hang, zero missing URLs. The 306 CSSOM/viewport URLs gained 171 passes and lost none; computed-initial zoom now runs instead of throwing when reading an indexed longhand by name. The 41 supplemental URLs stay at 27/572 versus 35/619 (874 observed); 39 crash/security exclusions remain separate.

An intermediate build (+229/-15 passes) lost eight observations from caching the enabled longhands before font-variant-emoji/hyphens initialization, six passes from exposing actual mask-border values rather than `undefined`, and one intermittent import assertion. Initializing the supported features early and expanding the mask shorthand to its reset-only longhands restored these. The mask reset test now passes all 14 names. The two retained losses on mask-valid.sub.html are its “should not set unrelated longhands” tests: their expected lists omit mask-border-mode, while mask-shorthand-subproperties-reset.html explicitly checks its reset. Neither test was changed. Three six-job controls: parent/candidate offscreen ready-time passed 1/3 and 2/3; constructed stylesheet import passed 2/3 and 3/3. Do not count the offscreen page gain as a confirmed timing fix. Web API integration: 633 passed.
