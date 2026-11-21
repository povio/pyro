# Studio: logical scroll snap alignment and signed offsets

Base `305840f45afbc4f55bfa24f303c1e858d30e4c1d`; implementation `4c224cf840f2fe6bd2fc8038900bd024c1363c1f`. `frozen-rank.json` ranks the frozen Chrome-only named passes per URL and flags Chrome-only strict pages within `css/css-scroll-snap/`. There are 165 such names and 92 such pages, all from main. Keyboard input ranked first (8 names); `snap-inline-block.html` ranked second (7) and isolated a shared layout defect without modifying input dispatch. Columns in reversed scrollers exercise the same signed candidate selection.

Snap selection previously treated inline as horizontal and block as vertical, and floored candidates at zero even when the scroll range was negative. The implementation resolves type and alignment against the container's physical writing-mode sides and compares reachable coordinates on the actual scroll range. Both native guards pass (0.05 s tests, 7.41 s Cargo command): one switches inline/block snapping in a vertical container with an orthogonal target; the other distinguishes RTL start alignment from the left edge and from offset zero.

## Focused comparison

Six ordinary main URLs, selected by intersection with the frozen positive manifests and subtraction of withheld URLs. No supplement, withheld, crash, security or native-fault tests ran. All three runs used the frozen runner, `--kind testharness --dirs css --server wptserve --testdriver --jobs 2 --tests urls.json`, clean WPT pin `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, and no expectation changes. Chrome was the installed stable `158.0.8127.37`. Every run completed all six pages with harness OK, the same 19 observed names, no restarts and no watchdog kills. Strict means every named assertion on a nonempty, harness-OK page passes.

| Run | Strict pages | Named passes | Runner wall |
| --- | ---: | ---: | ---: |
| Frozen | 2/6 | 6/19 | 262 ms |
| Candidate | 3/6 | 14/19 | 207 ms |
| Chrome | 6/6 | 19/19 | 864 ms |

Candidate gains seven names and the strict page on `snap-inline-block.html` and the RTL horizontal column assertion on `snap-to-column-scroll-origin.html`. No reverse losses. Chrome still leads by five names across three pages: two vertical/sideways column assertions, two padding/margin assertions, and one root snap-type assertion. `focused.json` records exact URL/name keys, strict pages, observed-name differences and CPU times in both directions for every pair. These are focused conformance measurements, not suite-wide counts or a speed benchmark.

## Reproduction

Studio raw files: `~/Library/Caches/lightningpanda/studio-r7/{frozen-evidence,scroll-snap}`. Source `~/Library/Caches/lightningpanda/pyro-env.sh`, set `CARGO_BUILD_JOBS=3` and `CARGO_TARGET_DIR` to the isolated `scroll-snap/target`, APFS-cloned from the warm flex target. `pyro-build` used the default render-enabled wptiter binary; warm-up build 155 s, final build 43 s. Native command: `cargo test --profile wptiter --locked -p pyro-webapi --test integration logical_snap -- --test-threads=1` with the same wptiter profile environment as `pyro-build`.

Patched JSC key `0e0dd7038a7a-6f4649e395e0`; static library SHA-256 `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`. Binary SHA-256: frozen `50c39eb1d6161c8ef97d2063bddf94c53fa73ab8b7995b4d9448f5b3fe3a0880`, candidate `de6a906415ca0214a6b298cf4d7fb8fe9fc01634e1cf15189ea46814b6d0ccea`.
