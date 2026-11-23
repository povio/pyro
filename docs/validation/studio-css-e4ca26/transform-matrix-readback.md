# Resolved transform matrices versus Typed OM

On Studio, the 8,082-URL main CSS testharness cohort used WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`, and Chrome stable 158.0.8127.37. CSSOM multiplies the sampled list against the unzoomed reference box after resolving individual lengths; a matrix's unitless translation coefficients must not themselves be unzoomed. Typed OM retains computed transform components. A separate Chrome invocation confirmed `translate(25%, 0px)` for `translateX(25%)`.

| Main | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent c75e9051 | 5,419 | 175,190 | 194,305 |
| Matrix readback | 5,444 | 175,796 | 194,304 |
| Chrome stable | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive sets: 26/1 pages, 627/21 passes and 19/20 observed names. Candidate/Chrome: 248/1,754 pages and 5,574/12,741 names. Both LP runs: 7,864/101/105 complete with harness status 0/1/2, 11 timeouts and one hang. Chrome: 7,986/23/62, ten timeouts and one hang. The compressed outcomes and both exact differences accompany this note.

Loss accounting:

- `minmax-angle-serialize.html`: 19 parent-only names replaced by 19 matrix-valued names. Both passed all 38 assertions in the main run and every repeat.
- `CSSStyleSheet-constructable-disallow-import.tentative.html`: the one strict-page loss and one lost name in the full run. Six focused copies at six jobs passed all four names on both binaries; the full-run loss remains counted.
- `transform-composition.html`: one genuine named loss, three out of three on each side. At progress 1.5 CSS animation sampling produced a final Z translation of −352.124 versus −352.125 for the static reference. The CSSOM-written easing is reparsed from its six-digit attribute serialization (`1.83333`); the Web Animations equivalent passes. No tolerance or WPT changes. Across the whole composition page the candidate passes 55 names to the parent's 42 in every repeat.
- Zoomed animated matrix retained its pass in the full run and all three repeats. `CSSStyleSheet-constructable.html` passed 25/32 on each side in all three repeats.

Outside main: 41 supplemental URLs (4 UTF-16, 37 worker; prior LP 27 pages/436 names, stable 35/619), not rerun here; 39 crash/security exclusions. H2 and testdriver limitations unchanged.
