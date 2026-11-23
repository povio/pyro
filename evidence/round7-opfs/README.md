# Origin-private file system

This topic targets 69 ordinary OPFS URLs and retains 29 existing storage manager/estimate/IDL controls. All 98 were frozen before source edits. OPFS tests requiring IndexedDB handle cloning, buckets, observers, BFCache or messaging serialization are outside the focused selection. Existing storage guards can exercise estimate reporting for other endpoints; no IndexedDB implementation changes were made. The tested globals include window, dedicated worker, shared worker and service worker variants identified in the source map. No WPT expectation or test was changed.

The frozen census ranked 799 Chrome-only names for this selection, compared with 505 in the Cookie Store suite and 56 SVG length-list names. The exact parent confirms the shared cause: 797 named failures explicitly mention the missing getDirectory method, 157 identify filesystem interfaces and three fail looking up a member of an absent interface. All 976 filesystem names were observed on the parent and signed reference.

Storage is in memory, shared by the origin's window and workers within a browser context, matching the lifecycle of this browser's other ephemeral storage. The namespace uses logical locators and never reads or writes host filesystem paths. Closing the browser context releases its storage. Each origin has a 64 MiB OPFS budget covering committed bytes and staged writer data. StorageManager.estimate() continues to report the browser's existing overall 10 GiB estimate and adds committed OPFS bytes to usage. This endpoint does not promise persistence between browser processes.

## Reproduction

Both exact source trees use the pinned CI JSC archive and the same wptiter build profile, with render enabled. The signed Chrome 158.0.8127.37 WPT rows reuse 79 original four-job shards from publication ee24e07f6. The signed URL map, source blobs, environments, WPT revision and input hashes are included. All 98 URLs have signed observations. Signed per-selection restart/watchdog totals are unavailable. Use the sibling cache layout in run.py and import-reference.py, placing baseline and candidate executables at base-pyro and pyro.

    python3 import-reference.py
    python3 run.py base
    node probe.cjs base ./base-pyro
    node probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    python3 report.py
    python3 ledgers.py
    python3 changed.py
    python3 details.py
    ./native.sh

named.jsonl.gz rows retain URL, exact subtest name, one-based occurrence and the statuses of base, candidate, signed Chrome. Null means unobserved. pages.json retains page outcome, harness status and strict-pass flag for all engines. A strict pass completes with an OK harness and a nonempty set of all passing names. fs and storage evidence are split under opfs-* and storage-*; scope-scores.json includes the original error details for each non-OK page. No signed Chrome WPT rerun is attributed to this batch.

The signed Chrome filesystem timeouts were retained with their observed named results: getFileHandle in a shared worker (11/13 passing), cross-primitive locking in a dedicated worker (12/90), file move in a service worker (6/24) and a shared worker (15/24), and writable-file-stream in a window (5/9). The remaining 64 filesystem pages completed. The three baseline storage-control setup errors are independent of the missing OPFS interfaces and are listed in scope-scores.json. All 29 signed storage guards completed.

The direct fixture verifies that a writer publishes data on close, abort preserves the committed contents, a worker reads window-written bytes, sync cursor reads/writes modify the same file, reload preserves contents, and another origin or browser context sees its own empty root. Supplemental cases borrow APIs from a same-origin iframe, inspect MIME types and stale handles, and enqueue a mutable byte array behind a prior write. Chrome persisted [1,9] after the caller changed the second byte before the sink consumed it. Existing browser data storage is deliberately ephemeral: none of the fixtures requests access to the host filesystem.

## Results

| Group | LP base | LP feature | Chrome stable 158.0.8127.37 |
|---|---:|---:|---:|
| 69 OPFS pages, named passes / 976 | 19 | 943 | 818 |
| Strict OPFS pages | 0 | 56 | 57 |
| 29 storage controls, named passes / 295 | 149 | 149 | 295 |
| Strict storage controls | 13 | 13 | 29 |

All 924 gains were previously observed failures. No baseline passing or observed name was lost. Of the gains Chrome recorded 770 Pass, 51 Fail, 5 Timeout and 98 Not Run. The OPFS pass sets have 154 LP-only and 29 Chrome-only names. The combined 98-URL selection has 1,092 LP passes and 1,113 Chrome passes. Both LP builds completed all 69 OPFS pages with OK harness status. Every storage guard retained its original named and page outcomes: 26 OK and three setup errors on partitioned storage estimates for caches, IndexedDB and service workers. Both LP runs recorded zero process restarts and watchdog kills.

The first 815aadea3 candidate gained 885 names with zero losses. Its normal fresh-file probe exposed an observable gap when opening the source while move or removal was pending: it returned NotFoundError after immediate removal, whereas signed Chrome returned NoModificationAllowedError during the pending mutation. The final source reserves the source, destination and affected subtree before scheduling the native mutation, commits atomically, and releases all reservations on cancellation. All seven independent lifecycle controls match Chrome. On the pinned cross-primitive worker page, all 90 names passed, compared with 64 on the precursor; signed stable timed out on that page. Thirteen worker IDL names also recovered. The pinned runtime exposes filesystem interface objects as writable, configurable, non-enumerable data properties on the worker global without constructing the OPFS module on every worker startup. The direct descriptor check also records two unrelated worker globals for context; they are outside this implementation.

Thirty-three OPFS names still fail on LP. Twelve command validation assertions expect DOMException SyntaxError for omitted write data, truncate size or seek position; the current filesystem specification requires TypeError. Four expect a File snapshot obtained before removal to become unreadable; the implementation keeps the copied bytes. Thirteen require handle serialization across messaging, outside this batch. Four fail identically on both engines because the pinned writable-stream helper is called with an incompatible argument list. The full ledger preserves all outcomes, including shared failures. [File System Standard](https://fs.spec.whatwg.org/)

The normal cross-realm probe retains two signed divergences. The candidate rejects iterator advancement with NotFoundError after the directory is removed, and the old File snapshot still reads `oX`. Signed Chrome delivered another cached iteration result and rejected the old snapshot with NotFoundError. Byte sequences, worker cursor, abort, reload, file types, same-origin borrowed methods and context/origin isolation otherwise match. The separate locks receipt additionally checks the actual descriptor before first property access and verifies targets remain usable after reservations release.

Independent runtime QA repeated all 98 URLs on the measured 3c9fb8ea0 binary (SHA-256 f26d36b5ba152cb597a57f0489f755ee8f4953210a88974fbbd71bb51b5f0724). The complete per-occurrence name ledgers, both sets of page tuples, primary I/O and worker lock/descriptor results match the owner run exactly. Its separate candidate receipt digest is in qa/summary.json.

Five native backend tests, the queued-stream/rollback integration guard, the standard dedicated-worker compatibility guard and the fresh-worker descriptor guard passed on the measured production source. Its preexisting native StorageManager guard failed solely because its hardcoded method list omitted getDirectory. The exact baseline lists estimate,persisted,persist; both signed stable and the new product list estimate,persisted,getDirectory,persist. Release commit 0cddcec2c changes only that native expected string, retains all other assertion values, and passes the guard. All nine focused native tests are green with that correction. native-results.txt preserves the original red and subsequent green separately. Product and WPT source files are byte-identical between 3c9fb8ea0 and the release tip. Independent review cleared both the product changes and this test-only delta.
