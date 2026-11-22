# SVG text positioning length lists

The combined RC.5 scan found 56 signed-Chrome-only named occurrences explicitly naming SVGLengthList or SVGAnimatedLengthList on 21 V2-eligible URLs. Seventeen of those URLs were strict Chrome passes. The frozen 41-page selection adds the neighboring list attribute synchronization fixture and 19 point-list/single-length controls. Its SHA-256 is ab3900f12a1f97c5a889c4fac2fe097fba09a9c068e542cf91c718dc3a2eee6b. The full SVG IDL harness tests many unrelated interfaces, so its complete page totals are reported separately from the length-list cohort.

The topic starts at integration revision 0e6341d8b4404ce168c4ad3544960ec04b2fb436. The archived parent executable 1d142e841e98b7d762b65b2e935a28c393c9df04f3b1367b15cc64f110f4cb6a was originally built at 4b6249f2f; provenance is recorded in 2055f24ee. Comparing those source commits showed differences only under docs/validation/round7-audio-offline-quantum. The same binary was used for the fresh local parent run. Signed Google Chrome stable 158.0.8127.37 WPT results are a 22-shard projection at pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Direct Chrome controls use the same signed executable.

SVGTextPositioningElement exposes stable x, y, dx and dy list pairs on text and tspan. Existing SVG list storage implements insertion, removal, item identity and attribute synchronization. SVGLength items retain their unit and resolve using the current destination list's coordinate axis, viewport and computed font. A read-only Stylo computation resolves font and viewport-relative lengths without changing the CSS cascade. Unanimated animVal mirrors baseVal through a distinct read-only list. Active SMIL length-list interpolation, CSS math expressions and container-query units are outside this implementation. The signed controls for cqw/cqh/cqmin document the current remaining gap.

SVG 2 specifies stable objects for surviving list slots, copying items already attached to a list, and clearing both readonly and directionality when detaching SVGLength items. Attaching a length assigns the reflected attribute's directionality. The list mutator and cross-document controls record the signed implementation's differences from these rules. Signed Chrome accepts a trailing comma and rejects the leading-space example in parse-probe.cjs; the implementation follows the SVG 2 text grammar's complete comma-delimited groups with surrounding whitespace. Numeric outcomes and object identity are reported independently. The unmodified WPT fixtures provide their own expectations.

- https://svgwg.org/svg2-draft/types.html#ListInterfaces
- https://svgwg.org/svg2-draft/text.html#InterfaceSVGTextPositioningElement

## Commands

Use the cache layout from run.py, the archived base executable and the pinned static JSC archive selected by build-exact. Both WPT arms use four page workers without native retry.

    python3 signed-reference.py
    python3 run.py base
    ./candidate.sh "$FEATURE_COMMIT"
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node parse-probe.cjs candidate ./pyro
    node cross-document.cjs candidate ./pyro
    node font-probe.cjs candidate ./pyro
    node relative-probe.cjs candidate ./pyro
    ./native.sh
    python3 report.py
    python3 classify.py
    python3 families.py

The same direct controls were run on the archived parent and on signed Chrome. Raw page records, complete named occurrence ledgers, strict page outcomes, harness errors and missing observations accompany this comparison.

## Results

| Cohort | Integrated parent passes / observed | Candidate passes / observed | Signed Chrome passes / observed | Strict pages parent / candidate / Chrome |
| --- | ---: | ---: | ---: | --- |
| 22 target URLs | 1,284/1,917 | 1,362/1,917 | 1,814/1,917 | 0 / 8 / 17 |
| 19 neighboring guards | 210/214 | 228/228 | 225/228 | 11 / 19 / 18 |

Across both cohorts the candidate recovered 82 already-observed named failures and made 14 additional passing names observable. No previous pass was lost and no previously observed name disappeared. The 41 candidate pages all completed, up from 35 complete and six harness errors in the parent. All runs recorded zero restarts and watchdog kills. Of the 56 signed-only names explicitly mentioning a list interface, 50 now pass. The six residuals all require active SVG animation of length lists, including interpolation across units or differing lengths, by/additive animation, and percentage values. Across the entire 41-page corpus, the candidate recovered 81 previously signed-only passes and recorded 53 LP-only passes; 502 Chrome-only names remain, including 496 on the broader non-animation target surfaces such as the full SVG IDL harness.

The direct probes match on unit-aware reflected edits, out-of-range/readonly/type exception names, font conversion in the destination frame and independent axes with explicit viewport dimensions or viewBox. For the ex/ch/cap/ic/lh and rlh controls, candidate SVG values agree with the candidate's CSS measurements after two font-size/line-height settings. Numeric ex/rex metrics differ slightly between engines; each agrees with its own CSS metric. Signed Chrome uses a normalized diagonal after appending a detached percentage item; the candidate uses the destination attribute’s horizontal or vertical axis, as SVG 2 specifies. A direct CSS-sized/omitted-height viewport guard records a current scalar-resolution limitation: when the ancestor lacks one of the explicit dimensions and has no viewBox, the candidate returns zero for both axes. Container-query length units and CSS math expressions remain unsupported by the token-list parser. Downloaded font-face metrics are unverified by these controls.

Seven native checks passed: five list-storage tests (including number/point regression guards) and two existing SVGLength binding tests. Independent QA repeated the 41 target/guard URLs on the immutable candidate with the original archived parent and signed Chrome projection. Its full named-occurrence ledger, strict page ledger and six direct output files match the primary run. Its records are in qa/.
