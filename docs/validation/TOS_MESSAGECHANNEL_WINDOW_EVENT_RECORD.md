# MessageChannel / window.event candidates: TOS runs and bounded diagnostics, 2026-10-17

This is a docs-only consolidated record. Product state at the time of writing:
- PR #75 (`MessageChannel`, `rewrite/message-channel`, approved head `b6fb762`)
  is **held**.
- PR #76 (`window.event`, `rewrite/window-event` at `994f9f9`, stacked on #75) is
  **held**.
- Neither is merged. Main is unchanged by them; the pinned main binary is
  `9f100bd`.

Updated 2026-10-18: the PR #81, PR #80 (JIT), MessagePort lifetime and final
integration sections below supersede the product state above. PR #75 and PR #76
are now merged. See **Status**.

> **Timer observation horizon (corrected).** In the batch 19 diagnostics,
> next-task `setTimeout(0)` markers were **unobserved only within 2–133 ms** of
> their registration (mostly 10–90 ms) before the timeline was read.
> - An earlier "~740–880 ms" figure was an analysis error: it was measured from
>   the first post of the whole page timeline, including page load.
> - None of these markers was explicitly cleared. Late versus lost is
>   undetermined.
> - A minimal repro showed no lost or late timers in any tested context.
> - They are not described as lost anywhere in this record.

## Binaries

Every release build comes from a fresh `--no-hardlinks` clone with its own
target directory. Every hash was verified before each run.

| Binary | Source | Release SHA-256 | Use |
| --- | --- | --- | --- |
| main `9f100bd` | pinned main | `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406` | batches 13–19 and baselines |
| `ab1e7b2` | PR #75 first head | `423c1038ca80c6050b5f09147235e8f2594b3e40df75dd022c985c0ec93d8a42` | built before the P2 hold; **not used** for any probe or TOS run |
| `b6fb762` | PR #75 approved head (P2 repair), tree `0dfda21d` | `8d7292f81fc487976ab65f0a4c27113d27e3ad276860ed3399ca5a8cc19ce184` | candidate, `CARGO_INCREMENTAL=0` |
| `994f9f9` | PR #76 head, tree `ffdc4221` | `3ff53d8d9c1cc15f4d5c466e3fd517c4ba28b343bc646323cb725124b91afd76` | candidate, `CARGO_INCREMENTAL=0` |
| upstream `2ad38773` | Zig upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | comparison |

## Unchanged TOS reruns (compatibility evidence)

- TOS is `f9fde4aa0d` with an unchanged manifest (`542d479b…`).
- The selections are the recorded batch 16 selection (`df59d443…`, 14 cases) and
  batch 19 selection (`fafb77a6…`, 10 cases).
- Each candidate got a fresh fixture for its first run; the repeat reused that
  fixture with the e2e tree verified unchanged.
- Fresh run-owned DBs, 0 left. One worker, native PostgreSQL 17.

| Selection | `9f100bd` (earlier) | `b6fb762` | `994f9f9` | upstream |
| --- | --- | --- | --- | --- |
| batch 16 | 13/14 | 14/14 (`lpb16mc`), 14/14 (`lpb16mc2`) | 13/14 (`lpb16we1`), 13/14 (`lpb16we2`) | 14/14 |
| batch 19 | 6/10; diagnostic repeats 7/10, 7/10 | 3/10 (`lpb19mc`), 3/10 (`lpb19mc2`) | 3/10 (`lpb19we1`), 3/10 (`lpb19we2`) | 10/10 |

- The batch 16 failure on `9f100bd` and `994f9f9` is the timing-sensitive
  business-partner-accounts "deletes an account line…" case.
- The batch 19 failures on both candidates are the same 7 ports autosave cases,
  all failing with `getInputOrTextareaByValue` → -1.
- These results are per binary. They are not aggregated and make no causal claim.

## Unresolved cases (kept separate)

- **Batch 7, credit note (2 cases).** Timing-dependent: the test reads
  `page.url()` right after a response. See [batch 7](TOS_COVERAGE_BATCH_7.md).
- **Batch 16, account delete (1 case).** Unresolved and timing-sensitive. See
  [batch 16](TOS_COVERAGE_BATCH_16.md). It passed on `b6fb762` in both runs and
  failed on `9f100bd` and `994f9f9`. No cause is established.
