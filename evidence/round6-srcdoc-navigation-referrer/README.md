# Referrer-policy stability and srcdoc navigations

LP control cbdeb11931f74c3de4417a25fb9103f1df84d54e plus b57c35dd45d54d6781aafcb2c8242c9ad9b26e04 and the candidate 3fd854f5045187a77f8ab59af909f719d3fd4197 use the pinned CI JSC 0e0dd7038a7a-6f4649e395e0. Local Chrome stable 158.0.8127.37 used the signed macOS arm64 build and exact stable launcher. WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, fresh contexts, wptserve, testdriver, HTTP/2, 800×600 DPR 1 and four jobs. Primary WPT repeats use the unchanged no-native-replay runner. [Frozen 157-name comparison](https://github.com/povio/pyro/tree/94884ba5689fca207b30d67939db8356eb4c5ea0/evidence/round6-noncss-b57c35dd4) · [Source fix](https://github.com/povio/pyro/tree/3fd854f5045187a77f8ab59af909f719d3fd4197)

## Historical race

The original 157-name comparison contains 99 Pass→Pass, 33 Pass→Fail, 14 Fail→Pass and 11 Fail→Fail between the LP builds. 31/33 lost names involved iframe-tag and two involved a-tag. Chrome 158 passed all 157. Every historical page with only passing names in each LP run completed in under one second. Every historical page with a failed name lasted at least ten seconds.

Before rerunning, 15 URLs containing 46 historical names were fixed by original transition stratum and deterministic URL hash (six lost, four gained, two consistently failing, three passing URLs). This enriched sample cannot estimate the whole inventory. Within the 46 names, cbdeb passed 38, 30 and 37 on three runs; b57c passed 39, 38 and 40. Chrome passed all 46. The same pages registered 172 names overall; those pass totals were 152, 140 and 134 on cbdeb, 136, 145 and 153 on b57c, and 172 on Chrome. All pages had complete reports. These runs show substantial run-to-run variation. They do not establish a causal b57c event-loop regression. The original loss remains in the historical ledger.

The same-origin diagnostic uses independent Node HTTP request and response timestamps, an ID on each nested request and iframe, and a listener recording both the first message and the eventual ID-matched echo. All engines returned the expected sender first on all 96 rapid exchanges per engine across ordinary URL and srcdoc frames, with and without nested testharness and same-origin redirects. Four seven-second controls per engine received their valid reports first. Four eleven-second controls on each engine received a zero-test timeout completion first and then the valid report from the same iframe. Checking only sender identity would not distinguish these two payloads. The detailed traces record the actual request header independently of both message order and document.referrer.

A separate passive CDP observer recorded requests, responses, frame navigation, load and message events on four original WPT pages. The test files, listeners installed by WPT and official result ledgers remained intact. These instrumented runs are diagnostic only. In the enhanced trace, b57c had one page with a 9.655-second gap before a completion message; cbdeb had three pages with gaps of 8.935–8.976 seconds. Chrome had none. A b57c nested request reached HTTP 200 and load within milliseconds of the command; the echo document had initialized its result object, but no matching result message was observed before the outer listener received a different earlier iframe's timeout completion. Other already-live scopes then emitted completions while normal results were arriving. The captures locate the delay past the network completion in this example. They cannot distinguish a missing send from a delivery or listener problem, or attribute the 157 original observations individually. No message dispatch or timer behavior was changed.

## Separate conforming header fix

The direct same-origin controls found an omission on both old LP builds. Nested srcdoc → network iframe requests omitted Referer, whereas ordinary URL-frame requests used the source document URL. The echo document's own document.referrer agreed with the wire in both cases. On a focused 36-case matrix, Chrome followed the expected policy for iframe, anchor and form navigation at depths zero, one and two and for an ordinary child URL containing srcdoc. The two old LP binaries each passed 18/36. Enclosing documents contained a fragment and an unrelated base href. Strict-origin-when-cross-origin and origin exposed the difference; no-referrer passed throughout.

The source uses the existing client referrer lookup for iframe, link and form navigations. The lookup finds the nearest enclosing non-srcdoc document, retaining the document and element policy and the network redirect-chain behavior. Independent source review checked the initiator, base-URL distinction and policy controls. Scripted Location and popup navigation, and the initial srcdoc document's own referrer, remain separate paths. [Referrer Policy algorithm](https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer)

The direct wire-header gap can exist even when the generated WPT permits an omitted referrer. Its fix cannot be credited with resolving the timeout/message race. All diagnostics, selection scripts and raw structured observations are in the accompanying archives.

## Reproduction

```sh
bun bench/wpt/run-no-native-retry.mjs --wpt "$WPT" --kind testharness --dirs non-css --jobs 4 \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --tests "$SELECTION" --bin "$LABEL=$BIN" --out "$OUT"
```

selection.json is the 15-URL repeat, all-historical-urls.json the 64-URL remeasurement and guard.json the independent compatibility guard. Passive observer runs are labeled separately. Named comparison rows contain URL, original name, occurrence, b57c control, candidate and Chrome 158. Page rows contain the three page outcomes, harness status and fully-passing flags in that order. A missing row or name is null, and raw timeouts remain in the recorded results.

A further uninstrumented run of all 64 original URLs produced 134/157 fixed-name passes on cbdeb and 128/157 on b57c. The fresh pair had 111 both-pass, 23 pass→fail, 17 fail→pass and six both-fail. Among the original 33 pass→fail names, only five repeated that direction; 20 now passed on both, four failed on both, and four reversed to fail→pass. Of the 13 original loss names in the repeated 15-URL sample, eight passed on all three b57c repeats and five passed on two of three. The totals are measurements from the stated runs; timing-sensitive losses remain explicit.

The candidate verified all 36 navigation cases and all 12 additional ordinary element-policy overrides, matching Chrome at both wire and destination document.referrer. The previous b57c build passed 18/36 and 8/12 respectively. In the rapid same-origin nested iframe controls, the candidate also returned the expected referrer on 48/48 srcdoc→network requests (b57c 0/48, Chrome 48/48). All 96 echo results still arrived first and carried the intended sender ID. At seven seconds versus eleven seconds the candidate retained the same result-first versus completion-first pattern.

All 444 disjoint compatibility URLs were observed. The candidate and b57c had identical named and page statuses, 2,187 passing names and 430 fully passing pages on each. Chrome had 2,193 passing names and 436 fully passing pages. Two LP harness timeouts remained on each build and Chrome had three. There were zero new/missing LP names, no native restarts and no watchdog kills. A separate candidate rerun across all 64 historical pages passed 131/157 fixed names, with 20 gains and 17 losses relative to the fresh b57c rerun (128). The candidate passed 37/46 and 35/46 in two further 15-page repeats. These timing-sensitive changes are reported without attributing them to the header correction.

The release-profile browser binary was built at the implementation commit. The following commit adds only a native same-origin regression test, which checks both the request Referer and the destination document.referrer on initial iframe navigation and a named link navigation. Point PYRO_JSC_DIR to the pinned CI archive directory to run it.

```sh
CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false \
CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true \
cargo test --locked --offline --profile wptiter -p pyro-browser --test integration \
  referrer_policy::network_frames_navigated_from_srcdoc_use_the_enclosing_document_as_referrer
```

The new regression and both adjacent native inheritance/named-target checks passed. The pushed topic ends at the [test-only commit](https://github.com/povio/pyro/commit/9a8bb66875a4081fbeeba6aa9741100a907d405d).
