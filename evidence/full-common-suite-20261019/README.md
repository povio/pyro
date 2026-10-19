# Frozen full common-suite result

The completed frozen run passed **729 of 742 common cases; 13 failed**, on engine `fb3b12dc65886e8271629dcf541294f2df725c47` and corrected TOS `e6cf6953f456a82df30e037620afb314a99abc85`. Each case ran once, with zero skips or retries. The 116 nativeOnly cases remain separate. Total parallel-lane elapsed time was **22 minutes 52.745 seconds**; this includes harness/setup/cleanup and is not a browser performance benchmark.

See the [completed result and provenance](final-run/README.md), [all 742 case attempts](final-case-ledger.json), [13 failures](final-run/failures.json), and [disjoint repair ownership](repair-ownership.json). Subsequent targeted repairs do not replace these frozen outcomes; a new complete common-candidate run is still required before claiming green.

The `not_run` statuses in [w2-final-selection-hashes.json](w2-final-selection-hashes.json) are the **pre-execution selection snapshot**, retained unchanged. Current frozen outcomes are in the completed ledger above. The planning text below is also historical, including its original pending statements and earlier engine pin.

Exit-code layers: `final-run/w2-lane-state.json` records `runner_exit` from the outer `run-selection.sh` cleanup/reporting wrapper. That wrapper does not propagate the Playwright harness failure exit, so its zero is not a test pass. The actual harness `exit_code` is preserved in each `w2-final-NN/metadata.json` (for example [batch 01](w2-final-01/metadata.json)); case outcomes, not wrapper success, determine the 729/13 result. Raw fields remain unchanged.

# Historical coordination snapshot (before freeze)

Canonical original TOS f9fde4aa0dce7d1d7a1ffebffe3306399973cd80 manifest SHA542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19 contains742 common cases in141 files and116 nativeOnly cases. All742 final results are currently **not run**. Historical or focused passing cases do not populate final results.

W3 confirmed ownership of its historical39 whole files/275 common cases; W2 owns the disjoint remaining102 files/467 cases. The union/dedup/whole-file checks are machine verified. NativeOnly remains explicitly separate, never counted as common passes or silently removed from a denominator. W1 calendar repair execution belongs W2; W3 is the independent TOS drift repair owner and coordinates shared-file changes with W1.

Initial engine pin5b467880d75c3e14a0b0108775de25f182d39734. Original application/test source remains f9fde4aa. Legitimate TOS repairs are separately branched/reviewed; final common-suite runs must pin an agreed final engine and TOS commit. Early current-main bounded repair checks are not a final aggregate. Retries, skips, flakes, setup errors and unsupported behavior stay explicit. No assertion weakening or silent case exclusions.

Historical W2-20..32 report recovery lists224 most-recent observed case records across multiple old engine heads, not current-main results. Its six nonpasses include three previews and HBL assignment with later exact targeted pass evidence, plus calendar persistence (W1 owner) and position exclude-from-statistics requiring fresh attribution. Earlier W2 batches1..19 remain retained separately; this historical subset is not the full467-case ledger. W3's TOS_W3_FAILURE_LEDGER.md supplies its historical drift/setup findings; route geometry and native frame focus subsequently merged and must be revalidated, not relabelled passed.

NativePG services/credentials remain unchanged. New UTC2026-10-19 forces template rebuild under the existing input/date ledger; run DBs use fresh clones and exact run-owned cleanup. W3 retains its own host/PG mode. No old binaries, evidence or fixtures are discarded. Full suite green can be claimed only after all intended common cases have final consistent-candidate results.
