# Named iframe form submissions

Across 85 matched ordinary file, form and iframe URLs, LP went from 314 to 575 named passes, gaining 261 with zero lost or newly missing names. Fully passing pages rose from 60 to 74. Local Chrome stable passed 584 names and 80 pages, with 13 Chrome-only names and four LP-only names. All 64 named FileAPI file upload cases on the seven named-target form pages now pass. These results apply to the selected URLs.

The control b26c9619e2b8f9dde519a7a714d91829d051174a already sent the expected multipart POST and loaded its response into the named iframe. Its load listener observed the preceding initial empty document after form.submit(). The first source commit now completes an initial about:blank iframe and dispatches its element load before append returns. Parent load waits for pending iframe attribute navigation. The second source commit performs newline normalization when serializing entries, so script observes original names and values. Multipart filenames retain their original newline bytes before header escaping. The combined topic is cdc2323a2db447c5d392b7c87449d43b1995c7e4. [Source](https://github.com/povio/pyro/tree/cdc2323a2db447c5d392b7c87449d43b1995c7e4)

The independent same-origin diagnostic posts one ordinary form per frame setting. The control, candidate and Chrome sent all four requests. The candidate matches Chrome's initial event and readyState ordering for missing src, empty src, explicit about:blank, and an ordinary network src. The parent-order diagnostic appends a blank iframe then sets its src in the parent script. A draft allowed the parent to complete before the child's network response in three of three runs. The final implementation and Chrome both delivered the child response before parent load in all three runs. In extra URL probes, Chrome stable agreed on the about:blank query case and differed in synchronous load behavior for a fragment, whitespace-only src and a parse failure. LP implements the HTML shared initial attribute-processing rules. [HTML iframe processing](https://html.spec.whatwg.org/multipage/iframe-embed-object.html#shared-attribute-processing-steps-for-iframe-and-frame-elements)

Both LP binaries use CI JSC 0e0dd7038a7a-6f4649e395e0, arm64 on macOS and WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Chrome observations reuse this same revision and the local stable WPT profile (157.0.8103.80). Independent review covered initial URL classification, parent readiness and serialization boundaries. Both runs used four jobs. This ordinary selection excludes crash and reentrant fixtures. [Frozen Chrome reference](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census)

The named table records URL, original name, occurrence, LP control, LP candidate and Chrome. Page rows contain the same three engines' page status, harness status and fully-passing flag. Missing observations are null. Summary suffixes 0, 1 and 2 refer to those engines respectively.

```sh
bun bench/wpt/run.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests urls.json \
  --bin "lp=$BIN" --out "$OUT"
```

All 85 URLs have reports on each browser. Control LP had two timeouts and one harness error, final LP had two timeouts, and Chrome had one timeout. The per-name counts retain observations made before those timeouts.
