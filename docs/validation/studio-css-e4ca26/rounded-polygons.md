# Rounded polygon geometry

Matched Studio testharness, six workers, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. The 63 focus URLs cover shape parsing, motion paths, masking, composition and interpolation. Candidate `dcf56dfa` adds a nonnegative length to the polygon model and shares the tangential, per-vertex clamped circular arcs across float layout, paint, SVG clipping and motion paths.

| Focus build | Strict pages | Named passes | Names observed |
|---|---:|---:|---:|
| Parent (scrollport range) | 49 | 7,126 | 7,288 |
| Rounded polygon | 59 | 7,274 | 7,288 |
| Chrome stable | 56 | 7,249 | 7,288 |

Gained 10 strict pages and 148 named passes without dropping any passing or observed name. Candidate-only vs stable: 7 pages / 39 names; stable-only: 4 / 14. These are not full-suite counts. All three vendor patches applied in both directions against their reconstructed upstreams.

Two separate rendering probes are not part of the testharness score. The oversized-radius clamp reftest matches exactly on both binaries (not a new pass). The rounded clip against a border-radius reference improves from 1,528 differing pixels (max 255) to 204 (max 3), **still failing** the test's 100-pixel/max-1 limit. Expectations and tolerances are unchanged.
