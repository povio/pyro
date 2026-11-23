# Undo and redo selection direction

Rich and plaintext hosts now save anchor and focus in their original order. At a text-node seam, a restored range start prefers the later node and its end the earlier node. Equal flat offsets restore a single caret. Text-control history keeps the range and direction before and after an edit, including the end of a coalesced typing group. A subsequent selection move therefore no longer replaces the caret redo should restore.

The optimized control path reads the UTF-16 length once for a replacement, clamps the proposed replacement span against that length and updates the value and caret together. Its post-edit history checkpoint reads the stored, already validated selection. A rejected readonly edit consequently leaves the selection intact.

## Direction and compatibility

* Independent QA on the host-direction change recovered backward undo in all 21 previously failing formatting scenarios. All 42 formatting and removal cases restored their original markup, selection text and direction. The parent restored markup and selected text but reversed every backward selection.
* On control-history revision `465c02ed3`, independent QA matched signed Chrome on the complete undo and redo snapshots in 36/36 moved-selection scenarios each. The no-listener subset improved from 6/18 to 18/18 for undo and from 0/18 to 18/18 for redo. Listener-induced selection moves split typing groups as in Chrome. All four keyboard cancellation controls retained their behavior.
* The builder repeated its 39 general, 21 seam/element, and 18 moved-control probes on both `465c02ed3` and the optimized tip. All 78 complete observation rows were identical between these two builds. Across the general set, the optimized tip matched signed Chrome on undo direction in 39/39 versus 26/39 on the exact parent, and on redo direction in 39/39 versus 24/39. Full snapshots also include markup, selected text and endpoint paths; the tip matched 35/39 on undo and 25/39 on redo.
* The flat-position model can normalize an element-offset selection to adjacent text endpoints during restoration. In the 21 additional seam probes, final undo direction matched Chrome in 19/21 and redo direction in 21/21. In the two remaining undo cases, deleting across a visually empty selection adjacent to an emoji differs in the underlying edit. Pyro restored a caret; Chrome restored a backward selection over the emoji. Rich-host HTML restoration can also replace DOM nodes. This change makes no claim of node-identity or complete redo-state parity.

## WPT and native checks

| Cohort | Exact Pyro names | Parent → optimized passes | Profile |
|---|---:|---:|---|
| 24 undo, typing and formatting pages | 18,523 | 16,492 → 16,492 | wptserve + testdriver |
| 26 text-control selection pages | 874 | 840 → 842 | wptserve + testdriver |
| 44 formatting-island guards | 33,316 | 29,402 → 29,402 | wptserve + testdriver |
| 27 control-command guards | 4,597 | 4,215 → 4,215 | wptserve + testdriver |
| 27 plaintext Enter guards | 2,852 | 2,603 → 2,603 | wptserve + testdriver |
| 54 static editing and DOM Range guards | 13,060 | 12,702 → 12,702 | static |

There were zero Pyro named losses or changes to Pyro membership in these cohorts. They overlap in coverage. The 24-page primary cohort had one fully passing page on both builds. The text-control cohort improved from 20 to 21 fully passing pages. Both of the gains were on `html/semantics/forms/textfieldselection/selection-readonly-keyboard.tentative.html`: “Typing and Backspace do not modify value in readonly input” and “Typing and Backspace do not modify value in readonly textarea”. Each failed on the parent because Backspace moved selectionStart and passed on signed Chrome and the optimized tip. The page improved from 3/5 to 5/5.

**Pre-existing harness error:** `selection/textcontrols/focus.html` still completed with harness status 1 and “The test never reaches the completion callback” on both Pyro builds. Signed Chrome completed it with status 0. All primary undo pages completed with status 0.

Seven exact native filters ran and passed: host anchor/focus and seam affinity, control undo/redo after an intervening move, locked inline formatting, formatting removal across host boundaries, formatting/structure commands, control/host undo and redo, and the DOM test for selection-direction ownership.

