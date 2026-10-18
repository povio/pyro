# Native microtasks and Promise species

The browser's `queueMicrotask` and MutationObserver delivery use `Scope::then(undefined, ...)`.
Previously the helper captured `Promise.resolve` and `Promise.prototype.then`, but the latter still
read the settled promise's `constructor` and its `Symbol.species`. Author code could therefore make
native microtask scheduling execute an author constructor. A Promise polyfill that uses
`queueMicrotask` in that constructor can recursively schedule more promises without returning to
the browser task loop.

The helper now retains one private, already fulfilled promise per realm with an own, non-writable,
non-configurable `constructor: undefined`. The captured intrinsic `then` consequently selects its
intrinsic default constructor for undefined-value native scheduling. The seed is never exposed to
page code and dies with the realm helpers. Reactions still enter JSC's ordinary FIFO microtask queue;
callback rooting, exception reporting and cancellation/disposal are unchanged. Non-undefined value
assimilation follows the existing helper path.

## Bounded regression

`crates/lp-browser/tests/microtask_species.rs` tests four observable contracts:

- `queueMicrotask` does not read an author `Promise[Symbol.species]` getter.
- It does not enter a Promise species constructor which schedules another microtask. The regression
  caps this constructor at 16 calls, so the broken implementation fails without an unbounded loop.
- Replacing `Promise.prototype.constructor` with a throwing getter does not break `queueMicrotask`.
- MutationObserver's native microtask delivery also ignores that getter.

All four fail on browser baseline `17cc11a84db2d5c147f731b75cdad4424536ad53` and pass with the repair.
Before repair the first getter was read once, the bounded constructor ran 16 times, and the throwing
getter either escaped scheduling or prevented observer delivery. Expected results are zero author
constructor/species calls and one delivered callback.

## Exact dependency probe

The unchanged XLSX preview uses `xlsx-preview` 1.0.5 and ExcelJS 4.4.0. W2's failing run on protocol
candidate `ca24d212868836b00bf7762e583e8b1dec0b8053` had reached the loading-indicator assertion and then
timed out waiting for the iframe. A native sample of its original browser thread showed module
completion inside JSC microtasks, Promise construction and WeakMap activity. A new CDP connection
answered on another browser thread, which did not establish original-page responsiveness.

An isolated probe evaluates the exact public `exceljs` 4.4.0 `dist/exceljs.min.js` browser bundle:

- Bundle SHA256: `7e49da68588e250dbb8bba190d2caa8ab3787cc0284bda1d8b2f805c4df742c9` (947702 bytes),
  independently downloaded from the npm 4.4.0 tarball and matched to W2's installed bundle.
- Same W1 host/build configuration: macOS 27.0.1 arm64, SDK 27.0, rustc 1.99.1, signed debug executables.
- Baseline executable SHA256: `64ae64a4c277e708679f8adbf5752d893f9a3b641f268ed2f7b5436e55261c25`.
  It hit the two-second engine watchdog: `evaluation_ms=2178 result=execution terminated`, exit 1.
- Repaired executable SHA256: `e09c89828c4e2581229cf2acf621a1d572fcf466ed21d09cd1f8efd27bd72de1`.
  It completed with `evaluation_ms=59 result=ok`, `exports=object:function`, exit 0. An earlier
  repaired evaluation also completed (78 ms).

These timings describe bounded reproduction, not a performance benchmark. The test does not convert
an XLSX file or exercise React, iframe rendering or the original application assertions. It establishes
a dependency-startup failure removed by this repair; an unchanged XLSX preview rerun is still required.

The reusable harness is [probes/promise_bundle_probe.rs](probes/promise_bundle_probe.rs). In an isolated
checkout, copy it to `crates/lp-browser/examples/promise_bundle_probe.rs`, then run
`cargo run --locked -p lp-browser --example promise_bundle_probe -- PATH_TO_VERIFIED_BUNDLE`.
It requires the existing engine watchdog capability and requests termination after two seconds.
The retained invocations also used an external ten-second subprocess timeout. No third-party bundle,
application payload, credential or modified application fixture is committed here.
