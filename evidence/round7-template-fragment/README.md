# Local HTML content patching in detached fragments

An inline template with a for attribute can replace the content between named processing-instruction markers. The HTML tree builder searches the adjusted insertion scope. In detached fragment parsing this can be the parser's own output fragment. The browser already supported these local patches in legacy HTML setters and inert DOMParser HTML documents. The implementation enables the same algorithm in HTML Range.createContextualFragment and in modern insertion methods with their default script setting and no parser filter.

A template can only find markers in the output being parsed. In the direct isolation control a matching local patch succeeds, an unmatched external name remains an ordinary template, and the original caller subtree is unchanged. The runtime continues through the existing XML, editing, filtering, external-resource and script handling paths. Chrome applies the inline fallback in the direct external-src case; LP retains that template and its content under its existing external-resource policy.

- https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment
- https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inhead
- https://html.spec.whatwg.org/multipage/scripting.html#prepare-content-patching

## Matched results

The selector contains all 47 eligible template-for pages and three neighboring DOM Parsing guards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Its SHA-256 is 2e230e3f009b27aef62a8cde53bae03463e6a1f82eb140cce418753a0103de90. Both LP revisions used the pinned static JSC, wptiter/render and four page workers. Signed Chrome stable 158.0.8127.37 is projected from 46 verified original shards. The two direct-control groups used the same signed executable.

| Cohort | LP base pass / observed | LP candidate pass / observed | Signed Chrome pass / observed | Strict pages base / candidate / Chrome |
| --- | ---: | ---: | ---: | --- |
| 47 template-for pages | 177/519 | 216/519 | 499/519 | 9 / 10 / 44 |
| 3 legacy Range guards | 36/40 | 36/40 | 40/40 | 1 / 1 / 3 |

All 39 gains were existing observed failures that signed Chrome passed. There were no lost passes, newly observed names or disappeared names. Thirty-two gains were Range cases in the multi-method scenario matrix, and seven were the local-target assertions for Range and six modern setters. All three runs completed all 50 URLs with zero restarts or watchdog kills. The candidate remains 283 named passes behind Chrome on the targets and four on the guards. Remaining cases include live document writes and streamed patching. Some multi-chunk fixtures split with integer division and omit trailing source characters; the resulting EOF/partial-end-tag marker behavior is a separate gap.

The native Range test previously asserted that its local template remained an ordinary element. With the revised replacement assertion and the unmodified parent product (f58a8a7956), it failed with both the original marker and template still present. The native-red.log records this result. The test now verifies the resulting replacement and caller isolation, while its editing and external-template assertions remain. Candidate commit 67130f6d5 has the same production code as 1364be30c with a stronger test of caller isolation during a successful local replacement. Eight native fragment context tests and the existing inert DOMParser regression passed. The original WPT fixtures and their expectations retain their contents.

## Reproduction

Use the paths in run.py and signed-reference.py, and preserve each executable and result separately. Build the parent 2c7fabcc6 and candidate using build-exact with the cache key and archive SHA-256 in provenance.json. For the native red check use f58a8a7956 and the Range test selected in native-red.log.

    python3 signed-reference.py
    python3 run.py base
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node scope.cjs candidate ./pyro
    ./native.sh
    python3 report.py
    python3 classify.py

Each named ledger row retains URL, name, one-based occurrence and three statuses. The page ledger preserves full lifecycle, harness and strict-page outcomes.

Independent runtime QA repeated the final candidate on the same 50 frozen URLs using the archived parent and signed reference. Both the complete named occurrence series and strict/lifecycle page ledger matched. The two direct-control outputs matched the primary candidate, including the preserved external-src behavior. See qa/ for the independent raw receipt and readback.