- **Batch 19, ports autosave (4 on `9f100bd`; 7 on both candidates).** See
  [batch 19](TOS_COVERAGE_BATCH_19.md). The worsening with `MessageChannel` is
  isolated below, but no cause is established.

No shared cause is claimed among these.

## Bounded diagnostics (not compatibility evidence)

All diagnostics ran on separate instrumented fixtures, with the tests' assertions,
auth and manifest unchanged, fresh run-owned DBs (0 left), and no product edits or
artificial delays. Artifacts:
- `/private/tmp/lp-tos-batch19-diag2`: FINDINGS.md, patches for each step, and
  `analysis/`;
- `/private/tmp/lp-order-repro`, `/private/tmp/lp-mc-probe`,
  `/private/tmp/lp-timer-repro`, `/private/tmp/lp-event-context-repro`.

Superseded repro versions (`*-v1.*`) are kept.

1. **Isolation (ports, 10 cases).**
   - `9f100bd` 8/10, `b6fb762` 3/10, upstream 10/10.
   - `b6fb762` with `MessageChannel` hidden by an init script (a control, never
     shipped): 8/10.
   - So on this file, the presence of the API is the factor.
2. **In-page timeline.** Timestamps are recorded before navigation, with no CDP
   round trips added before the lookup. The first form value lands at about
   19–39 ms with the setTimeout fallback, about 21–23 ms on upstream, and on
   `b6fb762` usually not before the lookup (about 114 ms when it does).
3. **Stacks and heartbeat.**
   - On `b6fb762`, all 11/11 long posts come from React `Iu` →
     `unstable_scheduleCallback` → `port.postMessage`.
   - A 1 ms heartbeat recorded 0 ticks during those gaps. That shows callback
     progress only; the native wait state is not inferred.
4. **Phase markers.**
   - The post itself costs +0.0 ms.
   - The `queueMicrotask` marker runs at +36.6..+45.5 ms on `b6fb762`, against
     +2.4..+2.8 ms upstream.
   - Delivery comes 3–13 ms after that checkpoint.
   - Limits: the marker runs behind any earlier-queued microtasks. It cannot
     separate synchronous work from microtask work, and it does not mark the end
     of the task.
5. **Timers.** See the corrected horizon above.
   - Minimal repro: every context ran its marker within 0.8 ms, with no
     duplicated or reused IDs.
   - `9f100bd` `MessageChannel` contexts were marked unsupported.
6. **Attribution** (2 cases).
   - The stalled window on `b6fb762` took about 51 ms with wrappers, against about
     39–45 ms in the control.
   - About 8 ms is in 21 wrapped DOM calls (`setAttribute` ×2402,
     `createElement` ×482, …). The other ~43 ms is unattributed: it may include
     unwrapped bindings, native work, GC and instrumentation. It is not proof of
     pure-JS or lp-js cost.
   - Upstream had no comparable window.
7. **Idle wake-up.** Not reproduced in the tested posting paths: timer, nested
   timer, rAF, chained port, fetch, microtask and `import()`. That does not rule
   it out universally.
