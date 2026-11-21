# Window listener ownership across realm teardown

A child browsing context can reuse its initial about:blank Window on its first same-origin navigation. Each activated world has its own Window wrapper for the same native event target. Adoption transfers that target to the replacement document. Previously, if an old isolated world released its wrapper later, its destructor asked the old document whether the key still belonged to a Window. The old document had transferred the key, so the destructor removed the active document’s listener list.

The fix records whether a wrapper owns native listener cleanup when the wrapper is constructed. Document-owned Window targets remain shared across worlds, nodes keep their existing lifetime, and standalone EventTargets still clean up their lists. The deterministic test uses two actual JSC binding realms and the production adopt, hand-over and rebind operations. It verifies disposal of the isolated realm before posting an ordinary message. The identical test on the exact parent failed with an empty message list. The candidate delivered the expected message. Independent QA reviewed the source and both native receipts.

## Matched observations

| Selection | Parent / candidate / Chrome named passes | Parent / candidate / Chrome full pages | Candidate gains / losses |
|---|---|---|---|
| Four historical iframe referrer URLs | 35 / 48 / 48 of 48 | 0 / 4 / 4 | +13 / 0 |
| Twelve ordinary messaging and event guards | 33 / 33 / 34 of 34 | 11 / 11 / 12 | 0 / 0 |

Both LP binaries completed all 16 URLs with an OK harness, zero restarts and zero watchdog terminations. No subtest names were added or missing. Each focus URL took 10.1–30.2 seconds on the parent and 0.23–0.28 seconds on the candidate. The remaining guard failure on both LP builds is “Making the current page become non-active must prevent message transmission” on webmessaging/multi-globals/messageport-current.html.

These counts describe one uninstrumented matched pair. Historical runs varied. The four URLs were preselected from the old diagnostic receipt; the adjacent guard list uses previously completed ordinary cases. Five additionally considered URLs without matched historical receipts were withheld. Neither historical results nor test expectations were modified. No inference is made about other referrer losses, full-suite gains or faults from these selections.

## Diagnostic evidence

The isolated instrumentation ran at 81c195367a245cd91177e28330fef28f264b660b on exactly the four historical URLs, one worker, before the ownership fix. It observed 39/48 passing names. Four 9.779–9.934 second gaps on three URLs ended when an older iframe sent its zero-test timeout completion. The expected echo’s UUID reached native postMessage entry, serialization, queueing, matching-loader dequeue and event dispatch in every case. Passive parent listeners recorded no matching result, and the engine reported no uncaught exception. The independent deterministic test establishes an ownership bug that removes those listeners. The traces did not instrument the destructor, so they cannot attribute every historical failure to it. The attached native instrumentation is diagnostic only and is absent from the production branch.

The diagnostic directory retains the original round-six passive archive at 6fa5684c, the new raw native and passive logs, all four pages’ subtest outcomes and the UUID-linked analysis. Its observer installs additional passive listeners. Measurements on the traced executable are separate from the matched unmodified-runner scores above.

## Versions and commands

- Production source 7b43a5de838c5a3cb9d6bb4cf61ae79a9991a045, parent 213f2a3cc429772c5bf6b79ce1032a3bc876b6c6. The candidate browser was built at local development commit 5decd721efac628058d4c8366e3fb389d30ed39f. The production source and build inputs match the final topic exactly; only regression-test files differed. The test executed from the final source tree and from a test-only patch on the exact parent.
- Both LP builds used CI JSC 0e0dd7038a7a-6f4649e395e0, archive SHA-256 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. Parent browser SHA-256 f7f27bd0336cc28fdf2edd4201946e982ba1ae2c3bd90e6dc944afafa2626301. Candidate SHA-256 4518fd3fd73fedc353e646bafad9ae3e7b57b682bd664ef546906d1a741a9b44.
- WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Both fresh LP measurements used one worker and fresh browser contexts. Chrome columns reuse the corresponding frozen signed macOS arm64 stable 158.0.8127.37 receipts. The Chrome export includes the source filename, browser launcher and verified SHA-256 for every URL; Chrome timings have separate run provenance.

The browser builds used the pinned JSC path and matching patch inputs with one Cargo job and the workspace wptiter profile (release-fast, no LTO, 16 codegen units, incremental). The following commands run the deterministic test and matched selector. For the base native test only, apply deterministic-test.patch to the pristine parent after building its browser.

```sh
CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false \
  CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true \
  cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration \
  dom_js::retiring_an_isolated_world_preserves_the_reused_windows_message_listener -- --exact
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 1 --slow-jobs 0 \
  --tests selection.json --bin "lp=$BINARY" --out "$LABEL.json"
python3 report.py
```

The named tables store URL, exact name, occurrence and the three statuses in parent/candidate/Chrome order. Page rows retain harness status, completion status, whole-page pass and milliseconds for each browser. Full-page pass requires a completed OK harness and at least one subtest, all passing.
