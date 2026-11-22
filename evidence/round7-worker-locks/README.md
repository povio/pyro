# Web Locks across browser agents

The standalone RC.5 non-CSS census recorded 324 Chrome-only passing occurrences on 59 eligible Web Locks pages. This development comparison freezes every V2-eligible web-locks URL (86) and four adjacent AbortSignal guards. The exact 90-URL selection SHA-256 is c85dcd81a13ba9cedb389e9a83975fae9b31ed993731a5a21e22cdea18d786ca at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The matched reference is signed Google Chrome stable 158.0.8127.37, projected from 75 original shard receipts. Both LP arms use wptiter/render and the pinned static JSC archive. The parent was freshly built at f29218a2886f578160e163b4886d5197e002f97d.

The parent provided document lock queues and query results, but its broker and JS callbacks were both thread-local. WorkerNavigator did not expose locks. The new broker shares lock queue metadata across the agents of a browser context and storage key; it keeps JavaScript references local to the owning thread. Exclusive/shared grants preserve request order. A client processes grant and stolen notices in its event loop; posting and calling JavaScript take place outside the queue mutex. Teardown releases held requests and removes pending ones. Same-name locks in different origins, top-level sites, cross-site ancestor partitions or browser contexts have separate managers. The worker startup path passes the same service to dedicated, shared and service workers. Trusted-context exposure and existing origin checks still apply.

Nine direct controls cover a window and a worker contending for a lock, pending and held queries with distinct client IDs, later shared requests waiting behind an exclusive request, abort, steal, release after worker termination, context and storage partition boundaries, plus window callback settlement relative to steal. The flavor control checks top-level dedicated, shared, service and nested-worker paths independently. The nested worker fixture times out on both LP revisions. A separate parent/candidate/signed control identifies the missing prerequisite: Worker is undefined inside LP’s dedicated worker. Chrome exposes it. Dedicated, shared and service workers successfully contend with their owning window in the final direct run.

- https://w3c.github.io/web-locks/
- https://storage.spec.whatwg.org/#storage-buckets

## Commands

Use the cache paths recorded by run.py and signed-reference.py. Build each detached source with build-exact, wptiter/render, one compiler job and the original JSC archive identified in provenance.json. WPT ran with four page workers, generated globals, wptserve and no native retry.

    python3 signed-reference.py
    python3 run.py base
    ./candidate.sh "$FEATURE_COMMIT"
    node probe.cjs candidate ./pyro
    node queue-probe.cjs candidate ./pyro
    node context-probe.cjs candidate ./pyro
    node partition-probe.cjs candidate ./pyro
    node settlement.cjs candidate ./pyro
    node flavors.cjs candidate ./pyro
    node pending-exit.cjs candidate ./pyro
    node nested-surface.cjs candidate ./pyro
    node idl-probe.cjs candidate ./pyro
    python3 run.py candidate
    python3 report.py
    python3 classify.py
    python3 families.py
    ./native.sh

All nine direct controls have separately recorded reference/parent/candidate outputs. The raw WPT receipts, complete named and strict page ledgers, harness states and per-environment groupings preserve missing names and timeouts.

## Results

| Cohort | LP parent pass / observed | LP final pass / observed | Signed Chrome pass / observed | Strict pages parent / final / Chrome |
| --- | ---: | ---: | ---: | --- |
| 86 Web Locks URLs | 187/494 | 489/508 | 507/513 | 22 / 72 / 79 |
| 4 DOM AbortSignal guards | 60/60 | 60/60 | 60/60 | 4 / 4 / 4 |

The candidate added 302 named passes. It recovered 296 existing non-passes and six newly observed passes. All 302 had passed signed Chrome. Fourteen names became observable; no previous pass was lost and no baseline name disappeared. The even earlier RC.5 scan and this integrated parent are distinct binaries. The fresh parent had 322 signed-only target names; 20 remain. Two target names pass uniquely on LP. Across all 90 pages the parent recorded 72 complete and 18 timeouts, the final candidate 83 complete and seven timeouts, and signed Chrome 89 complete and one timeout. No arm restarted or recorded a watchdog kill.

The first broker executable at a51615099 gained 286 names. The additional Web IDL increment at d5edcc2cd corrected the length of each non-constructible interface object and the names of its attribute getters. The four assertions appear in each of Window, DedicatedWorker, SharedWorker and ServiceWorker. All 16 failed on the broker-only candidate and passed on the final candidate and signed Chrome; idl-readback.json retains the individual occurrences. The source keeps branded getters and non-constructible interfaces, and LockManager.prototype.request.length remains 2.

The remaining 20 signed-only occurrences include 16 BFCache scenario names, two dedicated-worker query assertions that need nested workers and two closed-window-with-worker assertions. The direct nested constructor control documents the preexisting absence of Worker inside a dedicated worker. Shared- and service-worker Web Locks families have zero remaining signed-only occurrences. All direct controls agree with signed Chrome except the two fixtures explicitly recording the missing nested worker constructor and resulting timeout.

Four broker native tests, four WebAPI lock tests and one real HTTP blob-worker termination test passed. The empty-thread cleanup guard also passed. The repository-wide thread-local inventory test remains red. Its missing entries are association::OWNERS, custom::CONSTRUCTIONS and structured_clone::WIRE_CHUNKS. The repository scanner's lexical comparison on the original integration tree, the broker-only revision, and the final revision finds precisely those three on each, and no discrepancy in Web Locks' own TLS list. check-tls-manifest.py and tls-manifest-readback.json reproduce that comparison from Git sources. The parent browser was not rebuilt to execute this separate full-inventory native test. The native logs preserve the actual test failure.

Hookless embedders share only with other local-only clients on their own thread for this service. Hosts with a remote task route, including ordinary browser windows and workers, participate in the shared cross-agent broker. The native admission guard verifies that posterless requests cannot wait for a callback on an unreachable thread. The inline grant guard checks that it does not register an abort hook after delivery, and that aborting its signal after grant does not change settlement or release. Existing queued grant controls exercise cancellation and transfer in normal hosts.

Of the 20 remaining signed-only passes, seven names were absent from LP's run. LP reported nine Optional Feature Unsupported, two Fail, one Timeout and one Not Run for the other 13. Full status sets are retained. Independent QA reproduced every final named occurrence and page outcome as well as all nine direct outputs. Its occurrence-by-occurrence readback of the 16 interface corrections is in qa/idl-deltas.json. Ten native checks passed and one failed, with the preexisting inventory discrepancy documented above.
