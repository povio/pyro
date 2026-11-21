# Combined round seven independent QA

Exact combined 1f1be032436bfeee9f48cdb5acbc0f71d21d5af9 versus frozen 305840f45. Fresh Pyro WPT runs for both binaries. WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Warm build used two Cargo jobs, wptiter/render and pinned JSC. See source, sha256 and input-sha256 for identity. Signed Google Chrome 158.0.8127.37 passed executable/framework hash and strict signature verification. Reused 41 census URL receipts after checking their original ledger hashes. Ran seven CSS URLs on Chrome. The receipts index records the originating file and hash for every URL.

| Selected group | URLs | Names | Frozen passes | Combined passes | Chrome passes | Gains | Losses |
|---|---:|---:|---:|---:|---:|---:|---:|
| Editing and selection |25|5032|3620|4800|4975|1180|0|
| CSS functions and invalidation |7|52|3|48|45|47|2|
| Listener, referrer and messaging |16|82|70|81|82|11|0|
| Total |48|5166|3693|4929|5102|1238|2|

All engines returned the same 5,166 named occurrences and 48 complete/OK harness outcomes. Fully passing URLs were 12, 35 and 40 respectively. Of the gains, 1,227 also pass Chrome. Combined has 33 Pyro-only passes and 206 Chrome-only passes. Both losses occur on function-parameter-scoping.html, where defaults reference a later parameter. Chrome fails both. One existing messaging guard still fails on combined, covering transmission when the current page becomes non-active.

Names are keyed by exact URL, canonical name and one-based occurrence. Each names.jsonl.gz row then lists frozen, combined and Chrome statuses. Page rows list status, harness status and milliseconds in the same order. Ports use original wptserve role maps. The comparison covered representative selectors. It makes no suite-wide performance or parity claim.

Warm build elapsed 165.45 s, user 833.61 s, system 20.40 s, queue wait zero. Shard wall seconds for frozen/combined were editing 2.824/1.877, CSS 0.368/0.213, and guards 41.460/1.391. Fresh Chrome CSS took 2.432 s. Each shard used one worker. Logs include process wall time including setup and per-shard CPU. These are single observations under shared load. Original Chrome census timings describe their original four-worker runs. No new WPT run restarted a browser or killed a watchdog.
