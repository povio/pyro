# MessageChannel / window.event candidates: TOS runs and bounded diagnostics, 2026-10-17

This is a docs-only consolidated record. Product state at the time of writing:
- PR #75 (`MessageChannel`, `rewrite/message-channel`, approved head `b6fb762`)
  is **held**.
- PR #76 (`window.event`, `rewrite/window-event` at `994f9f9`, stacked on #75) is
  **held**.
- Neither is merged. Main is unchanged by them; the pinned main binary is
  `9f100bd`.

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

## Status

- The batch 19 worsening with `MessageChannel` is explained and repaired by the
  lp-runtime fix (PR #78). Evidence is per binary, above.
- Diagnostics and product expansion are stopped by the parent.
- PR #75 and PR #76 remain held pending focused review of #76 and a combined
  decision.
