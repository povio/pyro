# Computed alignment, full comparison

On the original pinned 8,082 URLs `bffbaea9` gained 27 strict pages and 232 named passes over `23a2ec14`, with no losses and no change in observed names. LP now passes 5,373 strict pages and 171,975 named assertions; Chrome stable passes 6,950 and 182,963. LP recorded 7,864 OK, 101 error and 105 timeout harness completions, 11 runner timeouts and one hang. Strict pages exclusive to LP / stable: 242 / 1,819; named passes: 5,376 / 16,364; names observed on one side only: 255 / 858. Exact sets accompany this note.

The separate 41-URL supplement was last run on the padding binary (LP 27 / 436; stable 35 / 619). The 39 excluded URLs remain unmeasured. Largest remaining single-page stable-only sets are intrinsic height interpolation (900), system font animations (588), and intrinsic width interpolation (522).
