# HTML DOMParser inline template markers

The parser now prepares inline patches in a freshly created, scripting-disabled HTML DOMParser document. A direct child template of the document body can address markers in head. Templates under a nested element retain their local scope. Preparation of eligible parser scripts records their already-started state before the scripting-disabled check, which also preserves it when scripts are imported. XML parsing and Range retain their own paths.

| Selection | Combined base | Candidate | Chrome stable 158.0.8127.37 | Full pages (base/candidate/Chrome) |
|---|---:|---:|---:|---|
| 32 template-for URLs | 138/494 | 172/494 | 475/494 | 7/7/30 |
| 33 parser controls | 778/801 | 779/801 | 788/801 | 27/27/31 |

All 35 gained names pass in Chrome. The candidate has zero named losses, omitted names or URL timeouts on these selections. The guard gain is “parseFromString as HTML, append importNode result”. Five Rust DOMParser tests passed on pinned JSC; six normal scope and lifecycle observations on the candidate equal Chrome. No WPT expectations changed.

**Starting gaps.** Of the starting 337 Chrome-only names, 296 were on multiple-cases.html. Four methods (iframe, DOMParser, contextualFragment, stream) each accounted for 60; the remaining 56 were chunked innerHTML and template setter variants. The WPT splits into equal-length chunks and drops the remainder, so the variants can end in different tokenizer states. This candidate gained 32 multiple-cases names and two unclosed-content names; 303 focus names remain Chrome-only. The complete original distribution lives in starting-gaps.json. Live document writes, streaming and script-activating Range parsing remain open.

**Provenance.** Source base `054ffa4c43fa84bdd609b0a02cac2ccb1bb04760`; candidate `93117170d0781be02321901ce68060e2cb59e40a`. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, macOS arm64, JSC `0e0dd7038a7a-6f4649e395e0`. Base binary SHA-256 `26b68bbb2315c5940ec0f6dd902ff573060fbd07800cc99a1e9df31e91f590a0`, candidate `a822d317711910d1d8f626fad1611a6de033732fb8ca2fa6bd2a6e521fcb7d5b`. The base LP and Chrome focus receipts are reused from the verified combined parser checkpoint c87c2d9af. The Chrome guard and candidate LP runs are fresh. Every recorded URL completed with a successful harness and all runs had zero restarts.

**Commands.** Each fresh WPT run used the no-native-retry runner with the pinned manifest and one of the attached selections. Decompress the raw JSON files and run compare.py to regenerate named rows (URL, name, occurrence and statuses) and page rows.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 2 \
  --tests "$SELECTION" --bin "$LABEL=$BINARY" --out "$LABEL.json"
cargo test --profile wptiter -p pyro-webapi --test integration dom_parser_
node probe.cjs controls-head "$BINARY"
```
