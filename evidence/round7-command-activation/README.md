# HTML button commands, explicit targets and event source

Six historical RC.5 invalid-element-type pages omitted 330 names after a global identifier lookup failed in setup. The verified pinned-JSC parent 59c4ba28f already emits and passes them. It also matches signed Chrome on ordinary bare/global lookup during parsing, dynamic named elements, custom dispatch, dialog opening/closing and cancellation. This batch changes command activation, target reflection and event source semantics. It does not modify Window named access.

The 20 ordinary button targets and four neighboring shadow guards include 730 named occurrences. The frozen unique selector is cc91dd00e1cb81fa04e31c52ace8a67ad1d439ae61c1c9adf2c6dca9a1827c45; prior native fault cases, disconnect regressions, fullscreen, scrolling and media commands remain excluded. WPT is pinned to a421f2fea5eb7f91e0db399e44b0fb0779ee1603; signed stable Google Chrome 158.0.8127.37 uses the original receipts indexed by chrome158-map.json. The exact parent is the shared editing binary (249180360d7d21c0a5f1c92c0e9512c9705bbb052ffb79a9a87facbd1ddc8ad1); source, JSC and output digests are in provenance.json.

Ordinary built-in popover commands fire a cancelable command event on a valid HTML target before checking the actual popover state. CommandForElement honors the invoker's tree scope. Authored same-value empty writes and removals clear the reflected explicit target; unrelated namespaced writes preserve it. When the command drives dialog state, toggle events retain the correct invoking button. The HTML Standard defines the source getter by retargeting against currentTarget. Signed Chrome and pinned WPT set composed=true for trusted activation, although the retrieved HTML activation steps explicitly initialize cancelable and source and omit composed. A newly constructed event keeps its ordinary EventInit default. The source-associated path behavior is verified by ordinary signed controls and WPT; the HTML Standard references open DOM issue 1328 for associated event data. See [button activation](https://html.spec.whatwg.org/multipage/form-elements.html#the-button-element), [CommandEvent](https://html.spec.whatwg.org/multipage/interaction.html#the-commandevent-interface) and [DOM issue 1328](https://github.com/whatwg/dom/issues/1328).

Direct controls exercise parsed/dynamic named element lookup, valid/inapplicable/canceled commands, attached/detached invokers, synthetic and trusted composed state, lexical scope of reflected targets, and per-listener source identity with actual capture paths. A source inside the same shadow root, including the target itself, or equal to its host, is distinguished from an unrelated outside peer or a null source. Both CommandEvent and ToggleEvent are checked. The reflection control performs IDL assignment, attribute lookup, empty and same-value null-namespace writes, removal and a separate namespaced attribute.

## Matched results

| Cohort | Parent named | Candidate named | Signed stable named | Strict pages parent / candidate / signed |
|---|---:|---:|---:|---|
| 20 targets | 671/718 | 706/718 | 706/718 | 13 / 18 / 18 |
| Four existing shadow guards | 2/12 | 12/12 | 12/12 | 0 / 4 / 4 |
| Union | 673/730 | 718/730 | 718/730 | 13 / 22 / 22 |

All 45 gains recovered previously observed failures. Final LP and Chrome pass sets agree by URL, named occurrence and strict page. All 24 pages completed with no native restarts or watchdog kills. No names or prior passes were lost. The remaining common nonpasses are eight tentative details command assertions and four tentative number-input assertions; no unsupported commands were added to alter their results.

All four direct receipts match signed Chrome byte for byte. The event source remains the inner button to a listener in the shadow root and retargets to its host outside or after dispatch. Both tested event classes stop at the shadow boundary where the target and non-null source converge on the same host, even if the original target equals its source. An unrelated outside peer or null source permits the composed event to cross normally. Default-action and constructor observations are reported independently of path trimming.

Use acquire.sh, run.py and report.py with the original selector and matching binaries; native.sh lists the exact Rust integration filters. Raw initial results are retained under initial/. The first candidate still had one shadow path difference and the stale explicit-reference behavior. Independent final evidence d970231272b45bfe7703b42bc1accdcaeca0ed1a confirms the 24-URL run and the four signed comparisons on executable 44439ff8166ad9b2754201ca70716bfa7e0b82298a5cf2613f223262d9f1599e.

All 10 focused Rust integration tests passed for event constructors, retargeting and path convergence, explicit-target invalidation, invoker event ordering, dialog actions, accordion behavior and neighboring popover APIs.