8. **window.event.**
   - `9f100bd` and `b6fb762` lacked it; upstream and Chrome set it during
     dispatch, including message dispatch and the callback's microtask
     checkpoint.
   - An init-script shim control on `b6fb762` was **negative**: 3/10 twice with
     the shim, 3/10 twice without. A shim may not reproduce native semantics, so
     this does not rule out a link universally.
   - The native implementation (PR #76, `994f9f9`) matches upstream and Chrome on
     every probed row, but leaves batch 19 at 3/10.

Side observation, deferred: in a repro server launched without TOS's
`--load-resources` flags, a dynamically inserted `<script src>` never ran on
`b6fb762`. This was not investigated and is not TOS-relevant.

## Runtime fix validation (PR #78), 2026-10-18

W1's lp-runtime fix (PR #78, final head `ca2ada7`) runs local tasks and due timers
in readiness order. Before it, every queued local task ran before any due timer,
so a self-reposting MessageChannel loop starved due timers.

The release builds below were made from `e0812a5`. Between `e0812a5` and `ca2ada7`
only `#[cfg(test)] mod tests` changed; the non-test source is byte-identical
(verified). The binaries are labelled with the SHA actually built.

Builds: fresh `--no-hardlinks` clones with their own target directories,
`CARGO_INCREMENTAL=0`, no V8. Integrations are local throwaway merges, never
pushed.

| Candidate | Commit / tree | Release SHA-256 |
| --- | --- | --- |
| main `4872e20` + fix | `e0812a5` / `1e9b7815…` | `3de2ef064082e78698ceb5cebdcdac7042363738f30bb1138c1f7ef10f666fb7` |
| PR #75 `b6fb762` + fix | local `6adc0a6` / `d44b2eaed0b6165880ad599c494a3f91ee261171` | `04a1ebaf854c01d6dfe5072eb1688733e56ee34684500cfa516e499f0ee244d6` |
| PR #76 `994f9f9` + fix | local `b1fe3fa` / `3d353841a45dc64f46adb25b43bc68252256b538` | `fc6cfeef57dae697f3a8961926516bbf58a5fbca3b4beb0caa73fa2261531163` |

Unchanged TOS:
- Same selections as above.
- A fresh fixture per candidate and batch; the repeat reused it with the e2e tree
  verified unchanged.
- Fresh run-owned DBs, 0 left.

| Selection | main + fix | PR #75 + fix | PR #76 + fix |
| --- | --- | --- | --- |
| batch 19 | **10/10, 10/10** (`lpb19fx1/2`) | **10/10, 10/10** (`lpb19fxmc1/2`) | **10/10, 10/10** (`lpb19fxwe1/2`) |
| batch 16 | 14/14, 14/14 (`lpb16fx1/2`) | 13/14, 13/14 (`lpb16fxmc1/2`) | 13/14, 13/14 (`lpb16fxwe1/2`) |

Ports (batch 19):
- Repaired on all three fixed candidates, in both runs.
- This establishes the TOS link for the ports regression: due timers starved
  behind chained local MessageChannel tasks.
- The fix also repairs the 4 ports failures on `9f100bd`, without MessageChannel.

Residual batch 16 limit:
- Business-partner-accounts "deletes an account line…" passes 2/2 with the fix
  alone and fails 2/2 on each MessageChannel stack with the fix. It also failed
  on `9f100bd` and `994f9f9`.
- Diagnostic only, on a separate fixture (`instrumentation-step7.patch`): the row
  is present and visible at the read.
- On PR #75 + fix the target input value is written about 28.6 ms after the test
  starts its single read, with React MessageChannel work still running after it;
  on upstream, about 8.8 ms.
- The observation recorder shifts timing: main + fix failed once under it.
- No scheduling or semantic defect is established for this case. It remains
  under investigation.
- The 14/14 versus 13/14 split in the table above does **not** establish
  MessageChannel-specific causation. The additional native-trace runs below show
  the case also fails without MessageChannel.

Additional single-case runs (native task trace, diagnostic):
- Unchanged test, `--grep "deletes an account line"`.
- Fixture `/private/tmp/lp-tos-f9f-batch16-fxmc`, reused; its e2e tree was
  verified unchanged.
- Fresh run-owned DBs, 0 left.
- W1's diagnostic-only trace branch `diag/runtime-task-trace` at `142840e` (main
  `6ad00f2` + PR #78 + an `LP_TRACE_TASKS` native task trace; never for merge),
  built fresh with `CARGO_INCREMENTAL=0`.

| Build | Release SHA-256 | Native trace only | With `DEBUG=pw:protocol` |
| --- | --- | --- | --- |
| main + PR #78 + trace (`142840e`, tree `8cfa3e9d…`) | `add744d26cc9202aff8145b530d8935640950ea14edd628ac44b6b02a2044046` | **fail, fail** | pass, pass, pass |
| PR #75 `b6fb762` + trace (local merge `51dd97a`, tree `0b26bda5…`) | `c7be4c0e9beb553dcacdb167debdd5e7ff8b1106bc40988f611928141ae0206c` | **pass, fail, fail** | pass, pass, pass |

- Without MessageChannel the case fails 2/2; with PR #75 it is 1/3.
- Client-side Playwright protocol logging flips it to pass 6/6. That supports
  timing sensitivity but proves no mechanism.
- Trace directories:
  `/private/tmp/lp-tos-b16acct-ntrace-{diag,diagmc}-r*` and
  `/private/tmp/lp-tos-b16acct-trace-{diag,diagmc}-r*`.

## Remote readiness validation (PR #81), 2026-10-18

W1's PR #81 (`be68374`, merged as `ccef1bf`) stamps remote tasks (CDP
deliveries, network completions) when they are posted and runs them in readiness
order with local tasks and due timers. These builds do **not** include PR #80, so
there is no JIT.

