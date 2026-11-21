# Round seven CSS Values

Independent QA compared the frozen round-six executable at `305840f45` with exact candidate `710592674` on 27 ordinary WPT URLs. Pyro gained 130 named passes and lost two, a net gain of 128. Named passes went from 653 to 781 out of 831, and fully passing pages from 4 to 14. No names disappeared and no page status changed. No browser restarted or hit the watchdog.

All 63 assertions on `random-in-custom-function.tentative.html` now pass. The implementation registers dashed functions with tree and layer scope, binds arguments and defaults, resolves local variables and computed results, and tracks invocation-specific random identities. The selected pages also exercise inline media condition invalidation and six live CSS font-feature value maps. This batch has partial function support. Nested conditional rules in the function body, named arguments and dedicated CSSFunctionRule wrapper APIs remain outside this implementation.

The inherit() change independently gained 15 assertions with zero losses at `984ac5b71`. One of these closed a Chrome-only gap. Fourteen now pass in Pyro where pinned Chrome fails. The two if() control pages retained their original results. The round-six if-cycle loss remains counted on its original baseline.

## Two counted pinned losses

The pinned parameter-scoping page expects the first parameter’s default reference to a later parameter to produce a guaranteed-invalid value. Both exact Chrome 158.0.8127.37 and Pyro instead return the later parameter's value. Independent QA recorded identical readbacks for all 18 combinations per engine. Changing the later parameter default from 3px to 7px changed the result on both. Changing the caller property from 99px to 117px or removing it had no effect. Both engines returned 6px in the earlier-parameter control. The self-reference control returned an empty raw value and the registered initial value of −1px. These measurements distinguish later-parameter resolution from caller leakage.

The current CSS Mixins editor's draft adds all parameter declarations to the argument rule before resolving that rule. Current upstream WPT still has exactly the pinned test bytes, SHA-256 `368288e44504bd63a543a7bd59a938db51b833009aae3bdbf524abe180fe801a`. Neither pinned loss was removed from the score. [Versioned draft algorithm](https://github.com/w3c/csswg-drafts/blob/804b7b0055e80dfaba091e7f32a4bca303e569e3/css-mixins-1/Overview.bs#L750-L758) · [Versioned upstream test](https://github.com/web-platform-tests/wpt/blob/9986bb32c9e97707eff870ab284ebc5cc7ae8d21/css/css-mixins/function-parameter-scoping.html#L38-L60)

## Verification provenance

* Initial candidate `90623d761` and corrected runtime `710592674f306758548bac60fcaa771165f0d2f9` have identical outcomes on all 831 matched names. The latter also fixes a real first-CSSOM-edit invalidation previously caught by a native guard.
* Frozen executable SHA-256 is `058c0ac124f412b2e91960d7ed5ebfb343341e90149fa38cf5004a2c6687dc09`. The tested candidate binary SHA-256 is `f0c3c22e5783c69b660267043e45ff4e9c8f0036efd1ed7084f0b945c988de08`.
* Seven native browser tests passed on `710592674`. The renderer media invalidation test passed on `06913c789`, which changes only that test’s tuple destructuring. The compressed named WPT reports retain their original executable and source provenance.
* Runs used WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, JSC `0e0dd7038a7a-6f4649e395e0`, wptserve on isolated ports, the pinned manifest, one browser worker and the explicit URL list. QA ran no omitted security or native-fault selectors.

## Font-relative values in functions

At `67c00db3e633bfb17e94560624f1026f15ae39fd`, typed parameters, defaults, locals and results record font dependencies from the values the invocation consumes. CSS font-relative units then resolve against the calling element after its font size is computed. Unused bindings introduce no dependency. The targeted native font and line-height guard passed.

Independent QA ran the same 27 URLs on that exact candidate. Compared with `710592674`, all five remaining assertions on `function-relative-units.html` gained a pass and no assertions lost a pass. Against `305840f45`, the cumulative result is **135 gains, two pinned losses, 786/831 named passes, and 15/27 fully passing pages**. Both later-parameter losses are still individually recorded. No names went missing and no page changed harness status.

The candidate binary’s SHA-256 is `8490f4cd11c467e0106ecfb1cff1baa00c4fce779ecb9c9e5b534b2339ad8453`. A single-job warm build took 69 seconds. The native guard build took 16 seconds and test execution took 0.07 seconds. From initial frozen-base selection to this tranche’s independent validation took about 34 minutes, including host-capacity holds. [Matched before/after results](font-metrics/differential.json) · [Increment since the preceding candidate](font-metrics/incremental.json)
