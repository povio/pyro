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
