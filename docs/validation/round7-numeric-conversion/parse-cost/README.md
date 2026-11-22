# Isolated Typed OM parse and conversion costs

The correctness checkpoint `fa924816d` was preserved. On that checkpoint, parsing 1,024 compatible sums took 2.221 ms versus 1.992 ms on the original exact `cc4f21a5e` executable. Converting an already parsed sum took 0.867 vs 1.100 ms. The original implementation allocated a map, two intermediate converted unit objects and a replacement unit for a common two-leaf sum. Source `74b1f31fbd3d7aa94b08b14af4fb3ce9f7d2c390` folds compatible parser-owned leaves directly. It never mutates a script-supplied operand.

Independent QA retained 191/191 across ten pinned WPT pages, matching signed Studio Chrome and preserving +5/−0 versus exact cc4f21a5e. All 18 direct observations matched frozen source 381839e90, including two- and three-term parsing, unresolved terms, operand identity and mutation. Seventeen matched live signed Chrome exactly. The remaining result was 4000 vs 4000.000000000001 ms on compound conversion. Both native guards passed in 0.07 seconds following a 31.78-second native build.

The two independent ABBA/BAAB series used the same original parent executable and identically ordered fixtures. Each arm used a fresh process, one warm-up and five measured samples per fixture. Each sample checked 1,024 operations. Parse-only verifies each returned sum; parsedRead converts one already parsed value; parse combines both operations. Mutable changes one operand before each conversion. Compound cancellation was supported only by the checkpoint and follow-up. Median wall times below include the CDP evaluation and are reported in milliseconds.

| Fixture | Original base, first series | Frozen 381839e90 | Original base, second series | Final 74b1f31fb |
| --- | ---: | ---: | ---: | ---: |
| unit | 0.971 | 0.859 | 1.029 | 0.896 |
| sum | 1.271 | 1.112 | 1.388 | 1.074 |
| parseOnly | 1.992 | 2.221 | 1.906 | 1.608 |
| parsedRead | 1.100 | 0.867 | 1.022 | 0.910 |
| parse | 2.420 | 2.732 | 2.434 | 2.154 |
| mutable | 0.967 | 1.017 | 0.962 | 0.927 |
| compound | — | 2.797 | — | 2.922 |

The measured parse-only and parse-plus-convert penalties disappeared in the final paired series. First-series occupancy was 19.13% baseline and 19.21% checkpoint. The two candidate revisions were measured in separate alternating runs.

The final 4.380-second window averaged 34.27% host CPU, with 32.37% on baseline arms and 36.00% on candidate arms. Dedicated QA and external CI had zero overlap; the midpoint snapshot showed no compiler. Results are descriptive for these fixtures. [Final host receipt](final-host.json)
