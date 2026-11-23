# System fonts, transition properties and view-transition selectors

Parent `26c804f9`. Exact URL and subtest keys, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, macOS Studio, JSC `0e0dd7038a7a-6f4649e395e0`, local Chrome stable `158.0.8127.37`. Six jobs, wptserve and the same testdriver setup. Strict requires an OK harness and all named assertions passing. Comparisons do not normalize assertion names.

| 398 URLs (css-fonts, css-transitions, css-logical, view-transition parsing) | Strict | Passing names | Observed |
|---|---:|---:|---:|
| Parent | 269 | 12,164 | 14,605 |
| Candidate | 278 | 13,243 | 14,605 |
| Stable | 359 | 13,911 | 14,601 |

Candidate versus parent: +9/-0 pages, +1,079/-0 passing names, +104/-104 observed names. Candidate/stable exclusive: 11/92 pages, 293/961 passing names. The six standard UI font roles are resolved before interpolating their longhands. The animation page passes all 588 assertions on LP and Chrome; 484 have identical names and the other 104 per engine include different resolved font metrics or family names. This is not a 588-name reduction in the Chrome-only set. All six tentative system-font serialization assertions still fail: reset-only longhands serialize as ordinary initial values rather than empty strings.

The initial font patch lost `font-variant-serialization.html`: serializing a mixed system font discarded `tabular-nums`. Retaining the system shorthand and its overrides restores the assertion. For view transitions, the four named pseudo-element kinds now use the selector parser's existing only-child restriction (not general state selectors); valid/invalid parsing controls gain 176 names without losses. For `transition: all`, physical longhands start once rather than again under logical aliases. Computed-style lookups for logical properties locate those same timelines using the after-change writing mode. The first physical-name prototype lost 41 assertions across five logical-animation pages; retaining explicit logical transitions and mapping readback recovered them all in the matched focus run.

The main inventory contains 8,082 URLs. Keep the 41 supplemental URLs (4 UTF-16, 37 worker) separate: prior LP 27 strict / 436 named; stable 35 / 619. No new supplemental result is implied. The 39 crash/security exclusions and existing H2/testdriver environment limitations remain unchanged.

Full main run at `91c2bf96`: 5,395 strict pages, 174,362 passing names and 194,312 observed names. Relative to parent: +9/-0 pages, +1,081/-0 passing names, +104/-104 observed. Stable: 6,950 / 182,963 / 194,912. LP-only/stable-only: 242/1,797 pages and 5,495/14,096 passing names. Both LP runs completed 7,864 pages with harness status 0, 101 with status 1 and 105 with status 2; 11 timed out and one hung. Stable: 7,986/23/62, ten timeouts and one hang. The offscreen transition-ready-time control passed one of three parent repeats and two of three candidate repeats at six jobs, so its behavior remains intermittent.
