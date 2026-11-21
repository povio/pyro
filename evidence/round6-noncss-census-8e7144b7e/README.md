# Non-CSS census at 8e7144b7e

The pinned-JSC binary at 8e7144b7e7de68be3b5b345138abe2c1dcc6d232 (SHA-256 daea9cf80d257a1a0574e625ba38e58358104defbf2b7dee408fd8af667d72f8) was compared with the frozen signed stable Chrome 158.0.8127.37 reference at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The reference ran on macOS 27.2.1 arm64. Pages use fresh contexts, four jobs per shard, an 800 × 600 viewport at scale 1, HTTP/2, wptserve and testdriver.

Of 31,827 manifest URLs, 30,358 were matched. The 1,468 originally withheld variants and the subsequently recorded Chrome hang have explicit page rows without a new LP observation. The combined binary reused 3,786 of its own matched guard observations and measured 26,572 additional distinct URLs. Timeouts and harness errors retain any named results reported.

| Metric | LP | Chrome | LP only | Chrome only |
|---|---:|---:|---:|---:|
| Named passes | 1,688,041 | 1,755,476 | 12,119 | 79,554 |
| Full pages | 15,327 | 24,533 | 605 | 9,811 |

Named rows retain the variant URL, server-port-normalized name, one-based occurrence and both browsers' status and original name. The 292 historical occurrences are separately indexed. Use `python3 check.py .` for an independent readback of every page and named count. The earlier cbdeb1193 census included the subsequently held Chrome hang and has its own denominator.
