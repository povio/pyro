# Intrinsic size CSSOM round trip and transition readback

On WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, the matched focused inventory is all 281 main-cohort URLs under `css-sizing/` or `calc-size/`. LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, macOS 27.2.1. Predecessor `8bb0f703`; immutable candidate `calc-basis-final-candidate`. LP used six jobs, wptserve and testdriver; Chrome is the same pinned local stable 158.0.8127.37 reference, not Canary.

| Test set | Before | Candidate | Stable |
|---|---:|---:|---:|
| Height interpolation, named | 256 | 348 | 1,156 |
| Width interpolation, named | 210 | 228 | 732 |
| Ordinary css-sizing, strict pages / names | 153 / 5,709 | 153 / 5,709 | 227 / 6,241 |
| Entire focus, strict pages / names | 154 / 7,348 | 156 / 7,491 | 257 / 10,452 |

The serializer alone restored ordinary layout and animation declarations but left CSS transition height reads at the endpoint: the quarter-progress identity-to-double case read 100px before the serializer, 200px with just the serializer, then the correct 125px with the final layout basis. No live layout is modified by the CSSOM read. Width readback is not yet sampled.

There are 212 newly passing names and **69 lost names**, not 143 gains with no losses. Losses: 14 width interpolation, 6 logical minimum size, 24 minimum height, 24 minimum width, one indefinite intrinsic height child. The minimum-size harness also uses computed style to construct its expected values: restoring CSSOM serialization exposes missing `interpolate-size` support. Both LP runs completed 278 OK / 3 harness errors, no runner timeouts; Chrome completed 281 OK. Three observed names changed in each direction. Exact sets are in the neighboring `calc-basis` archives. The separate 41 supplemental URLs and 39 excluded URLs are not in these counts.
