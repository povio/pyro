# Shadow reference targets and HTML form ownership

The pinned WPT checkout describes reference targets as a tentative proposal. The comparison includes all 16 eligible reference-target pages in V2 and six ordinary label/form guards. This cohort is part of the non-CSS census.

The exact development parent is 00e529058fdf22677867732ab8e00e46de89a76c. The macOS arm64 signed stable Chrome 158.0.8127.37 projection comes from the recorded four-job shards at 01f49f43da24a20ecb20d99c967fc5de54272120. The selection SHA-256 is ca62d121faad2b3f8b58f8e6f83b2a4d949afdefd727e2d712a734b9696be906. The revision, manifest, launch settings, Chrome executable and JSC fingerprints are in provenance.json and chrome158-map.json.

At the development parent, 245 target occurrences passed only in signed Chrome. Two baseline diagnostics directly identified a missing interface, 108 contained other assertion failures, and 135 names were absent. Three property checks in the WPT helper accounted for the missing names, 45 each for object.form, legend.form, and button.interestForElement. See named.jsonl.gz for exact URL, name, occurrence and status in baseline, final and signed Chrome order. Null denotes no observation. Each pages.json tuple records the harness outcome and strict all-named-pass result.

The first implementation (5e16a4e93) stores nullable reference-target IDs on roots. Native lookups follow nested targets dynamically across open and closed roots, and return no target for dangling IDs. Form/label/list getters expose the host across scope boundaries. Native form/list collections and activation operate on the resolved element. Template creation, cloning and serialization preserve the ID. The final increment supplies the standard HTMLObjectElement.form and HTMLLegendElement.form getters. The latter consults its immediate HTML fieldset parent on each access. DOM node header size remains covered by the native compactness test.

The direct controls cover nested closed/open targets, rename and removal, invalid ID, ordinary datalist filtering, form and label identity, command activation, custom-element association callbacks, native AX names, form getter reparenting and dynamic fieldset owners. Signed Chrome returns the host from ElementInternals.form but passes the resolved inner form to formAssociatedCallback. HTML's form-associated custom element algorithm supplies the new form owner to that callback. The tentative explainer preserves existing FACE behavior. Native CDP AX tests use Accessibility.getFullAXTree. The frozen WPT testdriver's computed-label path uses Playwright's accessibility engine, which has separate results.

- https://github.com/WICG/webcomponents/blob/gh-pages/proposals/reference-target-explainer.md
- https://html.spec.whatwg.org/multipage/custom-elements.html#form-associated-custom-elements
- https://html.spec.whatwg.org/multipage/form-elements.html#dom-legend-form
- https://html.spec.whatwg.org/multipage/iframe-embed-object.html#dom-object-form

## Reproduction

The commands assume the caches and layout in run.py. Build each detached tree with the pinned JSC using build-exact, CARGO_BUILD_JOBS=1 and PYRO_BUILD_JOBS=1. Archive the parent as base-pyro and candidate as pyro. Use separate output folders for repeats. Each WPT invocation has four page workers and no native retry.

    python3 signed-reference.py
    python3 run.py base
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node form-probe.cjs candidate ./pyro
    node aria-probe.cjs candidate ./pyro
    ./native.sh
    python3 report.py
    python3 classify.py

The same three direct probes ran against the archived baseline executable and signed Chrome. The pinned expectations and WPT fixtures retain their original contents. Known native faults and all other held cases remained outside the eligible corpus.

## Outcomes

| Cohort | Parent LP pass/observed | Final LP pass/observed | Signed Chrome pass/observed | Strict pages, parent / final / Chrome |
| --- | ---: | ---: | ---: | --- |
| Reference targets | 6,025/6,176 | 6,222/6,266 | 5,595/5,636 | 1 / 9 / 12 of 16 |
| Label and form guards | 31/34 | 32/34 | 34/34 | 3 / 4 / 6 of 6 |

The final executable recovered 107 already-observed target failures and one guard failure. Both HTML form getters made another 90 target names observable, all passing. No previously passing occurrence was lost and no baseline occurrence disappeared. Of the parent’s 245 signed-only target occurrences, 185 now pass. Sixty remain, including 45 names gated by the missing interest invoker interface. The different browser surfaces also produce 687 final-LP-only target passes. Both LP runs recorded an event-path harness error; signed Chrome timed out on label-descendant. Neither run restarted or recorded a watchdog kill. These results measure 22 pages only.

Ten final native tests passed, including the 64-byte node guard, eight association tests and the clone/serialization regression. The first clone fixture mistakenly used the document parser mode that disables declarative shadows. Its corrected setup follows the existing declarative parsing test; the original failure is archived under first/. All three final direct control receipts match their signed Chrome counterparts. In particular the native accessibility tree uses the designated name, clears it for a missing reference and restores it after mutation.

An independent four-worker final executable run in a fresh directory reproduced the complete named-status ledger (including occurrence), strict page outcomes, cohort counts and all three direct controls. It reused the digest-verified parent and signed projection. Its raw results and receipts are under qa/.