| Candidate | Commit / tree | Release SHA-256 |
| --- | --- | --- |
| rd = PR #81 | `be68374` / `afa59ae5…` | `4d98062b2db8a5fad273b0d99e0195d8d3018c401cdc43da3b3788275475633d` |
| rdmc = PR #75 `b6fb762` + PR #81 | local `a320731` / `0a6da142…` | `0ecad8e85d51c9e1cf9da1bc1ccf7a9ccd53b91ecc68f012278976c878fcbadb` |
| rdwe = PR #76 `994f9f9` + PR #81 | local `5e28214` / `9e46bd37…` | `94c9eab2a8548dfae5cb400197ec888972347ad78c90a13e92baf0cca07a01de` |

Account-delete case, single case (`--grep`, no protocol log):
- 3/3 on each of rd, rdmc and rdwe.
- Before PR #81: main + PR #78 failed 2/2, and PR #75 + PR #78 passed 1 of 3.

Unchanged run + repeat:

| Selection | rd | rdmc | rdwe |
| --- | --- | --- | --- |
| batch 19 | 10/10, 10/10 | 10/10, 10/10 | 10/10, 10/10 |
| batch 16 | **14/14, 14/14** | **14/14, 14/14** | **14/14, 14/14** |

Runs: `lpq19{rd,rdmc,rdwe}{1,2}` and `lpq16{rd,rdmc,rdwe}{1,2}`.

This supports the mechanism W1 found in the native traces: the test's CDP read
was overtaking a page timer or port task that became ready earlier. The repair
is attributable to PR #81 alone.

Reduced repro (`due-vs-cdp.mjs`): commits via `setTimeout(0)` now match Chrome
(20/20 on all three; 0/20 before PR #81).

MessagePort commits:
- rdmc: 170/170.
- rdwe: **369/370**.
- The single rdwe miss happened before the instrumentation that could classify
  it, and it did not recur in 350 further iterations.
- It is recorded as **unexplained**, and repro parity is **not** claimed.

## MessagePort lifetime repair (PR #75), 2026-10-18

W1 found that a queued `MessagePort` could be collected and its messages lost
(V1/V2). The repair is in PR #75 (final head `fbc3c13`; PR #76 `567ab17`
contains it).

The pinning rule:
- A port is pinned exactly while it is started, open, and either entangled or
  holding undelivered messages.
