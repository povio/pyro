# Position-try descriptor prototype

`5c0a8569` (tryproto) versus `e23c3f68` (visenum), LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`. The descriptor prototype now exposes own IDL accessors backed by the rule's declaration block. The focused Rust integration test directly calls the two alias accessors, rejects an invalid value and checks that `color` is not exposed on the prototype.

Six-job main run: +1 strict page (anchor-position IDL), +70 named assertions, no lost page, pass or observed name over 8,082 URLs. The seven focus URLs also cover rule mutation, allowed declarations, parsing, invalidation and page descriptor controls. LP 5,749 / 181,692, Chrome 6,950 / 182,963. Chrome-only 1,482 / 7,172; LP-only 281 / 5,901. LP had 11 timeouts and one hang, Chrome ten timeouts and one hang.

The 41-URL worker supplement was unchanged: LP 27 / 577 and Chrome 35 / 619; 39 withheld URLs remain unscored. The tryproto differences and outcomes preserve the exact sets. Ranking caveat for subsequent work: system-fonts.html passed all its local assertions; 104 names differ on each side because their generated labels contain platform-selected font metrics. They are retained in the directional sets, not counted as failed interpolations.
