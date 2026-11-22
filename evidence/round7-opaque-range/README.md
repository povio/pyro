# Live value ranges in HTML text controls

The frozen standalone RC.5 census identified 189 Chrome-only named passes on all 19 eligible OpaqueRange pages, all strict in signed Chrome. The matched development comparison includes these pages and 20 existing text-selection, Range and StaticRange guards. The selector SHA-256 is bb297c32fab75d7bccce47a7fd9ae52c91b481a2fbc69d8e1bec955e29645159. Both LP arms use the pinned static JSC on macOS arm64. Signed Chrome stable 158.0.8127.37 results come from 37 verified original WPT shards at a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The same signed browser was used for the separate direct controls.

## Behavior

The DOM Standard defines AbstractRange's offset/collapsed getters and gives its NodeRange subclass the node-container getters. Range and StaticRange both extend NodeRange. The signed prototype and receiver controls confirm this distinction. The new OpaqueRange/createValueRange surface in these tentative WPT pages is a proposal, tracked in WHATWG DOM #1531 and HTML #11741. OpaqueRange inherits offsets from AbstractRange, so it does not expose a node container. The current DOM proposal and signed implementation take precedence over the explainer's older wording about null containers.

A supported text control creates independent live ranges over UTF-16 code-unit offsets. Applicable input types are text, search, url, tel and password; textarea is also supported. Offset validation and unsigned-long conversion run through the public bindings. A backwards range collapses to its start. Changes to the control's value and incremental edits update live boundaries without changing normal selection semantics. A range keeps an associated detached control live; the control's registry holds weak references back to ranges. Explicit disconnect or removal from a connected subtree clears the association and offsets. Moving/removing an already-detached control retains its range. Form resets go through the existing event path and execute a native state reset preserving custom validity, selectedness/checkedness defaults and file reset behavior.

This increment supplies value offsets, lifetime and mutation handling. Rendering range rectangles, CSS highlights and hit testing remain separate work. No synthetic rectangles are returned. The archived fixtures also cover interactive input; results for that group are reported separately.

Observable differences are recorded. In the signed build a clean input's defaultValue can shorten its value while a previously obtained range keeps now-out-of-bounds offsets. The implementation follows the open HTML draft's changed relevant-value replacement steps and collapses that range. At the end of a nonempty incremental replacement, the candidate follows the signed browser's after-replacement endpoint affinity (for example, [2,4) replaced with three units maps old endpoint 4 to 5). This differs from the open HTML draft's current inclusive collapse at the old end. Same-content edits preserve offsets. The signed and native reset controls include a cancelable reset, external form-associated input, checked/selected state and validity.

- https://dom.spec.whatwg.org/#interface-noderange
- https://github.com/whatwg/dom/pull/1470
- https://github.com/whatwg/dom/pull/1531
- https://github.com/whatwg/html/pull/11741

## Commands

Use the cache layout in run.py and signed-reference.py. Build each detached revision with the pinned JSC archive in provenance.json, wptiter/render and one compiler job. Each WPT run uses four page workers and the original no-native-retry runner.

    python3 signed-reference.py
    python3 run.py base
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node boundaries.cjs candidate ./pyro
    node lifecycle.cjs candidate ./pyro
    node defaults.cjs candidate ./pyro
    node reset-probe.cjs candidate ./pyro
    node prototype.cjs candidate ./pyro
    node webidl.cjs candidate ./pyro
    ./native.sh
    python3 report.py
    python3 classify.py
    python3 families.py

Each direct control also has an explicit baseline and signed Chrome result. Null status in the named ledger denotes an unobserved occurrence. Strict passing requires a completed page with an OK harness and all observed subtests passing.

The direct raw-text control tests the distinction between direct child text and descendants. Nested-element text and changes beneath that element leave a clean textarea's value, defaultValue and range unchanged. Changing a direct Text node from LF to CRLF changes raw defaultValue but preserves the normalized value and offsets in signed Chrome. The open HTML draft's children-changed text currently tests whether the raw value changed; the measured implementation preserves the normalized API-value behavior. Changing an input from text to search collapses a live range even when the text stays the same in the signed browser. The candidate retains its association for supported types and disconnects when the new type ceases to support ranges.

