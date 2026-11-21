# Combined-head integration guards

Measured detached commit 305840f45afbc4f55bfa24f303c1e858d30e4c1d from a fresh Cargo target on macOS arm64 with CI JSC 0e0dd7038a7a-6f4649e395e0. The static JSC SHA-256 is 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. The wptiter executable SHA-256 is 058c0ac124f412b2e91960d7ed5ebfb343341e90149fa38cf5004a2c6687dc09. WPT revision is a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

Prior results are reused LP receipts. Focus, guard, navigator, production and CSS layout shards compare to 97a4b379d (evidence 1e3f7d8e9). The two VideoFrame shards compare to 678808573 (evidence 89d558209). CSS Values compares to the 21fd48c15 candidate receipt committed at 3e7072d3b. Matched URL, name and occurrence sets are retained; per-cohort strict page results and raw inputs include harness errors, timeouts and missing results. Seven Studio exclusions and two interrupted-shard URLs were withheld from the 236-page CSS Values selection. The 58 production URLs retain the separately verified Wasm storage control.

| Guard | URLs | Prior → combined named passes | Gains / losses | Prior → combined fully passing URLs |
| --- | ---: | ---: | ---: | ---: |
| studio-closeout | 31 | 423 → 423 | +0 / −0 | 26 → 26 |
| css-values | 227 | 4537 → 4538 | +1 / −0 | 171 → 171 |
| focus-00 | 32 | 172 → 172 | +0 / −0 | 7 → 7 |
| guard-00 | 33 | 779 → 779 | +0 / −0 | 27 → 27 |
| navigator-00 | 20 | 279 → 279 | +0 / −0 | 7 → 7 |
| css-00 | 80 | 297 → 298 | +1 / −0 | 69 → 70 |
| css-01 | 80 | 535 → 535 | +0 / −0 | 48 → 48 |
| css-02 | 80 | 1621 → 1621 | +0 / −0 | 52 → 52 |
| css-03 | 75 | 1513 → 1514 | +1 / −0 | 55 → 56 |
| video-focus | 30 | 922 → 922 | +0 / −0 | 18 → 18 |
| video-guard | 19 | 19 → 19 | +0 / −0 | 19 → 19 |
| production-00 | 58 | 9188 → 9198 | +59 / −49 | 14 → 16 |

All six DOMParser lifecycle observations matched the saved Chrome 158 controls. The browser-executed VideoFrame pixel, layout, clone, close, transfer and color-space assertions passed. Commands used the pinned build-exact helper with a fresh target, the no-native-retry WPT runner and original worker counts and flags recorded in guards.py, followed by closeout.py. Each receipt hash and prior receipt hash is in summary.json. This validation measures the specified LP head against its feature precursors.

The 31 Studio closeout observations compare against the saved b2a76c024 cumulative receipts for root viewport cycles and the CSS namespace. Five URLs overlap the CSS Values cohort.

The direct prior-head comparison on the original 734 matching URLs gained 60 names and lost 40. The separate 31-URL direct comparison against db594cd99 gained seven names and four strictly passing pages with zero losses. Six gains cover CSS.escape surrogate preservation, the four namespace tag assertions and the root scrollbar-cycle assertion. The seventh URL, dynamic-viewport-units-rule-cache.html, timed out on the prior binary. Five URLs occur in both the 31- and 734-URL selections. Counts across all twelve cohorts are repeated observations for those five URLs. Exact selection and page results retain both observations.

The two named Rust regressions passed with the same JSC pin. The native test command used the wptiter profile environment from pyro-build: cargo test --locked -j 3 --profile wptiter -p pyro-webapi --test integration -- cssom_namespace_preserves_webidl_string_shape css_root_viewport_condition_cycle_guards_only_scrollbar_properties.
All 34 named gains from the final CSS Values topic versus its parent were selected and passed in the combined binary.
