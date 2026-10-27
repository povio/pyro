# Where browser CPU goes per CDP command (xctrace Time Profiler)

Plain release binary built at f9baf98 (the timers are compiled out). Each workload ran in a loop against one
`serve` process for 10 s while Time Profiler sampled it at 1 ms intervals, on-CPU samples only, all threads.
Sample shares are scaled to the browser CPU per operation that `cdp-perf` measured for the same scenario
(`overhead-run.md`, `plain`, cpu/op). The resolution is CPU/op ÷ samples (6–400 ns per sample, shown per
workload). Buckets come from the innermost stack frame that matches a rule (`tools/xcat2.mjs`).
The host was busy, at load average 8–15, rising to 64 at the end. Shares are reliable; treat absolute
syscall costs as pessimistic.

| workload (cdp-perf scenario) | CPU/op | CDP plumbing¹ | real work |
|---|---:|---:|---|
| getVersion (`rtt.Browser.getVersion`) | 25 µs | **≈ 24 µs (96%)**: wake/park 12.8, socket 8.5, tokio 1.2, dispatch 1.1, JSON 0.5, ws 0.2 | none (static reply) |
| evaluate `1+1` (`rtt.Runtime.evaluate`) | 31 µs | **≈ 25 µs (80%)**: wake/park 13.0², socket 7.8, dispatch 1.6, JSON 1.3, tokio 1.2, ws 0.3 | JSC 2.3 µs, bindings 1.6, checkpoint 1.2, watchdog 0.33, RemoteObject 0.17 |
| callFunctionOn 38 KB (`runtime.callFunctionOn.38KB`) | 238 µs | 95 µs: JSON 59 (two scans of the 38 KB string), wake 16, socket 14, ws 5 | JSC compile/codecache 72 (hash 45 + CodeBlock link 25), JSC call 57 (incl. UTF-8→UTF-16 22, isolatedCopy 4) |
| querySelectorAll 2k (`dom.querySelectorAll.2k`) | 304 µs | 34 µs | selector matching 213 µs, DOM/bindings 43 |
| create+append 1000 (`dom.mutate.1k`) | 3431 µs | 66 µs | JSC 2108, DOM+bindings 1099 (of which C-API lock/protect overhead ≈ 0.5 ms, see below) |
| target lifecycle (`target.lifecycle`) | 2939 µs | 330 µs (11 commands) | realm creation ≈ 1 ms of JS installs; JSC 1722, DFG compiles; watchdog 80 µs |
| navigate simple (`page.navigate.simple`) | 2641 µs | 180 µs | same realm rebuild: JSC 1637, DOM 360, GC 106, watchdog 82 |

¹ cross-thread wake/park (self-pipe `write`/`poll`/`read`, tokio `kevent`), socket `sendto`/`recvfrom`, tokio,
serde JSON, WebSocket framing, outbox. ² Includes `lp_browser` `sleep`/`post`, which wrap the pipe.

## Hot spots and waste

1. **Cross-thread hop per command (≈ 11–15 µs CPU, ≈ 15 µs latency).** The socket thread (tokio, main
   thread) posts each message to the browser thread through a self-pipe (`write`). The browser thread wakes
   from `poll` and drains with `read` (`lp_runtime::wake::Sleeper`). The reply goes back through a tokio
   mpsc, whose waker triggers `kevent`, and tokio parks in `kevent` again after every write. That is 5–6
   syscalls per command on top of `recvfrom` + `sendto`. The perf-trace stages show the same thing as latency:
   `io.to_browser` 8–10 µs, `io.outbound_queue` 4.7–5.3 µs and `outbox.push` 1.6 µs (it includes the wake).
2. **38 KB `callFunctionOn`:** `Scope::call_function_rooted` re-evaluates the declaration every call. JSC's
   CodeCache lookup hashes the whole source (`StringImpl::hashSlowCase` 45 µs), a new CodeBlock is linked
   (`CodeBlock::finishCreation` 25 µs), `String::isolatedCopy` copies it again (4 µs), and the UTF-16
   conversion goes through `encode_utf16().collect()` (22 µs). JSON scans it twice: `parse_command`
   `ignore_str` into a RawValue, then `Command::params` (`cdp.parse` 25 µs + `cdp.params` 28 µs).
3. **JSC C-API lock churn in DOM bindings (`dom.mutate.1k`).** Every API call takes and releases
   `JSLockHolder`: self time 0.49 ms/op ≈ 14%. `realm_for_ctx` → `JSContextGetGlobalContext` runs on every
   binding call (93 µs/op), `JSValueProtect`/`Unprotect` for each Persistent cost ≈ 0.23 ms/op, and
   `JSValueIsObjectOfClass` ≈ 30 µs/op. `drain_finalized` → `JSWeakRelease` → `~JSLockHolder` drains the
   microtask queue (120 µs of 204 µs). `Value::to_string` (DOM strings) costs 148 µs and wrapper creation 172 µs.
4. **Realm creation per page (`target.lifecycle`, every navigation).** `lp_js::realm::install_classes` /
   `install_members_raw` / `structured_clone::install` make JS calls costing ≈ 0.8–1 ms per realm. DFG
   compiles them over and over (`DFG::AbstractInterpreter::executeEffects` ≈ 96 µs, `Plan::compileInThread`).
   `Watchdog::startTimer` → `thread_selfusage` costs 80 µs per realm, about 1 µs per VM entry.
5. **Per-task checkpoint (≈ 1.2–3 µs per command).** `Browser::checkpoint` → `sync_root` clones the frame
   list, `FrameHost::child_keys` / `maybe_fire_load` allocate Vecs, and `drain_finalized` takes a mutex even
   when nothing changed.
6. **By-value results (`runtime.evaluate.largeValue`, from the stage timers).** `remote.describe` 356 µs and
   `register+reply` 297 µs (with `json.serialize` 73 µs) cost 6.6× the JS that produced the value
   (`js.eval` 99 µs) for 1000 small objects.
