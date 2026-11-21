# Disposition of the pinned inline-if cycle loss

Keep the current implementation and count `css/css-values/if-cycle.html`, subtest `CSS Values and Units Test: CSS if() function cycles 4`, as a loss against the original baseline. This is a reproducible difference between pinned Chrome, current upstream WPT, and the current editor’s draft.

The test sets `--prop: if(style(--prop): var(--prop); else: value)` and expects an empty computed value. Upstream WPT at the inspected HEAD contains the exact pinned file, with SHA-256 `61e95f22f70ce20a9a14e09554fb707d0161ca2a01bd4a71721d8d7d8c7356f9`. GitHub’s path history reports the last change on 2025-05-15. [Pinned test](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/css/css-values/if-cycle.html#L64) · [Upstream test](https://github.com/web-platform-tests/wpt/blob/9986bb32c9e97707eff870ab284ebc5cc7ae8d21/css/css-values/if-cycle.html#L64) · [Last test change](https://github.com/web-platform-tests/wpt/commit/649319c229a6037ad25250a4f006bf605e986e81)

The exact Chrome 158.0.8127.37 executable used for the inventory passed all 46 assertions on the unmodified pinned page. A separate fresh-document readback distinguishes a guaranteed-invalid value from a valid empty token stream by substituting `var(--prop, fallback)`.

| Expression assigned to `--prop` | Chrome computed / via var | Pyro computed / via var |
| --- | --- | --- |
| `if(style(--prop): var(--prop); else: value)` | empty / `fallback` | `value` / `value` |
| `if(else: var(--prop))` | empty / `fallback` | empty / `fallback` |
| `if(style(--absent): never)` | empty / `fallback` | empty / empty |

The current CSS Values 5 substitution algorithm explicitly makes a style query false when guarding its referenced property would mark it cyclic. Here that selects `else: value`. The unused `var(--prop)` branch is never substituted. In the second row the selected value itself is cyclic. In the third row the draft returns an empty token stream because no condition matches. Both the published editor’s draft and its source contained this algorithm when checked. [Draft algorithm](https://drafts.csswg.org/css-values-5/#if-notation) · [Immutable specification source](https://github.com/w3c/csswg-drafts/blob/804b7b0055e80dfaba091e7f32a4bca303e569e3/css-values-5/Overview.bs#L1733-L1765)

The original Pyro baseline passed subtest 4. The candidate fails it under the pinned expectation. The differential keeps both results and applies no expectation or inventory adjustments. The adjacent evidence files contain the per-engine readbacks, the original and candidate assertion, the unmodified Chrome page report and the verified reference executable hash.
