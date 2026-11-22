# OPFS writable stream storage task

The immutable 98-page selection preserves the preceding OPFS publication. Nine URLs directly cover the remaining signed-only names in writable-file-stream write, isSameEntry and create-sync-access-handle; the other 89 are guards. The original 69 filesystem and 29 storage-control groups remain separately available. Every test URL has a signed Chrome stable 158.0.8127.37 observation from the pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The five signed filesystem page timeouts, three LP storage-control setup errors, names with timeout or not-run status and harness status are independent of strict page pass counts. No WPT source or expectation was changed.

The target assertions have three causes. Twelve pinned assertions expect DOMException SyntaxError when a command omits data, position or size. The current File System Standard requires TypeError in all three cases. Four tests remove an OPFS entry after saving its File, then expect reading that File during write to reject with NotFoundError. The getFile algorithm sets snapshot state and copies the file's underlying byte sequence. File API specifies NotFoundError when the read resource itself cannot be found and NotReadableError when a changed snapshot prevents a read. This in-memory backend retains a readable copy. Thirteen tests require handle serialization across messaging: three isSameEntry assertions in each of four globals and a window test that posts a file handle to a dedicated worker. Handle serialization remains outside this change. [File System Standard](https://fs.spec.whatwg.org/) · [File API read failures](https://w3c.github.io/FileAPI/#errorsAndExceptions)

The ordinary timing fixture separately runs on a window and a dedicated worker. For stream.write, a default writer's write, and a dictionary command containing a DataView, the caller mutates one byte after 0, 1, 4 or 32 additional Promise turns. Every case records whether the write settled before mutation and the byte actually committed after close. This tests ordering within a single microtask checkpoint without timing thresholds or a sleep. It also records the three missing-member errors and the readable/deleted snapshot result without translating divergences into passes.

## Commands and provenance

The parent LP is 4dc0af154, built from its exact source archive with the pinned CI JSC key 0e0dd7038a7a-6f4649e395e0 (static archive SHA-256 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029). Build both revisions with Cargo wptiter, inheriting release-fast, LTO disabled, 16 codegen units, incremental, features render and a single Cargo job. The WPT commands use the sibling cache layout in run.py.

    python3 run.py base
    node timing.cjs base ./base-pyro
    node timing.cjs chrome158
    python3 run.py candidate
    node timing.cjs candidate ./pyro
    python3 report.py
    python3 group.py
    python3 verify.py
    cargo test --locked --offline --profile wptiter -p pyro-browser --test integration storage_events::
    cargo test --locked --offline --profile wptiter -p pyro-webapi --lib opfs::store::tests
    cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration crypto_js::file_stream_writes_observe_queued_bytes_and_abort_preserves_the_file -- --exact

The no-native-retry WPT runner uses wptserve, HTTP/2, testdriver, generated globals and four page jobs. WPT named rows retain URL, original subtest name, one-based occurrence and base, candidate, signed Chrome statuses. Null means unobserved. Page outcomes and harness errors are recorded independently of named statuses. Each direct row is method, number of additional promise turns, settled-before-mutation flag and committed byte. The native regression runs the actual browser event loop and verifies deferred settlement, a one-byte DataView offset and the committed result.

## Result

The exact LP parent stayed pending on 12/24 direct rows and captured the changed byte on 6/24. At depths four and 32, every parent write had already settled. The feature stayed pending and captured the changed byte on all 24 rows. Signed Chrome stayed pending throughout but captured the old byte on all 24 rows, even with no intervening promise turn. The change follows the current storage-task copy step and thus differs from that observed Chrome capture behavior. It reuses the event loop's existing posting path for storage work. Byte consumption occurs in the posted task's microtask checkpoint; no dedicated scheduler task-source enum was added.

| Cohort | LP parent named passes | LP candidate named passes | Chrome stable named passes | LP / Chrome strict pages |
|---|---:|---:|---:|---:|
| 9 direct target URLs | 152/181 | 152/181 | 181/181 | 0 / 9 |
| 89 guards | 940/1,090 | 940/1,090 | 932/1,090 | 69 / 77 |
| All 69 OPFS URLs | 943/976 | 943/976 | 818/976 | 56 / 57 |
| All 29 storage-control URLs | 149/295 | 149/295 | 295/295 | 13 / 29 |

All 1,271 named observations and all 98 LP page/harness outcomes matched between builds, including setup messages. LP completed all nine target URLs. On the other 89 URLs LP had 86 completed pages and three pre-existing storage-control setup errors. Signed Chrome completed all nine targets, then 84 of the 89 guards; five filesystem pages timed out and emitted the partial results recorded in the original signed receipts. Neither LP run restarted or killed a browser.

The timing controls are new direct conformance evidence. They do not add WPT passes or recover any of the original 29 target occurrences. The unrecovered names are recorded individually in residuals.json. The pinned three absent-argument cases and deleted-snapshot observations were also independently reproduced directly in both Window and dedicated workers. Existing WPT and native test expectations were preserved.

Independent source review cleared 9b5eb6100. Independent runtime QA repeated the candidate WPT and timing fixture using the same immutable binary. All 1,271 named rows, 98 page rows, 29 classified residuals and both direct timing result records matched the owner's run exactly. Eight native controls passed, including the new real-BrowserThread timing regression, five backend/locking controls, the ordinary storage event guard and the existing queued writer/abort integration guard.
