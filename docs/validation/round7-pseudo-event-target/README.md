# Trusted pointer pseudo-target readback

Source `5f856d7ba033dd1f8558c2581403d102f66ab03c` starts at `c343ac093`. The measured baseline executable was built at `bc0cba7f5`, SHA-256 `dcab9b15353b18f5eacbfdb86daf6a74df5d4c92551867d2343050f992367ccc`. Only prior validation evidence differs between those source revisions. The equivalence receipt is included.

The renderer checks the existing Blitz paint hit against the DOM owner selected for trusted input. Recognized before, after and marker identities are captured before dispatch. The getter returns the corresponding CSSPseudoElement only while dispatch is active and the listener sees that originating owner. The click derives its pseudo-target from both the press and release. Synthetic events, ambiguous or mismatched hits, and post-dispatch reads have no pseudo-target.

Independent QA ran the three frozen WPT URLs with testdriver on pinned WPT a421f2fea. All six shadow-tree assertions gained Pass with zero losses. The other two assertions cover scroll markers: mutation during dispatch still yielded null, and the multi-event harness timed out on both executables. Signed Chrome 158.0.8127.37 passes all eight. Neither Pyro run restarted.

All five fresh direct event record sets on the candidate matched live signed Chrome, including closed and nested shadow boundaries, an ordinary owner surface, an owner before pseudo-element, distinct before/after surfaces on press and release, and retained post-dispatch reads. Two native guards passed in 0.07 seconds after a 70-second native test build. The one-job executable build took 92 seconds.

## Trusted input cost

Eight serial fresh processes ran in ABBA/BAAB order. Each process ran one warm-up and four measured samples per fixture. Each sample dispatched 96 trusted pointer moves over CDP, with one listener reading pseudoTarget per move. Median wall times include the 96 protocol round trips. CPU is mean process-tree CPU per sample.

| Fixture | Baseline median ms | Candidate median ms | Base / candidate CPU ms | Base / candidate non-null reads |
| --- | ---: | ---: | ---: | ---: |
| ordinary | 13.93 | 14.33 | 3.1 / 3.1 | 0 / 0 |
| pseudo | 13.23 | 13.97 | 3.8 / 2.5 | 0 / 1536 |
