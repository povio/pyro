# Font feature values and font-face auto descriptors

Studio, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, six jobs. Identical main inventory and Chrome stable reference as the preceding descriptor checkpoint. Strict pages require successful completion, at least one named assertion, and all assertions passing; named keys are (URL, name).

| Main: 8,082 URLs | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent `8737351a` | 5,416 | 174,968 | 194,312 |
| Candidate | 5,418 | 175,077 | 194,305 |
| Chrome 158.0.8127.37 | 6,950 | 182,963 | 194,912 |

All changes in passing sets were reproduced in the 199-URL font focus: +2/−0 pages, +116/−7 names. The seven former passes were conditional round-trip assertions generated only after accepting invalid rules; none now fail. The parsing page completes 299/299. No other pass losses or changed completion counts. Candidate/Chrome exclusives: 246/1,778 pages and 5,568/13,454 names. The focus measured parent 157/8,442, candidate 159/8,551, Chrome 168/8,624. The four compressed differences preserve both directions and observed sets.

Main candidate: 7,864/101/105 completed with harness status 0/1/2; 11 timeouts, one hang. Chrome: 7,986/23/62; ten timeouts, one hang. Outside main: 41 supplemental (4 UTF-16, 37 workers; prior LP 27 pages/436 names, Chrome 35/619) and 39 crash/security exclusions, not rerun. H2 and testdriver limitations unchanged. No expectations changed. A separate Unicode-range comment experiment was discarded: joining separated tokens accepts invalid ranges and is not the CSS Syntax source re-tokenization algorithm.