- The pin is re-evaluated on enqueue, start, delivery and close (including the
  peer's), and when the peer is collected.
- The peer's `Drop` only records the surviving port. `PageDom::after_task`
  reconciles it at the next safe point, so no JS runs from a drop, with or
  without page hooks.
- Disposal releases every pin.

Forced-GC regressions, in `crates/lp-webapi/tests/dom_js.rs` (each fails on the
code before its fix):
- queued messages kept across GC when the peer closes;
- late start after the peer was collected;
- unstarted queued ports and disposal hold no roots;
- a receiver is released when its sender becomes unreachable;
- peer collection is reconciled at `after_task` without hooks.

Validation, `CARGO_INCREMENTAL=0`:
- PR #75 `fbc3c13`: fmt, clippy and test exit 0, 487 passed.
- PR #76 `567ab17`: fmt, clippy and test exit 0, 491 passed.

The focused review passed both heads.

## JIT validation (PR #80), 2026-10-18

PR #80 (`306ceac`, merged as `a756213`):
- links through `scripts/macos/link-and-sign.sh`, which ad-hoc signs with
  `com.apple.security.cs.allow-jit`;
- defaults `JSC_usePollingTraps=true` in lp-js.

Build rules:
- Every build runs cargo from inside its own fresh `--no-hardlinks` clone, so that
  clone's `.cargo/config.toml` applies.
- Each binary was checked for:
  - no "without scripts/macos/link-and-sign.sh" warning;
  - the `allow-jit` entitlement (`codesign -d --entitlements -`);
  - the polling-traps default present in `crates/lp-js/src/engine.rs`.
- Earlier JIT builds made with `--manifest-path` from another cwd (`j-main`,
  `jmc`, `jwe`), or from the wrong branch (`j2-main`), had no entitlement. They
  are labelled invalid and are not used. Hand-signed copies were diagnostic
  only.

| Label | Source | Local commit / tree | Release SHA-256 | CDHash |
| --- | --- | --- | --- | --- |
| j4-main (baseline) | main `ccef1bf` + PR #80 `306ceac` | `12458a0` / `bb51384d…` | `dca3d768f57218531686972ea7c552cb593e58ad273b3d78f9448a4485cc0b5a` | `f2ea0a17…` |
| jfmc | PR #75 `fbc3c13` + main `ccef1bf` + PR #80 | `b3ab5a9` / `3ab6638c…` | `ee8918b9e72b03e615003be40ff831f7ce65dd18a7f092e38b56395859bafff6` | `16989c96…` |
| jfwe | PR #76 `567ab17` + main `ccef1bf` + PR #80 | `d8abe39` / `9c592e27…` | `3acaf346d4cf1fdfb494424888f73045e5570badceb3f280269908927cc41498` | `91d02384…` |

Unchanged TOS run + repeat:
- Fixtures `lp-tos-f9f-batch{19,16}-fx` reused, with the e2e tree and status
  verified unchanged.
- Fresh DBs, 0 left.

| Selection | j4-main | jfmc | jfwe |
| --- | --- | --- | --- |
| batch 19 | 10/10, 10/10 (`lpj419j4{1,2}`) | 10/10, 10/10 (`lpjf19mc{1,2}`) | 10/10, 10/10 (`lpjf19we{1,2}`) |
| batch 16 | 14/14, 14/14 (`lpj416j4{1,2}`) | 14/14, 14/14 (`lpjf16mc{1,2}`) | 14/14, 14/14 (`lpjf16we{1,2}`) |

Playwright time with JIT: batch 19 is about 11 s and batch 16 about 15 s. This is
functional evidence only, not a performance claim.

## Integration check on current main (`a756213` + PR #76 `567ab17`), 2026-10-18

The JIT candidates above predate PR #82 and PR #83 (history traversal), which are
now on main. This is one fresh combined candidate.

Build:
- `567ab17` merged with main `a756213`: local `614b8f2`, tree `551b01bf…`.
- Ancestry verified for `567ab17`, `fbc3c13`, `a756213`, `306ceac` and PR #82
  `3f9b73d`.
- The only file changed on both sides is `docs/PARITY.md`, which merged
  automatically. `git show --remerge-diff` is empty.
- Signing, entitlement and polling-traps checks as above.
- SHA-256 `521dd4f7eba04f6e440e18b18b2c34b5808c48d63878c48a633b8dc0e44433ba`,
  CDHash `42ec3fab…`.

Results, run once each:
- Protocol smoke, `node tests/e2e/about-blank.mjs <binary>` (W3's check): 31/31
  ok, including goBack/goForward across SPA and document history.
- Batch 19: 10/10 (`lpjc19`).
- Batch 16: 14/14 (`lpjc16`).
- W3-02 (selection sha256 `d5ee9317…`, 15 cases): 15/15 (`lpjcw302`). This
  includes "Email Logs › restores text drafts on Back and Forward after
  duplicate blur commits".
- W3-02 ran on W2's fixture `lp-tos-f9f-batch16-fx` (TOS `f9fde4aa0d`).

## Status

- **Batch 19 ports:** repaired by PR #78 (due timers starved behind chained
  local MessageChannel tasks).
- **Batch 16 account-delete:** repaired by PR #81 (a CDP read overtook
  already-ready page tasks).
- **Queued-port lifetime defect:** repaired in PR #75 `fbc3c13`.
- **JIT:** with PR #80, batches 19 and 16 pass on main and on both feature
  stacks. The combined candidate on current main also passes the PR #82
  Back/Forward checks.
- **Merged:** PR #75 at exact head `fbc3c13` as
  `e42e9f7abc5c9f875da626691edeefe35098d73c`; then PR #76, retargeted to main, at
  exact head `567ab17` as `e56e787aeef8cb5764567c8c234042ea88ef14ec`. The parent
  merged both after the focused approvals and the integration check above.
  Merged main `e56e787` has tree `551b01bfba0dcb9637ce2b9153b9f5c3b3b488ef`,
  which is identical to the tested integration candidate `614b8f2`.
  The local integration commits in this record were never pushed.
- **Limits:**
  - Fixtures were reused within a family, with the tree and status verified.
  - W3-02 was run once, on W2's fixture.
  - The integration check ran each selection once.
  - W3's optional engine cargo tests were not run.
  - The invalid JIT builds are excluded from all evidence.
- **Open:** the 1/370 MessagePort repro miss is unexplained. No repro parity is
  claimed. Batch 7 (credit note) is separate and not addressed here.
