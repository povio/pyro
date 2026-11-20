# Nested and length calc-size bases

Parent `572c16d4`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`; local stable Chrome 158.0.8127.37. Six WPT jobs, same inventories and comparison method as `geometry-rect-list.md`.

| Main cohort (8,082 URLs) | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,453 | 176,050 | 194,329 |
| Candidate | 5,455 | 176,358 | 194,331 |
| Chrome | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive: 2/0 pages, 308/0 passing names, 2/0 observed names. Candidate/Chrome exclusive: 249/1,744 pages, 5,577/12,182 passes, 306/887 observed names. No missing pages; harness complete status 0/1/2: 7,866/99/105; 11 timeouts and one hang.

On 281 sizing URLs: 163/8,617 parent → 164/8,922 candidate, stable 257/10,452 (strict pages/passing names). No lost names. Width interpolation 504 → 576; height 564 → 750; parsing 78 → 85, static width 8 → 15, static height 17 → 31. The replaced-element page newly passes strictly. Off-cohort fixed table layout also exercises nested bases. The other strict page gain (constructed stylesheet import) is intermittent: 3/3 parent and 2/3 candidate on six-job 20-page repeats. The previous offscreen transition ready-time failure remains intermittent (1/3 and 2/3); neither discrepancy is excluded from totals.

The 41 separate UTF-16 and worker URLs are unchanged at 27/572 LP versus 35/619 stable (874 observed). The 39 crash/security exclusions and H2/testdriver limits are unchanged. Render integration: 1,417 passed, three ignored; includes negative intermediate sizing. The implementation preserves the computed expression while storing a bounded prepared calculation for layout and interpolation. Percentage-containing numeric bases and mixed authored percentage/size calculations remain unsupported; max-size functions are not generalized in this batch.
