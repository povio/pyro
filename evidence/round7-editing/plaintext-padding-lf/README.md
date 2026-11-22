# Typing on the final empty line of a plaintext editor

A block plaintext editor uses a preserved linefeed to render its last empty line. Typing on that line replaces the padding. The comparison checks a solitary linefeed and the padding left after an explicit line break. An inline editing host keeps its trailing separator.

## Provenance

The integration parent is fbbb896cd. Its executable was built from edc8ba24d. Only evidence was added between those revisions. The measured candidate is 44e431b5b. The JSC archive and candidate binary hashes are in provenance.txt. Parent and candidate WPT outputs are fresh. Chrome results reuse the signed 158.0.8127.37 designMode cohort on WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The direct Chrome fixtures were run separately.

## Named outcomes

| Cohort | Parent | Candidate | Signed Chrome |
| --- | ---: | ---: | ---: |
| Four plaintext insertText variants | 128/212 | 140/212 | 134/212 |
| 34 neighboring pages | 7,933/8,352 | 7,933/8,352 | 7,793/8,352 |

All 12 gains were previous failures that pass signed Chrome. The target retains 30 Pyro-only passes and has 24 remaining Chrome-only failures. The raw page reports and named ledger retain the original subtest names and occurrence numbers. All 38 page harnesses completed on each arm, with zero restarts or watchdog kills.

## Direct controls

Independent QA verified the candidate digest and compared the same 8,564 occurrences. Its separate 16-case fixture improved from 14 to 15 exact Chrome matches. All 16 original markup snapshots were recovered by undo. The remaining rich-editor difference predates this patch. See qa-readback.txt and the qa-*.json observations.

In the 36 owner controls, HTML agreement improved from 10 to 16, and caret agreement improved from 10 to 16. In the 12 split-node and Enter controls, markup agreement improved from 6 to 12. Exact caret agreement improved from 6 to 9. The Enter sequence still has differences in caret representation and undo grouping. Direct fixtures use the same Playwright CDP harness and locally archived launchers as the adjacent editing batches.

The original source review identified adjacent text nodes in inline editors as a separate case. The direct signed fixture established that inline hosts retain the separator while block hosts consume it. The final candidate was measured with both host types.
