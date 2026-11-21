# Structured serialization of CSS Geometry values

Independent QA compared exact source `73b72e644673ee5d19df1cf3b4a3187e000604c4` to direct parent `3142bdcb2d983228fac894fa0b16d8ae3cf1bd06`, on pinned WPT a421f2fea with JSC 0e0dd7038a7a-6f4649e395e0. All 250 named occurrences matched across six pages. Serialization improved from 1 to 31 of 31; all 30 gains pass in the signed Studio Chrome 158 reference. The other 219 outcomes were unchanged. The combined score rose from 211 to 241 of 250. Eight DOMMatrix-001 failures and one invert-preserves-2d failure predate this change. All harnesses completed without a restart.

The implementation reads the point, rectangle, quad and matrix slots, preserves mutable and read-only types, and reconstructs geometry in the receiving realm. The WPT covers custom and throwing properties, non-finite values, signed zeros and matrix dimensionality. The existing DOMRectList rejection remains counted as a pass. Both native guards passed in 0.13 seconds after a 62-second test build. The one-job executable build took 30 seconds.

## Quad point identity

For the graph `[q.p1, q, q]`, Pyro preserves both the repeated quad reference and the alias between the standalone point and the cloned quad's first point. Mutation through that point is observable through either path in the clone. The original geometry remains independent. This held for structuredClone, same-realm MessageChannel, dedicated worker echo, history.state and IndexedDB. Live signed Chrome 158.0.8127.37 preserved the repeated quad but produced distinct points on each of those five routes. This direct difference is separate from the 30 WPT gains. Fourteen neighboring WebCodecs, storage and ordinary messaging controls retained their reference results.

Geometry Interfaces §7 defines each serialized DOMQuad slot as the **sub-serialization of the point object**, and each deserialized point as the **sub-deserialization** of that slot. HTML supplies the same enclosing memory map to both recursive operations, including the forStorage flag on serialization and targetRealm on deserialization. The maps preserve the already visited point identity. [Geometry Interfaces §7](https://drafts.csswg.org/geometry-1/#structured-serialization) · [HTML sub-serialization](https://html.spec.whatwg.org/multipage/structured-data.html#sub-serialization) · [HTML sub-deserialization](https://html.spec.whatwg.org/multipage/structured-data.html#sub-deserialization)

## Representative cost

Eight fresh processes ran serially in ABBA/BAAB order. Each fixture ran one warm-up and five measured samples per process, with 1,024 clones per sample. Each clone checked a repeated object alias and independence from the source. Only the ordinary graph exists on both executables. The candidate geometry fixture used a mutable DOMPoint in place of the ordinary point-shaped record. Median wall time includes page evaluation.

| Binary | Fixture | Median ms | Mean process-tree CPU ms |
| --- | --- | ---: | ---: |
| geom-base-3142bdcb2 | ordinary | 2.40 | 3.5 |
| geometry-73b72e644 | ordinary | 2.40 | 4.5 |
| geometry-73b72e644 | geometry | 3.16 | 4.0 |
