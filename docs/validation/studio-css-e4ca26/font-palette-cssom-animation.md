# Font palette CSSOM and animation

Studio Chrome stable 158.0.8127.37 and LP 0.0.17 (JSC 0e0dd7038a7a-6f4649e395e0), WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The same 8,082 main URLs ran with six jobs. The comparison keys are URL for strict pages and (URL, subtest name) for names.

| LP checkpoint | Strict pages | Named passes | Pages gained / lost | Names gained / lost | Chrome-only pages / names | LP-only pages / names |
|---|---:|---:|---:|---:|---:|---:|
| Palette rule descriptors `2886b3fb` | 5,497 | 177,733 | 2 / 0 | 57 / 0 | 1,709 / 10,925 | 256 / 5,695 |
| Palette animation `1848d88c` | 5,497 | 177,775 | 0 / 0 | 42 / 0 | 1,709 / 10,883 | 256 / 5,695 |

Descriptor parsing passed all 36 valid and 27 invalid WPT cases. Only the corrected `palrule2` binary is counted. Palette interpolation improved from 70/114 to 112/114; two serialization assertions remain, and unspecified endpoints stayed at 1/2. Neither batch lost a previously passing name in the matched main or supplemental run.

Chrome passed 6,950 strict pages and 182,963 names out of 194,912 observed. Both LP candidates observed 194,540 names, with 11 page timeouts and one hang each. Exact outcomes and directional differences are in the neighboring `palrule2` and `palanim` archives. The separate 41-URL supplement stayed at LP 27 pages / 577 names and Chrome 35 / 619 (874 observed each); the other 39 manifest URLs were withheld and are not passes.
