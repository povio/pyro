# Performance and memory: findings and opportunities

Every item is labeled with its evidence class:

- **MEASURED-HERE**: measured in this repository. Commit, method and platform are given.
- **UPSTREAM-HISTORICAL**: figures quoted from upstream Lightpanda commit messages. All the cited commits are ancestors
  of our pinned reference `2ad38773`. We have not reproduced them, so they are *not* a baseline for that commit.
- **HYPOTHESIS**: derived from reading upstream source. Not measured.

None of the MEASURED-HERE numbers below is a browser E2E benchmark. TOS E2E baselines and matched comparisons are
owned by the automation worker (`bench/`, `docs/automation/`).

Host for MEASURED-HERE: macOS 27.0.1 (26A434) arm64, system JavaScriptCore 22625.1.29.11.28, rustc 1.99.1, release
profile (`lto = "thin"`, `codegen-units = 1`). CPU frequency scaling was not controlled, and every figure is a single
run unless stated.

## Measured here

| What | Value | Method |
|---|---|---|
| `lightningpanda` binary size (eval-only CLI, `d603683`) | 738 KB | `ls -l target/release/lightningpanda` |
| `lightningpanda eval -e 1` | <10 ms wall, 10.3 MB max RSS, 4.9 MB peak footprint | `/usr/bin/time -l`, `d603683` |
| 1,000 chained `setTimeout(0)` | 1.9 ms | `eval` script using `__lpNow`. No HTML 4 ms nesting clamp, a known deviation |
| Realm creation with a synthetic 300-interface × 30-member registry | 9.6–10.3 ms per realm (mean of 20), registration 0.38–0.44 ms; 20 live realms → 78.7 MB peak footprint | `cargo run --release -p lp-js --features bench --example realm_bench`, `a89b281`. Synthetic: real interface counts and shapes will differ |
| Playwright 1.59.1 `connectOverCDP` → `page.goto` of a 4-script local page | 5.9–6.2 ms (two runs) | **smoke only.** `integration/runtime-dom` `be9bf02`, local Node static server, no DOM bindings yet |
| Style: full restyle of a 6,001-element page with 1,500 rules (`bench/scripts/css-bench.mjs`, flow geometry, release, M1) | 98.6 ms → 23.8 ms per restyle (18.7 ms of it is the read floor with one rule: JS wrapper per read, color serialization, cache teardown); first layout pass after styling 83 ms → 22 ms; appending one row under `.list > .item:nth-child(2n)` rules then re-reading every style 98.6 ms → 10.3 ms; cached reads of all 6,001 styles 10 ms | Attribute- and attribute-value-keyed rule buckets, direction-aware `:nth-*` sibling invalidation, ancestor Bloom filter outside layout passes, one matching pass for the typed and generic cascades, an ancestor-class secondary index for `.ctx tag` / `.ctx .x` rules |
| Style sharing on a real page: the TOS Positions list captured from a running FE E2E stack (`bench/scripts/tos-capture-positions.mjs`), its 16 rows cycled to 200 and 1,000 rows (`bench/scripts/tos-positions-bench.mjs`; 12k and 52k elements, 3,189 Tailwind rules, flow geometry) | full restyle with every style re-read: 200 rows 540 ms → 284 ms, 1,000 rows 2,387 ms → 1,190 ms; first layout pass 688 ms → 464 ms and 3,757 ms → 2,677 ms; cached reads unchanged (39 ms / 174 ms) | Elements share the declared map and typed cascade of a previous sibling or same-position cousin under the flow-layout sharing predicate (`shares_cascade`), now also for SVG elements, with revalidation results memoized and revalidation selectors indexed by bucket. Rows hold `a` links, buttons and checkboxes, which never share; those and the read floor are what remains |
| First flow pass of the in-crate app-shaped page (`crates/lp-dom/tests/flow_bench.rs`: sticky header, sidebar, a table whose rows hold a checkbox, an input, two buttons, an SVG icon and badges; Tailwind-like rules; release, M-series Mac, 3 runs) | 1,000 rows (20k boxes): ~145 ms → 62–65 ms; 200 rows: ~12.5 ms. Container callgrind: 1.54G → 0.64G instructions | Style sharing now reaches table rows: ids no selector names no longer block it, each element also tries the last element styled under a parent of the same share class with the same tag and class attribute, ancestors are styled first, and inputs and buttons share when every state pseudo-class some selector tests agrees. `::after` styles are skipped unless a matching rule sets `content`. Rows whose cells lay out alike at any height are then laid out once, at their content heights, and sized to the row instead of after a height probe: 0.64G → 0.58G instructions, ~76 ms → ~73 ms on the Mac. Less than the probe's share suggested, because the probe's measurements of flex and inline-block contents were already reused by the final layout |
| Playwright `locator(css).textContent()` on a 2,000-row page (`bench/scripts/cdp-perf.mjs` `pw.locator.textContent.2k`, release, M1 Max, 3 rounds; Playwright's selector engine makes about 44,000 DOM calls per locator: `parentElement` x18k, `NodeList.length` x8k, `shadowRoot` x6k, `parentNode` x4k, `matches` x2k) | 11.1 ms -> 5.4 ms per call, browser CPU 12.2 -> 6.6 ms (upstream Lightpanda 5.2 / 5.2, Chrome 147 headless shell 2.9 / 0.2); `pw.flow.e2e` 22.5 -> 17.6 ms, `dom.mutate.1k` 2.3 -> 1.9 ms, `page.navigate.scripts` 5.0 -> 4.3 ms | JSC patch `0012`: attributes as custom accessors (inline caches call the getter directly), operations as internal functions with data (call inline caches link them), indexed callbacks with numeric indices and cacheable collection wrappers, string arguments read without a copy; a per-thread realm cache, fewer thread-local reads and one class-chain walk per callback; an O(1) mutation-observer pending check. Playwright's injected engine itself is left as shipped |
| DOM calls from script, browser CPU per call (`ps` CPU time of the browser around an in-page loop over 2,000 elements, so other processes' load does not count; release, M1 Max) and Playwright's selector engine run in the page (`injected.querySelectorAll('#iN span')` on the 2,000-row page, 300 queries, no CDP) | `nodeType` 45 -> 17.5 ns, `parentElement` 72 -> 32 ns, `shadowRoot` 42 -> 20 ns, `nodeName` 210 -> 45 ns, `hasAttribute` 212 -> 40 ns, `getAttribute` 227 -> 40 ns, `matches('#id')` 115 -> 87 ns, `NodeList.length` 35 -> 10 ns, `list[i]` 65 -> 25 ns, `for...of` over a NodeList 95 -> 45 ns per element; the engine query 6.3 -> 4.1 ms of browser CPU (13.9 ms before the fast paths above), 3.5 to 3.8 ms with `--prepare`. Wall clock, best of 7, against Chrome 147 on the same machine: `parentElement` 25 vs 8 ns, `nodeType` 15 vs 4, `list[i]` 25 vs 16, `getAttribute` 32 vs 24, `matches` 70 vs 28; the engine query 2.05 vs 1.8 ms (2.9 before). End to end, `pw.locator.textContent.2k` (load average about 25, four runs): 4.2 to 5.3 ms, 3.6 to 4.5 ms with `--prepare`, against Chrome's 3.7 to 4.0 ms in the same runs and upstream Lightpanda's 6.2 ms. A getter that returns a constant costs 7.5 ns through the JIT path, loop included: what is left above Chrome is `lp-js` dispatch, the DOM borrow and the wrapper identity lookup | Pure members (`pure_getter`, `pure_method`, indexed getters) take a lean dispatch: scope from the realm the receiver's payload keeps, data from the payload's class chain, no registry, no thread-local state, no post-callback hook; integers, booleans and null are made as immediates; ASCII attribute-name arguments are borrowed, not copied; `tagName`/`nodeName` return one interned JavaScript string per name per realm; a node keeps its own wrapper in a per-page table by node index, alive while the node is connected through a keep byte the collector reads (JSC patch `0014`, `JSWeakCreateKeptWhile`), so finding it is a load and a script that walks the document does not have its wrappers collected and remade: `parentElement` 30 -> 27.5 ns and the engine query 4.25 -> 4.05 ms against the identity map and pin map it replaces (and each document counts its referenced nodes, so the DOM's garbage pass does not walk a held document: navigating away from a page with thousands of wrappers took 36 s without that count). JSC patch `0013`: pure getters as DOM attributes the JITs call directly (`JSObjectSetFastGetter`), `wrapper[i]` as an inline-cache case, and the API callback scope's collector check in debug builds only (two thread lookups per indexed read or method call, 9% of the query). `--prepare` lets the heap grow 4 MB between collections instead of the C API's 1 MB (`lp_js::prefer_throughput`): collections were 20% of the query; it costs a loaded mid-size SPA 14 MB of RSS, so it is not the default |
| Memory of one Playwright page with the ToS FE sign-in SPA (`vmmap`/`footprint`, release, M1 Max): physical footprint at idle, with a blank page, with the SPA loaded, 2.5 s after closing its context | 14 -> 2 MB idle (no spare engine or render prewarm unless `--spare-engine`/`--render-prewarm` or `--prepare`), 23 -> 14 MB blank, 106 -> 82 MB loaded, 97 -> 38 MB closed (upstream Lightpanda: 7 / 10 / 57 / 34 MB). `ps` RSS: 165 -> 127 MB loaded, 165 -> 83 MB closed (upstream 89 / 68); what remains above footprint is clean code pages (our binary and the patched JSC dylib, about 20 MB) and the free pages inside the system allocator's partly used regions, which it only `madvise`s | ASCII script sources kept as 8-bit JSC strings; the idle collection (1 s without a command) drops compiled and unlinked code and runs the libpas scavenger, and a close-time collection re-arms it; libpas returns freed pages by remapping them, which takes them out of the resident set |

## Upstream historical (quoted, not reproduced)

| Commit | Claim (from the commit message) | Platform / method stated |
|---|---|---|
| `87a69181e` (2026-10-12) | `malloc_trim(0)` per closed connection. Idle anonymous memory from the first to the last fifth of a run: 300 sequential connections 14.5→67.8 MiB without, 6.9→7.6 MiB with; 8 concurrent clients over 400 connections 121→419 MiB without, 16.6→18.3 MiB with. About 1% more CPU per page | x86_64 glibc, synthetic growing pages, one CDP connection per page; "aarch64 shows the same" |
| `c3f1314fe` (2026-09-24) | Slab allocations are rarely freed: 0.1% (HN) to 9.5% (youtube) of allocations per site. The replacement recycler takes 17 ns/op against 310 ns/op | Per-site allocation counts from real sites (reddit, wikipedia, youtube, bbc, github, react.dev, HN) |
| `555415da8` (2026-09-24) | Memoizing AX "isIgnored" cut `getFullAXTree` on a 20k-deep div chain from 5.5 s to 12.5 ms | ReleaseFast |
| `db9779a65` (2026-09-13) | `CURLOPT_PIPEWAIT`: 12 fetches to a fresh h2 origin used 6 TCP+TLS connections before and 1 after; time to the last response went from about 285 ms to about 105–135 ms | Release build, cdnjs fixture |
| `1105aec02` (2026-09-08) | nth-* selector cache: `:nth-child(2n)` at W=4000 went from 55,862 µs to 388 µs per query | lightpanda-io/demo PR 245 benchmark |

Relevance to this design:
- **Per-connection allocator hygiene (`87a69181e`).** It matters if we keep a thread per connection. Our VM is
  destroyed per engine. macOS malloc behaves differently from glibc, so it needs its own measurement.
- **DOM node reclamation (`c3f1314fe`).** It motivates GC-driven node release in `lp-dom` (W2 design: a node is
  freed when it is unreachable and has no wrapper).
- **AX and selector caches (`555415da8`, `1105aec02`).** Design inputs for W2/W3.

## Hypotheses from upstream source (prioritized for TOS)

1. **Per-context cost and reuse.** Upstream allows one browser context per CDP connection
   (`src/server/cdp/CDP.zig:362-365`, `domains/target.zig:129`), so TOS starts one process per Playwright context.
   Each context also costs a new V8 isolate (`Server.zig:538,1333-1360`) and a forced GC on close
   (`Session.zig:203-209`). This rewrite supports many contexts per process (`Browser::create_context`, each with
   its own `NetClient`). Any warm-realm pool must keep fresh-context isolation of cookies, storage and JS globals.
   Pooling must never carry login or test state between contexts.
2. **Realm creation cost.** Measured above for a synthetic registry. Lazy installation (building an interface on its
   first global lookup or first wrap) is planned as a separate, measured PR.
3. **Cold heavy-route stalls.** Upstream loads imports synchronously (`ScriptManagerBase.zig:304-340` spins
   `tickSync`) while the CDP queue is blocked (`HttpClient.zig:1687`). Dependencies are discovered one level at a
   time (`js/Context.zig:509-545`). Code and HTTP caches are off by default (`js/Local.zig:206-238`,
   `network/cache/Cache.zig:39-41`). Here, modules load asynchronously through `ModuleFetcher` and CDP is never
   blocked on loads. JSC bytecode caching and a shared HTTP cache are future work.
4. **Fake layout is O(N) per call.** `getBoundingClientRect` → `calculateDocumentPosition`
   (`webapi/Element.zig:2023-2043`) is uncached, and an IntersectionObserver recheck runs on every mutation
   (`Frame.zig:2477-2481`). Fix: cache document order per DOM version (W2).
5. **Style invalidation is all-or-nothing** (`Frame.zig:2470-2488`, `StyleManager.zig:117-155,954-960`). Fix: keep
   parsed rules per sheet and invalidate per subtree (W2).
6. **CDP `Runtime.callFunctionOn` path.** Upstream parses JSON twice through the V8 inspector (`CDP.zig:156-164`,
   `1296-1300`). The native RemoteObject registry here should use cheap object ids and support releasing object
   groups (W3).
7. **Response-body capture.** Upstream copies every body while `Network.enable` is on, up to 200 MB total and 20 MB
   per resource (`domains/network.zig:43-48`). W3 is building a bounded, lazy body store.
8. **Execution-context loss** (TOS `LIGHTPANDA.md`). Candidate causes:
   - an event path truncated at 128 ancestors (`EventManager.zig:246,617-619`);
   - the 30 s watchdog dropping CDP replies (`CDP.zig:158-160`);
   - a missing API throwing before `preventDefault`, which lets a navigation proceed.

   Each needs a trace on the failing TOS case before any claim.
