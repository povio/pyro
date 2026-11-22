# Interest invoker reflection and InterestEvent

The frozen 50-page cohort contains 44 eligible interest URLs, three reference-target reflection matrices and three neighboring command/popover/event-path guards. Selector SHA-256 is 892f376c212e1151685bcc65de3856f494f32a30ca131f21182d8588fa770dda. See provenance.json for pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, manifest, JSC and browser identities. Signed Google Chrome stable 158.0.8127.37 results come from 48 verified four-job shards. Direct controls use the same signed executable.

The integrated parent ed9324b9ddc19263c963a20799378a980acee814 differs from the preceding reference-target executable in compiled Streams and WebAudio sources. The parent was rebuilt exactly with pinned JSC, wptiter/render and one compiler job. Four page workers used the existing no-native-retry runner. Of 286 baseline signed-only passing occurrences, 45 were absent from LP because the property matrices gate assertions on button.interestForElement. The other 241 had observed non-pass statuses.

Attribute ID lookup follows the invoker's tree. Explicit IDL assignments use generation-checked weak targets revalidated against the invoker's shadow-including ancestors. Corresponding attribute writes, including repeated empty values, clear explicit references. The property is installed on HTML button, anchor and area and SVG anchor. The host remains the reflected identity even when its shadow reference target becomes invalid. Native tests cover cloning, adoption, lifetime and attribute updates.

Window-exposed InterestEvent validates its init dictionary and optional Element source, inherits from Event and participates in native event wrapping. The source getter retargets against the current observer. Dispatch uses the existing related-target algorithm to prune boundaries where source and target retarget to the same host. The binding checks DOM-store identity before accepting a source key. Same-origin frames share a DOM store. Direct controls cover source identity in both directions across a same-origin frame, dictionary conversions, ordinary Event propagation and null, distinct and coincident sources.

The hover-activation cohort remains separate from author-dispatched events and from IDL reflection. The three assertions in shadow-dom/reference-target/tentative/interestfor.tentative.html require input-driven interest and popover behavior. The families.json table separates these results from the matrix and event-interface assertions.

## Reproduction

Use the cache layout referenced by the scripts. Build each detached source with build-exact and pinned JSC. Preserve the parent as base-pyro and candidate as pyro.

    python3 signed-reference.py
    python3 run.py base
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node event-probe.cjs candidate ./pyro
    ./native.sh
    python3 report.py
    python3 classify.py
    python3 families.py

Raw JSON, SHA-256 receipts, exact occurrence-indexed named results and page ledgers accompany the commands. Null denotes an unobserved occurrence. Strict passing requires completion, an OK harness and all observed named assertions passing.

## Results

| Cohort | Parent LP pass/observed | Final LP pass/observed | Signed Chrome pass/observed | Strict pages parent / final / Chrome |
| --- | ---: | ---: | ---: | --- |
| 47 targets | 6,218/6,467 | 6,299/6,512 | 5,819/5,837 | 5 / 8 / 34 |
| 3 event and invoker guards | 8/18 | 8/18 | 18/18 | 2 / 2 / 3 |

Of the 81 added named passes, 36 were failures already present in the parent (18 IDL harness, six reflection interface, 12 interest event interface/dispatch). All 45 newly observed names passed in the three gated reflection matrices. All 81 were signed-only passes in the parent. No pass was lost and no observed occurrence vanished. There are 205 remaining signed-only names (195 targets and ten event-path guards) and 675 LP-only matrix passes. These figures include all three unchanged reference-target hover failures and two shadow-event fixtures requiring trusted hover generation. The own-host direct fixture separately tests author dispatch on a host with its own shadow button as the source.

Both LP revisions retained four harness errors, including the neighboring event-path guard. Chrome retained three harness errors, the focus/hover basic behavior variants and keyboard behavior. No watchdog kill, restart or timeout occurred. The browser-specific matrix denominators remain visible in the named ledgers.

Reflection-only source a76e2be32 was independently reviewed. The staged matrix/interface observations come from the same measured combined executable. A distinct intermediate executable at 63385054d passed the 50-page comparison and the two original direct suites, but failed the added own-host author-dispatch probe. Signed Chrome delivered to document capture and host on that probe. Final d47fe3095 retargets the initial dispatch seed before pruning. This preserves its original source for the getter and for redispatch. See prior/ for the intermediate result. Nine native tests cover the two reflection regressions and seven existing event dispatch controls.

Independent QA reproduced both final ledgers and all three direct suites in a fresh directory on the digest-verified executable. A fourth control reused the same event over host, inner target and host again. Its callbacks and source identity matched signed Chrome on all three dispatches. See qa/ for independent observations and the redispatch probe.
