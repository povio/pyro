# Loaded import rule associations

Selection: Chrome-only named passes in batch9 (`5503e80ece0f148851478a65fdd36de7d10be500`). All seven focused URLs are completed entries in the frozen main safe manifest, outside the withheld list. Parent `b7c2c22943dee3c7390c1d29484052e3af996e32` differs from its tested executable's source `255eb2f4d8aa5953a5b9403e17daf86cba48ff90` only by evidence. Candidate `fcca8ecddd13574197e9dd6777c732a1cb1e3a16`. WPT `a421f2fea5eb`, stable Chrome `158.0.8127.37`, patched JSC `0e0dd7038a7a-6f4649e395e0`.

| Run | Fully passing pages | Named passes | WPT wall |
| --- | ---: | ---: | ---: |
| Parent | 4/7 | 29/32 | 194 ms |
| Candidate | 6/7 | 31/32 | 122 ms |
| Chrome | 7/7 | 32/32 | 709 ms |

Two page and named gains: `CSSImportRule-media-identity.html` (the rule and its sheet share a media list), and `cssimportrule-parent.html` (deleting a loaded import unlinks the child sheet). No losses. `CSSImportRule-insertRule-styleSheet-parentStyleSheet.html` remains failing: the synchronously inspected, newly inserted import has no sheet yet. No loader or request policy changes.

All three runs have identical URL/name/occurrence inventories, seven completed harnesses and zero restarts or watchdog kills. Three workers, isolated ports, sequential commands. JSONs are exact runner results. Native browser integration: two existing controls pass on both; the new combined quoted-URL, media forwarding and removal test fails on the parent and passes on the candidate (2/3 to 3/3). Native command elapsed times including compilation: 53.92 s / 27.25 s. Candidate binary build: 29.23 s.
