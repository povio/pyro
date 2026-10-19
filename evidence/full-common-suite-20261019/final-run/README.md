# Frozen common-suite result

All **742 common cases in141 files** ran once: **729 passed,13 failed,0 skipped,0 retries**. The separate116 nativeOnly cases were not part of this selected browser suite. This is not an aggregate green result.

- Engine: `fb3b12dc65886e8271629dcf541294f2df725c47`.
- Corrected TOS: `e6cf6953f456a82df30e037620afb314a99abc85`; reviewed calendar readiness, UI/fixture drift and separate AddParty scroll-readiness corrections are included. Original `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80` fixtures/results remain preserved; no develop merge.
- Manifest SHA256: `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`, unchanged. Exact file/title identities: W2 467 in102 files, W3 275 in39 files; empty intersection and exact742 union.
- Total elapsed: **22 minutes52.745seconds** (`1372.745s`), from first lane start `2026-10-19T04:03:07.292020Z` to last lane end `04:26:00.037020Z`. Parallel host lane durations are not summed. W2 took973.905s; W3 took888.141s. This is functional-suite execution time including per-batch harness/setup/cleanup, not a browser performance benchmark.
- W2:458/467, W3:271/275. Every completed batch restored its expected database state; no run-owned databases remain. W2's explicit same-UTC-date template ledger and W3's preserved preexisting database set remain retained. No hidden replacement runs or setup retries.

`../final-case-ledger.json` contains all742 identities and attempts; `failures.json` lists all13 failures. `SUMMARY.json`, per-batch records and W3 source evidence provide durations/provenance. Both hosts built locked release from exact final engine and verified JIT signing. W2's exact-final invocation reused production-identical7f7 compiled artifacts, explicitly disclosed in its build provenance; no old source pin was silently relabelled. Polling traps enabled,64MiB shared and64MiB private cache. Backend-token localStorage authentication and documented direct-lane version gate bypass remain unchanged.

Remaining failures: chat retry2; shipment tab1; Position close/reopen2; Office badge text1; shipping-event sort HTTP5001; platform/To-Do canvas-only table expectations2; W3 quote-party autocomplete2; split-route readonly activation2. Source-proven test drift is separated from unclassified native/application behavior; fixes and later validation are not substituted for these frozen outcomes.

Artifact limitation: W2 batches1/2 default error-context files were removed at the next runner start; JSON assertion failures remain. Batch3 passed, batch4 attachments were copied, and batch5+ use per-run external artifact paths. This output-only adjustment changed no tests/auth/assertions/engine/manifest. No rerun was made to replace lost attachments.

Separate release workspace validation failed in the native auth-cancellation test (`reply.is_closed`); this is not counted as a TOS case and no full-workspace green claim is made. A separate owned regression/repair is pending.
