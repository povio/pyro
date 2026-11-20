# Fetch body MIME extraction

LP gained 1,548 named passes and lost zero across 34 pinned WPT pages. The two MIMESniff Window and worker pages gained 773 each, and an adjacent XHR page gained two. Every process completed with zero missing names or restarts. Fully passing pages and named passes measure different outcomes on this selection.

| Browser | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP base | 1,552 | 23 |
| LP candidate | 3,100 | 23 |
| Chrome stable | 1,942 | 31 |

Per MIMESniff page the candidate passes 940 of 942 Request/Response cases. The two remaining inputs contain an unquoted comma. WPT expects a single MIME value in those cases, while Fetch splits header values at unquoted commas before extracting a MIME type. Existing Blob/File constructor outcomes are separate, with 353 passing and 602 failing per page. Request's existing streaming upload limitation also remains.

The supplemental probe covers repeated Content-Type values, charset inheritance, quoted commas, escaped quotes, case-sensitive and duplicate parameters, ignored text after a closing quote, multipart boundaries, cloned headers, streamed Response form data and MIME packaging time for buffered and null bodies. The candidate passed 35/35, the base passed 4/35 and local Chrome passed 5/35. The probe expectations follow the Fetch and MIME Sniffing algorithms and its counts are excluded from the upstream WPT totals. [Fetch MIME extraction](https://fetch.spec.whatwg.org/#concept-header-extract-mime-type) · [Fetch consume body](https://fetch.spec.whatwg.org/#concept-body-consume-body) · [MIME Sniffing parser](https://mimesniff.spec.whatwg.org/#parse-a-mime-type)

## Reproduction

The upstream selection ran from the pinned WPT checkout at four jobs. The Chrome reference reuses unchanged original named rows from the corrected-profile same-pin census. Probe runs used the same local Chrome executable and the dedicated static runner. The compressed receipts retain all selected page and named outcomes. The LP runner metadata abbreviates local binary paths to basenames. Verify the results with `python3 compare.py` and the files with `shasum -a 256 -c SHA256SUMS`.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out result.json
```

For the separate probe, place probe.html under a probe/ directory beside the pinned WPT resources/ and fonts/ directories, and run the same driver with `--wpt "$PROBE_ROOT" --kind testharness --dirs probe --jobs 1 --bin "lp=$BIN" --out probe.json`.