Independent QA on the exact optimized binary reran the 24 primary pages and confirmed all 18,523 statuses against the parent and benchmark comparator. It independently reran the readonly page (5/5, both gains signed Chrome passes), all 36 moved-selection/listener/coalescing observations (36/36 complete post-undo and redo snapshots match Chrome), and four keyboard cancellation controls. The remaining 25 text-control pages, the broader guard cohorts and timings retain builder provenance.

## Long textarea benchmark

The comparison isolates the selection-state cleanup. Both arms contain the validated control-history behavior. One fresh browser process per arm ran six workloads, using a fresh context for each. The textarea contained repeated `ab😀`. Each workload performed eight untimed warmup commands, reset the original value and selection, then ran 64 scripted insertText or backward-delete commands. Each recorded batch validated the command count, resulting value, length and both caret offsets. The ten arm processes ran serially as five alternating pairs (AB, BA, AB, BA, AB).

| Initial UTF-16 units | Command | 465c02ed3 median ms | dc68a019e median ms | Elapsed reduction |
|---:|---|---:|---:|---:|
| 4,096 | insertText | 1.5 | 1.0 | 33.3% |
| 4,096 | delete | 2.6 | 1.4 | 46.2% |
| 32,768 | insertText | 12.0 | 7.4 | 38.3% |
| 32,768 | delete | 19.8 | 10.2 | 48.5% |
| 262,144 | insertText | 91.5 | 58.1 | 36.5% |
| 262,144 | delete | 157.9 | 80.8 | 48.8% |

Across all five repeats and six workloads per arm, timed command batches totaled 1.4347 s on the comparator and 0.7978 s on the optimized tip. Whole-process elapsed totals including startup, preparation and warmup were 102.094 s and 100.645 s respectively. The measurement window was 21:16:12.605–21:19:35.366 UTC on 2026-11-21. Average whole-host CPU was 34.00%. The comparator and optimized timed-operation windows averaged 34.74% and 32.66% respectively. The co-running CI worker was active in all nine nearby snapshots. Local compiler work was held during the window. These figures describe the measured textarea workload.

The operation receipts include each start/end in UTC, elapsed milliseconds, validation results and whole-arm markers. The occupancy receipt includes precise per-arm boundaries and one-second CPU samples. To reproduce one arm with Playwright Core available in NODE_PATH:

```sh
node long-text-benchmark.cjs candidate /absolute/path/to/pyro
```

## Immutable artifacts

| Role | Source SHA | Executable SHA-256 |
|---|---|---|
| Exact parent | c6600a40c066b59e903940ba7a2257a98bf2ca3c | c39c3d56b5bad0e02d1f4cbc69cf88c73f2ea3a7bd37fe9a5a4ffe61ae249180 |
| Host-direction intermediate | c37dbb2355f3d6a672dfec2984f87f75ac5508a6 | dd54eb5aff60023ec321457b44accfcaf9b30b0bb2ca7e6aab70c9841d5708ee |
| Control-history benchmark comparator | 465c02ed3194217dae7e1246ed0e7b1910de4c9e | 4f70bfa42d580032559bb630f624fddf6b21df1f70b1c2a6478b80ba689aa15e |
| Optimized tip | dc68a019e825ce87722224476307c94869c8e025 | 4f3fd305e86671fb15147e1e6f1448d7805ccc9b8df751079f65a30316e92c60 |

All four binaries used clean source in the private wptiter renderer target, one Cargo job, rustc 1.100.0, incremental compilation, 16 codegen units, LTO disabled and pinned JSC archive SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. The optimized source tree is `04e0bcc5a2d3975ad8ca6c3417401d052c0a749b`.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Parent and signed Chrome 158.0.8127.37 were measured on the 24 primary pages, with those receipts reused across revisions. The 26 text-control pages ran on the exact parent, optimized binary and signed Chrome with one worker. Both exact Pyro builds ran each guard cohort; its Chrome column reuses the preceding matching-profile signed Chrome receipts. Ledger rows give URL, subtest, occurrence, parent, optimized and Chrome status. Empty statuses identify names absent on that browser.

```sh
gzip -dc primary.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events,selection --server wptserve \
  --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
```
