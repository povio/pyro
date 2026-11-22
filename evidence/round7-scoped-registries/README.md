# Scoped custom element registries

The frozen selection contains 40 custom-elements/registries URLs and all 157 other eligible custom-elements URLs as legacy guards. Both groups use the same manifest and signed reference. The original census identified 372 Chrome-only named occurrences in the registry directory. No withheld variant, native fault reproduction, expectation or WPT source change is included. The signed Chrome results come from 150 original four-job shards at ee24e07f6. Every selected page was observed. Four original Chrome harness errors and their emitted names remain in the comparison.

## Reproduction

The scripts use the sibling cache layout identified in run.py and import-reference.py. Build the exact base and feature trees with the pinned CI JSC static archive and wptiter profile described in provenance.json. Place the resulting executables at base-pyro and pyro, respectively.

    python3 import-reference.py
    python3 run.py base
    node initial-probe.cjs base ./base-pyro
    node probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    python3 report.py
    python3 ledgers.py
    ./native.sh

Each named row contains URL, original subtest name, one-based occurrence, then base/candidate/Chrome statuses. Null denotes no observation. Page rows record outcome, harness status and strict-pass flag. A strict page completes with an OK harness and passes every name in a nonempty set. scope-scores.json additionally retains every non-OK page's setup error and separate scoped/legacy denominators. Signed-reference maps preserve exact URL/source/environment, original outcome and producing shard; every source shard digest is included. No per-selection signed Chrome restart or watchdog count is available.

The source keeps definition/waiting state per registry and realm. Scoped constructors, callbacks and unresolved whenDefined values are traced from their JS registry wrapper. Nodes and documents preserve scoped identity through creation, shadow attachment, clone/import, parsing and adoption; sparse per-node data provides the non-default association. Registry-scoped documents are recorded when needed in insertion order. Detached associated foreign DOM arenas receive definition metadata before connection. Definition ownership is recorded separately from an element's current registry, so callbacks continue to refer to the definition under which it was constructed.

The signed direct fixtures cover distinct registries with a shared name, whenDefined, creation options, shadow parsing, explicit-null initialization, connection/reaction order, inert document import, cross-document adoption, cross-frame definition order, isolated shadow-disable flags, and a component workload using 48 independent registries. Supplemental lifecycle controls exercise retaining a child-frame registry and a late definition for a detached auxiliary-window host.

The pinned generic HTML/XHTML customelementregistry attribute tests predate the current parsing algorithm. HTML creates elements using the registry of the intended/adjusted parent. XML delegates to HTML's token-creation steps. The declarative shadow template has an explicit shadowrootcustomelementregistry attribute that preserves a null registry; normal elements have no corresponding generic attribute in that algorithm. These URLs remain in both engine ledgers. [HTML parsing](https://html.spec.whatwg.org/multipage/parsing.html#create-an-element-for-the-token) · [HTML XML parsing](https://html.spec.whatwg.org/multipage/xhtml.html#parsing-xhtml-documents) · [DOM cloning](https://dom.spec.whatwg.org/#concept-node-clone)

## Final outcomes on 682d66d2

| Group and engine | Named passes / observed | Strict pages / selected | Harness outcomes |
|---|---:|---:|---|
| Scoped LP base | 2,056 / 2,323 | 4 / 40 | 35 complete, 5 errors |
| Scoped LP feature | 2,432 / 2,456 | 36 / 40 | 40 complete |
| Scoped Chrome stable 158.0.8127.37 | 2,426 / 2,456 | 35 / 40 | 39 complete, 1 error |
| Legacy LP base | 1,586 / 1,890 | 92 / 157 | 152 complete, 5 errors |
| Legacy LP feature | 1,664 / 1,890 | 107 / 157 | 152 complete, 5 errors |
| Legacy Chrome stable 158.0.8127.37 | 1,775 / 1,892 | 137 / 157 | 154 complete, 3 errors |

The scoped group gained 376 names, including 243 previous failures and 133 newly observable passes. Chrome passes 349 of the gains and fails 27. The legacy guards gained 78 previous failures, all passing Chrome. No group lost any previously passing or observed name. The combined feature has 4,096 passes against Chrome's 4,201, with 137 Chrome-only and 32 LP-only. Both WPT runs and the signed projection have zero page timeouts. No LP run restarted or killed a browser.

All five original scoped setup errors recovered. The same five legacy page URLs retained a harness error on both LP builds, with the exact messages and named observations in scope-scores.json. The three missing HTMLSubmitButtonBehavior setups also fail in Chrome. The two other LP setup errors involve form-disabled callbacks and parser construction. The intermediate 3d8ecf6d source lost a completed harness on Document-createElementNS-prefix-timing.html without losing a previously passing named assertion. Final source fixes fresh construction vs upgrade and applies the namespace prefix after validation. All three prefix timing names pass and the page completes strictly. The inert SVG :defined name was lost on the 89cd90a9 precursor and passes again at release. The two precursors retain their own scores, diagnostics, binary identity, direct results and native receipts.

All 16 main structured DOM controls match the signed reference. The supplemental retained-frame fixture also records a divergence. When the parent document builds with a constructor extending the iframe's HTMLElement, the candidate reports the wrong-document constructor result and returns a fresh failed element. Signed Chrome returns an instance of the foreign constructor. Both retain the registry identity and definition after removing the frame. The positive candidate-only control constructs in the iframe's own document, adopts into the parent, then removes the frame: constructor identity, registry identity and instanceof are all preserved. Both engines reject a shadow root on a detached foreign host after its associated registry receives a shadow-disabled definition. These normal probes are separate from the WPT scores. [DOM create an element](https://dom.spec.whatwg.org/#concept-create-element) · [HTML element constructors](https://html.spec.whatwg.org/multipage/dom.html#html-element-constructors)

The final independent source review covered the captured definition, realm and scoped wrapper during constructor callbacks, fresh autonomous construction, prefix timing and failed-constructor behavior. Ordinary document/scoped definitions are preserved at enqueue. Multiple CDP isolated-world global definitions retain the engine's existing main-world-first fallback; this fixture does not test competing definitions across isolated worlds.

The implementation run and independent QA used 682d66d2. Release source abc384c38 changes only the new native test's setup to use a detached div. The original HTML context created implied head/body siblings; the corrected fixture directly tests the two intended custom elements. No test assertions changed. The 17 WebAPI and eight DOM adoption guards passed on 682d66d2. All seven DOM reaction guards passed on abc384c38. The repository's existing self-inserting recursion case was outside the selected native controls. The 197 official WPT URLs remain intact.

Independent QA repeated all 197 URLs using the measured candidate binary and compared both scoped and legacy groups against their original baseline and signed reference. All occurrence-level name and page rows match exactly between the two independent candidate runs. All 16 primary direct controls and the supplemental lifecycle results also match the owner's measurements. The independent run's distinct receipt digest is in qa/summary.json.
