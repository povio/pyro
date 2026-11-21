# DOMMatrix construction from CSS transform strings

Tested source `e26a3c9161ebcb28a860586e9f09f16339e122fb` uses Stylo's existing specified transform parser and matrix conversion with no element or reference box. The empty string yields identity, CSS comments are tokenized normally, and context-independent arithmetic is supported. Invalid input leaves an existing mutable matrix intact. In a Worker, string constructors throw TypeError and the Window-only stringifier and setMatrixValue are absent. Numeric constructors and mutable/read-only types remain available.

The exact integrated parent was `97761dfa5af02f6ce2e3433a72b522a9413a0459`. Independent QA matched 250 names over the same six pinned WPT pages and signed Chrome 158.0.8127.37 reference. The result improved from 241 to 249 of 250 (+8/−0). DOMMatrix-001 now passes 138/138, serialization retains 31/31, and the previously observed scale inversion precision failure remains. The supplemental generated dedicated-worker page improved from 0 to 11/11. All 19 gains pass signed Chrome. The seven harnesses completed without restarts.

In separate direct checks, all 20 Window and 20 Worker readbacks on the final head matched live signed Chrome. These cover both constructors, 2D/3D values, invalid setter atomicity and the Window/Worker exposure distinction. Two native guards passed in 0.49 seconds after a 55.80-second test build. The one-job final executable build took 46 seconds. [Geometry Interfaces §6.2](https://drafts.csswg.org/geometry-1/#parse-a-transform-list)

## Constructor cost

Eight fresh processes ran serially in ABBA/BAAB order. Each fixture had one warm-up and five measured samples of 1,024 constructions with validated output. Arrays and simple CSS strings were compared on both binaries; nested calc was measured only on the candidate. Wall medians include the CDP page evaluation. CPU values are coarse process-tree means per sample.

| Executable | Fixture | Median ms | Mean CPU ms |
| --- | --- | ---: | ---: |
| matrix-base-97761dfa5 | array | 0.79 | 2.0 |
| matrix-base-97761dfa5 | string | 1.73 | 2.5 |
| matrix-e26a3c916 | array | 0.77 | 2.0 |
| matrix-e26a3c916 | string | 1.30 | 1.5 |
| matrix-e26a3c916 | math | 1.43 | 1.0 |
