# Browser integration validation, 2026-10-17

The automation worker tested pristine browser candidate
`7551f073a8cc4fc65a5e641da44410a8d36551fc` in a clean detached checkout on Apple M1,
macOS 26.7.1, with SDKROOT `/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`.
Debug and release ran sequentially, with the same Cargo.lock and host. Other host
activity was uncontrolled. This is functional integration validation, not a benchmark.

| Exact candidate profile | Browser + CLI tests | Expanded Playwright 1.59.1 |
| --- | ---: | ---: |
| debug | 27/27 | 21/21 |
| release | 27/27 | 21/21 |

Commands used `cargo test --locked [--release] -p pyro-browser -p pyro`
and `cargo build --locked [--release] -p pyro` (the optional argument is
`--release`). Each test/build/E2E process returned exit 0. No SIGSEGV or SIGTRAP
was observed; these runs do not replace the earlier repeated loader stress suite.

Binary SHA-256:

- Debug: `ebf41bf0c9d59af48a1686312479803d6c249cd538ab1a58ef78255e1a37e595`
- Release: `7ade1c63905817913a6bbe61bf824d727fe26ced7b215587478d94e269a0d6f2`

The expanded harness is `tests/e2e/about-blank.mjs` from protocol commit
`b3e7853fb8b04e6d41103a22f531a739983733dd`, SHA-256
`3f33995ac9b6ec40203a5eb1cff934d84baf2531f0b1ed41287d8f3b2bffae07`.
It runs real Playwright against the built candidate. Its four added checks verify:

- DOM/Fetch/Storage availability before init scripts, document lifecycle states,
  and shared native DOM with separate isolated-world JavaScript globals.
- Page fetch response values and body capture through CDP, with the exact current
  frame/loader IDs and a distinct native subresource request ID.
- Page fetch interception/fulfillment and body rejection after AbortSignal abort.
- Local storage sharing within a context, session storage separation between pages,
  isolation between contexts, and preservation across same-origin navigation.

The harness uses isolated loopback HTTP fixtures. It does not exercise DOM/Input
protocol commands, HTTPS, broader resource loading or full TOS flows. No CPU/memory
improvement is inferred. Historical measurements remain tied to `d673567`.

Raw per-profile logs, Playwright JSON and binary hashes are retained on the worker
host under `/private/tmp/pyro-w3-browser-7551-{debug,release}-*`. The original published
candidate and the protocol integration merge are separately preserved. No browser
source was changed by the automation worker.
