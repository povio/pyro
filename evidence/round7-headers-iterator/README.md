# Headers iterator independent browser QA

Frozen 0a1f01b726b74617eba70c4b5529198826f6fc09 passed all 69 ordinary headers-basic assertions. Its exact parent c5afdca9ee664f7aa97887388b359ad121125a54 passed 60. Window, dedicated worker and shared worker each improved from 20/23 to 23/23. Each page completed with harness status OK on both exact builds and the signed Chrome reference. The fresh runs recorded zero restarts and watchdog kills.

In each global, “Check keys method”, “Check values method” and “Check entries method” changed from Fail to Pass. The other 60 passes held. All four observations had identical page/name membership. The candidate matched the signed reference on every status. The iterator checks cover prototype ancestry, next property flags, sorted yields, for-of traversal and repeated exhaustion. Default iteration, forEach and the four cases for mutation during iteration also passed in all three globals.

## Provenance

The MacBook build owner produced each immutable executable once using wptiter/render and pinned JSC. Independent QA verified the artifact, build log and provenance checksums, then ran each executable with run.py against WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The two build receipts record the source and tree, compiler, JSC and executable hashes. base.json and candidate.json are the fresh raw runs.

chrome158.json and observed-fada294a3.json retain the three selected raw page records from earlier independent four-page runs. Their originalReceipt fields identify the full source files and their hashes. Run-level metadata in these extracts describes the original four-page run. Chrome 158.0.8127.37 signing and executable identity are documented in signed-reference.txt and chrome-identity.json. observed-build.json identifies the earlier Pyro executable at fada294a39210c7e5552ea8cc977fe4f58a8b7e1. The exact parent replay reproduced all 69 earlier URL/name/status/message tuples and all three harness outcomes.

pages.json records completion and counts for each page. per-name.json records every page/name/status across the four observations. Run shasum -a 256 -c SHA256SUMS in this directory to verify the publication.
