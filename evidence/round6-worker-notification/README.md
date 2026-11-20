# Worker Notification exposure and Wasm storage controls

Local topic `topic/round6-worker-notification` at `07d2e50f53971607c94fdc0a926dda38f0df8799` is directly based on `aa0268b479a6b463da16910b4b743cf38901d962`. The Rust host and the notification/clone bindings changed. The pinned shipped JSC archive, JSC API, backend dependencies, WPT tests and expectations did not change.

## Measured result

Local Chrome stable 158.0.8127.37 and both LP binaries ran exactly the same URLs from WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` with the shared manifest runner, wptserve, generated globals, testdriver, and two jobs. All three used fresh browser contexts. Previous Chrome 157 or canary observations were used only for initial triage.

| Scope | LP base named passes | LP final named passes | Chrome 158 named passes | LP gained/lost | Missing/new LP names |
|---|---:|---:|---:|---:|---:|
| Two Notifications storage-serialization pages in wasm/ | 2 | 4 | 4 | +2 / 0 | 0 / 0 |
| Seventeen adjacent notification and IDB/history pages | 94 | 168 | 152 | +74 / 0 | 0 / 0 |

The Window target already passed both subtests. The DedicatedWorker target now passes both as well. The tests construct an eight-byte empty native WebAssembly.Module and check that notification data rejects it with DataCloneError, including getter ordering within an ordinary array. No Wasm execution was required. This fixes the worker host API exposure and its persistent-storage rejection. It does not implement successful module cloning, asynchronous compilation, instantiation or execution. Among the 616 pinned Wasm manifest URLs, this lane remeasured five browser URLs: the two targets plus two IndexedDB and one history control. The other 533 of the original 538 browser URLs and 78 jsshell variants were not re-executed. Previously withheld runtime workloads and Wasmi experiments remain separate.

All target pages completed on all three binaries with zero restarts. For the guard, both LP builds completed 15 and timed out on two (onclose and tag); Chrome completed 12, reported three harness errors and timed out on two. LP had 8 fully passing guard pages before and after. The Chrome guard had 6 full pages. The guard is not an across-browser win: 60 LP-only and 44 Chrome-only names remained. `target-final-summary.json`, `guard-final-summary.json`, and the corresponding compressed named ledgers retain successes and missing outcomes.

## Ordinary worker controls

The final binary matches Chrome 158 in 22 of 24 observed controls, including both dedicated and shared worker construction, data copying and worker-relative icon URL, callback/sequence conversion order, cached frozen actions, absence of window-only requestPermission, explicit granted/denied permission updates and the service worker's constructor restriction. The program never creates a notification while granting it permission. Two prompt-state readings differ: LP returns default and this Chrome returns denied in workers. The Notifications Standard maps a prompt result from its permission-state query to default. The browser context's mutable permission table is shared with worker threads; worker realm objects and their JS callbacks remain thread-local. Closing or detaching the page does not transfer the Window's JS-owned data to another thread.

On both LP revisions and Chrome 158, in dedicated and shared workers, the original healthy eight-byte module returned its genuine native module brand, empty imports and empty exports. The Response probe consumed a normal application/wasm Response as bytes, posted the empty module's synchronous constructor to an owner timer and verified bodyUsed, native Module identity and metadata. No module was instantiated or executed. `probe-aa.json`, `probe-final.json`, `probe-chrome158.json` and `probe-summary.json` give all observed outcomes. These custom probes are distinct from official WPT scores.

## Why buffering alone does not qualify compileStreaming

The WebAssembly Web API allows a complete Response body to be buffered. It specifies exact case-insensitive application/wasm MIME matching after trimming HTTP space/tab, a CORS-same-origin response, OK status, consumption of the internal body, propagation of rejection reasons, and asynchronous compilation of stable bytes with the converted options on the networking task source. Parameterized MIME types are not accepted.

The JS API explicitly places compilation in parallel and queues the completion. The pinned native Module constructor checks the JSC realm's WebAssembly-enabled flag, reads supported compile options, and invokes synchronous validation. Calling it from a timer settles the tiny normal control, but the synchronous work still occupies the owner for its entire input-dependent duration. That isolated control does not measure acceptable latency on an unbounded response. A late constructor call also does not provide the required original WebIDL options-conversion timing by itself. Deferring to the existing native compile promise has an independent delivery problem: archived original exact-JSC empty-module controls timed out, and no new native async, nonempty-module or fault work was run here. The archive's C API offers no checked module transfer across independent JSC VMs.

CSP defines a realm-specific byte-compilation authorization hook. The constructor itself checks JSC's global enabled flag; an existing LP path connecting that flag to the effective document/worker CSP through the public C embedding was not established by the static review. No claim of a policy bypass is made and no such test was performed. These constraints preclude shipping a buffered synchronous shim as a transparent streaming implementation. References: https://webassembly.github.io/spec/web-api/ , https://webassembly.github.io/spec/js-api/ , https://w3c.github.io/webappsec-csp/#can-compile-wasm-bytes , https://notifications.spec.whatwg.org/ .

## Reproduction and review

`run` receives a label, a binary/Chrome launcher and either `target` or `guard`. The URL lists are fixed in `target-urls.json` and `guard-urls.json`. `compare.py final` computes the page/name/occurrence comparisons from the raw files, preserving duplicate names and the harness status. `probe.cjs` supplies the ordinary persistent-storage, non-Wasm controls and empty-module introspection. `chrome158` invokes CHROME_BIN. Set RUNNER_DIR, WPT_CHECKOUT and WPT_MANIFEST as documented in run, and make playwright-core available with NODE_PATH for the standalone probe.

The verified test binary SHA-256 is `24f5da9e683819fd2e84ff2af0798ec19c166706f6c88e29ec27594742a0dbf1`. The independent reviewer signed off on final patch SHA-256 `c1cc7232f6148c8728bad1969f456e89eea7933046041866cfd017c6420cfe3a` after rechecking observable option conversion, action-array identity, explicit storage-mode traversal and sequence-iterator behavior. No PR or merge was performed by this lane.
