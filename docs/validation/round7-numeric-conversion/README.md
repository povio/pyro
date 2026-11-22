# Typed OM parsing and unit conversion

Exact source `09e9b79fdae2a2162619cc01f213757dd9cad5f9` was independently compared to direct parent `cc4f21a5ed6c742057bd541ce7f7ef3791b37aa8`, on ten frozen Window WPT pages, pinned revision a421f2fea and JSC 0e0dd7038a7a-6f4649e395e0. Pyro rose from 186 to 191 of 191 named passes (+5/−0), matching the signed Studio Chrome 158 reference on every occurrence. All ten harnesses completed without browser restarts.

CSSNumericValue.parse combines compatible numeric leaves inside parsed sums. Explicitly constructed math objects preserve their operands. Conversion retains canonical unit powers through sums, products, reciprocals and extrema. This permits cancellation before converting to the requested unit. The five gains cover parsed same-dimension lengths and numbers, a clamp, a reciprocal product and a compound conversion.

Six of seven direct controls returned exactly the same result as live signed Chrome. The compound quotient returned 4000 ms in Pyro and 4000.000000000001 ms in Chrome; the parent threw TypeError. Both native guards passed in 0.09 seconds after a 68-second native build. The one-job executable build took 37 seconds. [CSS Typed OM sum values](https://drafts.css-houdini.org/css-typed-om-1/#create-a-sum-value)

## Conversion cost

Eight fresh processes ran serially in ABBA/BAAB order, with one excluded warm-up and five measured samples of 1,024 operations per process per fixture. Wall medians include page evaluation; CPU means are process-tree measurements per sample. The compound cancellation fixture was supported only by the candidate.

| Executable | Fixture | Median ms | Mean CPU ms |
| --- | --- | ---: | ---: |
| numeric-convert-base-cc4f21a5e | unit | 0.99 | 2.0 |
| numeric-convert-base-cc4f21a5e | sum | 1.31 | 4.0 |
| numeric-convert-base-cc4f21a5e | parse | 2.66 | 8.0 |
| numeric-convert-09e9b79fd | unit | 0.83 | 1.5 |
| numeric-convert-09e9b79fd | sum | 1.59 | 6.0 |
| numeric-convert-09e9b79fd | parse | 3.16 | 7.5 |
| numeric-convert-09e9b79fd | compound | 2.98 | 8.5 |

The initial comparison covered 2.866 seconds. Host occupancy averaged 26.14% on baseline arms and 26.79% on candidate arms, 26.49% overall. The direct controls had finished; no compiler or CI worker appeared in adjacent snapshots. [Initial per-arm host receipt](first-host.json)

## Scalar-sum follow-up

Final source `381839e9038bfa889e7ce3c1d9c952ec4e2b568c` converts scalar operands sharing a canonical unit without constructing compound term maps. It reads each operand at conversion time. Independent QA retained 191/191 and all earlier gains. Changing an original operand and then an operand via sum.values yielded 97, 193 and 189 px successively, matching signed Chrome. Both native guards passed in 0.08 seconds after a 33.44-second test build. The final binary took 43 seconds to build.

Each series used the same executable for exact cc4f21a5e. First and refined candidates ran in separate paired series. Times are median wall milliseconds per 1,024 operations.

| Fixture | First base | First candidate | Second base | Refined candidate |
| --- | ---: | ---: | ---: | ---: |
| unit | 0.991 | 0.832 | 1.694 | 2.014 |
| sum | 1.315 | 1.587 | 3.267 | 3.164 |
| parse | 2.660 | 3.155 | 3.767 | 5.100 |
| compound | — | 2.982 | — | 6.808 |

Host occupancy during the refined series averaged 85.14% over 4.826 seconds, 84.15% on baseline arms and 85.92% on candidate arms. The first comparison averaged 26.49%. Even with the identical baseline and alternating arm order, absolute timings differ substantially between these two load windows. In the refined series the ordinary scalar sum median was 3.2% lower than its paired baseline, while parse plus conversion was 35.4% higher and direct unit conversion 18.9% higher. The later two lower-wall-time pairs for parse plus conversion still measured 3.01 vs 2.78 ms and 3.00 vs 2.71 ms (candidate vs parent process medians). This leaves a measured cost for constructing the simplified parse result; the timing does not isolate its individual operations. [Refined host occupancy](refined-host.json)