The first candidate (d015ba4c3) recovered 124 target assertions and one guard with zero losses, but the native JSC numeric conversion accepted BigInt offsets. Signed Chrome rejected primitive and boxed BigInt as well as objects whose valueOf or Symbol.toPrimitive returned BigInt. The final binding uses the JavaScript unsigned right shift in a realm-local private conversion function, preserving Web IDL argument evaluation order, ordinary modulo wrapping and BigInt rejection without changing numeric conversion for other APIs.

The initial textarea.defaultValue setter also bypassed replacement for an identical sole Text child. HTML specifically requires this setter to string replace all. The final change replaces that child, producing the signed mutation record and children-changed steps, including for equal text. Other textContent setters retain their own implementation. The direct defaults control records child identity, MutationObserver records, empty replacement and dirty control behavior. The old native prototype regression failed only its two direct-parent booleans with the updated DOM hierarchy; native-red.log is from the initial candidate. The revised test now checks NodeRange inheritance, transitive AbstractRange inheritance and ownership of the container getter.

Intermediate ac460b046 linked successfully but cached the converter on the Window global. The primary browser rejected that object as a hidden-property holder during an ordinary numeric call, so this executable did not supply the final result. Its raw run and failure receipt are preserved under rejected-cache-holder/. The final correction uses the native per-realm module holder already used by other browser bindings. Ordinary numeric calls, object coercion order and BigInt exceptions are verified before the final WPT run.

## Results

| Cohort | LP parent pass / observed | LP candidate pass / observed | Signed Chrome pass / observed | Strict pages parent / candidate / Chrome |
| --- | ---: | ---: | ---: | --- |
| 19 OpaqueRange URLs | 0/189 | 124/189 | 189/189 | 0 / 11 / 19 |
| 20 neighboring guards | 10,281/10,312 | 10,282/10,312 | 10,306/10,312 | 17 / 17 / 19 |

The candidate added 125 observed named passes, including one neighboring select-event assertion on a disconnected search input. That guard is an observed delta; its cause was not independently established. No baseline pass was lost and no name appeared or disappeared. Every engine completed all 39 pages without a restart or watchdog kill. The candidate recovered all 30 assertions in the interactive and event-order group. Sixty-five target failures remain: 64 on geometry/highlight-family pages and one disconnect fixture requiring geometry. Passing offset/lifetime checks in that family do not establish rectangle or highlighting support.

Seven direct groups matched signed Chrome exactly: primary state/identity, prototype ownership, Web IDL, normalized/nested direct text, form reset, BigInt conversion and textarea default-value replacement. In the two other groups the sole differences are the same current-draft choice for clean input defaultValue: LP collapses prior offsets on a changed relevant value; the signed browser preserves them even when they exceed the new value length.

Thirteen final native checks passed: nine forms value tests, the merged hierarchy/IDL regression, ordinary form reset, form-associated custom element reset and UTF-16 selection behavior on value changes. Independent QA repeated the frozen 39-page corpus on 7d235ed4b, verified both entire ledgers including occurrence keys and strict/lifecycle pages, and reproduced all nine direct-control outputs. The comparison records standalone source revision, tree and executable hashes for the final measurements.

### Clean input defaultValue and range boundaries

The input IDL defaultValue setter reflects the value content attribute. Open HTML PR #11741 explicitly adds full replacement steps when changing that attribute changes a supported control’s clean relevant value. Its full-replacement algorithm uses offset zero, old length and new length, and its range-adjustment rules collapse prior endpoints in the old span to zero. Both of the following direct cases are specified by those proposed steps.

| Clean input case | Candidate | Signed Chrome 158 |
| --- | --- | --- |
| Change defaultValue from abcdef to abXYZef, tracking collapsed offsets 0 through 6 | [0,0,0,0,0,0,0] | [0,1,2,3,4,5,6] |
| Shorten defaultValue from abcdef to ab with range [3,6] | value ab, range [0,0], collapsed | value ab, range [3,6], uncollapsed |

These results are input attribute reflection observations. The separate form.reset control passes in both engines: canceled reset preserves the current value/range, successful reset restores the default and collapses the live changed-value range. The textarea.defaultValue control independently checks the standardized string-replace-all setter: an identical string replaces the Text child (one mutation record) and collapses the clean live range in both engines.

[HTML proposal at the measured head](https://github.com/stephanieyzhang/html/blob/5e7135632eb7d1d09c288b19f78037433d0f950f/source) · [Proposed text-control integration](https://github.com/whatwg/html/pull/11741/files) · [HTML textarea defaultValue](https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue)
