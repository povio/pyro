# Ordinary non-CSS conformance follow-ups

All 1,475 selected page URLs were observed on the frozen 902fe811f control, cumulative LP b26c9619e2b8f9dde519a7a714d91829d051174a, and same-revision local Chrome stable 157.0.8103.80 on macOS arm64. Tests used WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 and LP CI JavaScriptCore 0e0dd7038a7a-6f4649e395e0. The original Chrome receipts came from the [published census](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census). The separate full census measures its own frozen head.

| Matched browser | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP 902fe811f | 149,885 | 496 |
| LP b26c9619e | 154,612 | 528 |
| Local Chrome stable | 165,123 | 1107 |

Raw original-name comparison recorded 4,730 gains and 3 losses. The full lost-name list and page/harness status counts are in wave-summary.json. Reports retain missing names and duplicate occurrences. The two compressed tables store URL, name and occurrence (named table only), followed by the three browser values. Page values contain the outer status, harness status, and fully passing flag.

Editing now preserves inherited white-space and inline formatting when joining editable lines. The separate all-editing comparison gained 4,263 named passes across 710 URLs. [Paragraph join](https://github.com/povio/pyro/tree/9ad015f2f8ccad956505c00a1370bc7e491d8b9c/evidence/round6-paragraph-join-whitespace) · [Inline formatting](https://github.com/povio/pyro/tree/9f5d97cd2558eaf7cad46d52e09edfc0fe2c3590/evidence/round6-editing-join-inline-lines)

Observable and Subscriber now use the existing worker realm module holder. The isolated 48-page comparison gained 237 named passes with zero losses; two async-iterator getter timing assertions fail in both browsers. [Worker results](https://github.com/povio/pyro/tree/25e1b264daeb03eb739d8d0afcf9ac796ec6d351/evidence/round6-observable-workers)

StereoPannerNode applies equal-power panning to mono or stereo audio, driven by the pan AudioParam. ChannelSplitterNode routes discrete mono outputs to downstream ports and AudioParams, including selective disconnection. Panner-only results on the entire 348-page ordinary Web Audio selection gained 134 and lost three names. The three original names contain the runtime-generated exception text for a missing constructor, and the new constructor tests pass using the implemented interface. Numeric probes cover both stereo channels, invalid float inputs, NaN modulation, silent ports and multiple connections to the same parameter. Web Audio still has missing processing nodes and context capabilities visible in the named rows.

The guard uses the reviewed manifest runner with four jobs. The selection includes all ordinary editing, Web Audio and DOM Observable URLs, the previous conformance guard and its reconciliation selection. Run from the verified runner checkout with WPT, MANIFEST and BIN set to the pinned source, manifest and executable.

```sh
bun bench/wpt/run.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests cumulative-urls.json \
  --bin "lp=$BIN" --out "$OUT"
node numeric.mjs
node splitter.mjs
```

The numeric probes read the shipped module relative to the browser source checkout. Test expectations and shipped dependencies were preserved.
