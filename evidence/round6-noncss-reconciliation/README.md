# Round 6 non-CSS reconciliation and conformance batches

The reconciliation topic is based on upstream 572c16d41b03064bb89d3ed40722d0eb46d2c7fd. It includes all 30 commits from the non-CSS range e4ca26f48..345acc12d and one Window-associated document lifetime correction. It is published at 97378f212759636a841aa25c19169bcbf91f1cb9. The 236 paths changed only by upstream and the 62 paths changed only by the original non-CSS series match their respective source trees byte-for-byte. The seven overlapping paths are recorded in latest-source-proof.json; the sole non-identical patch presentation is vendor diff context. The vendored patch round-trip checks passed. No expectation or Wasm backend was changed. [Reconciliation branch](https://github.com/povio/pyro/tree/topic/round6-noncss-reconciled-current)

A retained Window now keeps its associated Document alive. Before this fix, five assertions in DOMParser-parseFromString-url-moretests.html failed in two full guards while passing in three isolated runs. On the corrected tree all 118,957 named statuses and all 330 page statuses matched the previous non-CSS head. On 1,038 CSS pages all 35,514 named statuses and all page statuses matched the CSS upstream head, with 814 fully passing pages. A further 41 worker/UTF-16 variants gained five named passes with zero losses.

## New ordinary browser behavior

The first five commits run from 97378f212 to 37aba62ab. They implement worker FileReaderSync with shared MIME decoding, structured chunk copying for Response.clone, editable paragraph/list indentation, Server-Timing response metrics, and MathML focus/event reflection. They gained 1,631 named passes and lost one on 505 matched pages. The last commit at 902fe811f recognizes the XHTML Mobile 1.1 and 1.2 public identifiers using the built-in named character reference table. That change independently gained 4,240 named passes on 12 XHTML pages (25,606/25,606 final, zero lost). It does not fetch DTDs. [First batch](https://github.com/povio/pyro/tree/topic/round6-noncss-conformance-batch) · [Cumulative topic](https://github.com/povio/pyro/tree/topic/round6-xhtml-mobile-entities)

| Same 517 non-CSS URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| Reconciled LP | 135,426 | 240 |
| New LP cumulative topic | 141,296 | 261 |
| Local Chrome stable | 143,082 | 355 |

The cumulative result gained 5,871 named passes and lost one. It registered 411 new names and stopped reporting 72 names. The only lost pass was the second message-generated case on navigation-timing-trickle.https.html. Before the implementation the first case failed and a second one passed. Afterward LP and Chrome both reported a single passing case. The main CSS guard on the cumulative feature topic recorded one timing loss (0.1 ms) on transition-ready-time-offscreen.html, with zero added/missing names. In five alternating isolated repeats the reconciled control passed 0/5 and the feature candidate passed 2/5. The 41-page supplement was unchanged. All comparison results retain these original outcomes.

Server-Timing covers final response headers and the timing-allow-origin decision in a secure client context. HTTP trailers and early pre-EOF notifications remain outside the measured implementation. The normal public ReadableStream.tee behavior is preserved; native network-body cloning and broader byte-stream tee remain separate gaps. Autofocus processing in the new MathML batch is measured during DOMContentLoaded. The editing commands preserve live Range selection through ordinary edits; the pre-existing undo snapshot path recreates nodes and selects forward. The browser's existing default 64 MiB per-read budget applies to FileReaderSync.

## Reproduction and provenance

All WPT runs used a421f2fea5eb7f91e0db399e44b0fb0779ee1603 with JSC 0e0dd7038a7a-6f4649e395e0. Chrome was stable 157.0.8103.80 on macOS 27.2.1 arm64, using the WPT profile from the published census. All builds used the local wptiter profile. The non-CSS runs used four jobs; CSS used two. Results here are scoped guards. The earlier complete non-CSS census retains its original LP checkpoint and raw named comparison, including the 292 audited named losses. The end-to-end parity gap remains open.

Set WPT to the pinned full checkout, MANIFEST to its upstream manifest, BIN to an absolute browser executable and OUT to a result path. Use the verified manifest runner for the non-CSS corpus. [Runner](https://github.com/povio/pyro/tree/topic/round6-reviewed-testharness-census) · [Prior census/remaining gaps](https://github.com/povio/pyro/tree/evidence/round6-noncss-census/evidence/round6-noncss-census)

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --dirs non-css \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --tests final-selection.json \
  --jobs 4 --bin "lp=$BIN" --out "$OUT"
```

The CSS main selection is urls.json and the supplemental selection is supplement.json. The main run uses --dirs css --server wptserve --testdriver --generated-globals "$MANIFEST" --jobs 2; the supplemental run adds --manifest-inventory. The original and rebuilt binaries and hashes are recorded in provenance.json and binary-hashes.txt. The JSONL tables use cohort, URL, original name, one-based occurrence, base status and head status; the non-CSS feature comparison also includes Chrome. Missing observations are null. See summary.json for status counts and source hash references. No pull request or merge was made.
