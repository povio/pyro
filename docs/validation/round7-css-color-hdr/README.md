# CSS Color HDR: dynamic-range-limit

Studio assigned the four ordinary `css/css-color-hdr` pages to the local CSS lane after its batch-nine census found 99 Chrome-only assertions. The local baseline was the clean combined checkpoint `b8cf9e7150c191b4eab5f26c13e44d85505ce01c`; its tree was `a40ae09207552ba5fedb4891abc89c8c1b4d93a9`. These are development measurements after RC.4.

`dynamic-range-limit` is an inherited paint property with initial value `no-limit`. The candidate retains percentages in nested specified mixes, computes them against the element context, normalizes the three contributions, and interpolates their weights during animations. Canonical computed serialization drops zero contributions and collapses a single surviving level to its keyword. The renderer exposes standard dynamic range and emits 8-bit SDR output. The tests establish property and animation behavior; they do not measure tone mapping on an HDR device.

## Independent comparison

The base passed 17 of 116 named assertions. The candidate passed **113 of 116**: **99 gains, three counted losses**, a net gain of 96. Fully passing pages rose from zero to three of four. All 64 interpolation assertions, 21 computed-value assertions and two inheritance assertions passed. Parsing gained 12 and lost three. Every original Chrome-only gap in this directory was recovered. No test names or page statuses changed, and there were no browser restarts or watchdog kills. QA ran the exact candidate at 20:04:24–20:04:26 UTC on November 21. [Per-name differential](differential.json) · [Raw base](parent.json.gz) · [Raw candidate](candidate.json.gz)

The three retained pinned losses are singleton, literal all-zero and nested literal all-zero mixes. Both engines accept a singleton. The candidate accepts and preserves the latter two declarations and computes the initial `no-limit` value, whereas Chrome rejects those literal inputs. This is a deliberate implementation of the current draft, distinct from a claim of three additional cross-browser matches. [Candidate readbacks](candidate-readbacks.json) · [Chrome readbacks](chrome-readbacks.json)

## Reproducibility

| Item | Revision or SHA-256 |
| --- | --- |
| Exact candidate source | `c8a59563d25aa6b131dee7f3be01824e72f22df1` |
| Base executable | `ea872465222be67d7af6fc80c20a8e3d83b2d1cc5766b56c96c35313c4b7ef62` |
| Candidate executable | `4abb17bc2ee412687836818b8e933c44eefc9bc29d3d34dc6f400e4e6de7eb63` |
| Pinned WPT | `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` |
| Frozen selection | `a55b1b3b296d8c306b4c525b7092577f5dd897ef82679e23ba13762f38ae46e9` |
| Pinned JSC | `0e0dd7038a7a-6f4649e395e0` |
| JSC static archive | `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029` |

The private warm target used one Cargo job. The base build took 121 seconds. The candidate build took 219 seconds after a 27-second attempt exposed a keyword-parser call signature mismatch. All three connected browser guards selected by `dynamic_range_limit_` passed, covering CSSOM writes, inheritance, nested mixes after a font change, grammar and animated sampling. The native suite built in 96 seconds and executed in 0.08 seconds. WPT used the pinned manifest, wptserve on isolated ports and one worker. [Explicit selection](hdr-urls.json)

## Pinned grammar and editor’s draft

The current draft accepts **one or more** terms in a mix; terms allow the limit and percentage in either order. If all contributions are omitted, the computed result is the initial value. The CSS Working Group removed the old zero-sum invalidity rule. The current upstream parsing test and pinned WPT file have identical bytes, SHA-256 `2b033cf019c5be317c698f93035cdfe0debe55f964d4197be3abc5430bee1876`. The original Pyro implementation rejected all declarations of this property, so it passed all 17 invalid-value assertions. [Versioned draft grammar and normalization](https://github.com/w3c/csswg-drafts/blob/804b7b0055e80dfaba091e7f32a4bca303e569e3/css-color-hdr-1/Overview.bs#L493-L535) · [WG zero-sum discussion](https://github.com/w3c/csswg-drafts/issues/11678) · [Upstream parsing test](https://github.com/web-platform-tests/wpt/blob/9986bb32c9e97707eff870ab284ebc5cc7ae8d21/css/css-color-hdr/parsing.html)

Exact Chrome 158.0.8127.37 passes 115/116 on the pinned pages. It accepts singleton mixes and rejects literal all-zero mixes, including when nested. It falls back to the inherited `constrained` value on those invalid writes in the recorded probe. [Chrome page reports](chrome.json.gz) · [Chrome readbacks](chrome-readbacks.json)

## Cost on SDR

Eight processes ran sequentially in ABBA/BAAB order, using the same scripts and a fresh browser per arm. The longer run measured 8,000 computed-color reads per sample (80 ancestor mutations × 100 descendants), with 20 timed samples per binary per fixture. The second fixture also writes an inherited HDR limit on each ancestor change; the baseline does not implement that declaration. Values below are wall medians and mean process-tree CPU milliseconds per sample.

| 8,000-read fixture | Base wall | Candidate wall | Base CPU | Candidate CPU |
| --- | ---: | ---: | ---: | ---: |
| Ordinary inherited color, no HDR declaration | 13.302 | 13.152 | 14.0 | 13.5 |
| Inherited color plus HDR declaration | 13.038 | 13.478 | 12.5 | 13.5 |

The longer run measured 52.52% total host CPU during its 3.462-second timed window, 52.39% on base arms and 52.66% on candidate arms. Two census shards and CI overlapped. The earlier shorter run, 400 reads per sample, measured 2.728→3.619 ms on ordinary style and 3.314→3.338 ms on inherited-limit writes. It ran at 99.59% host busy time; the longer sample did not reproduce the large ordinary-style penalty. Neither experiment estimates performance for arbitrary pages. [Long timing samples](hdr-long.jsonl) · [Long host data](long-host.json) · [Short timing samples](hdr-cost.jsonl) · [Short host data](short-host.json)

Pixel readbacks ran outside the timing window. On the SDR color/gradient fixture, all four settings on both binaries produced identical PNG bytes, SHA-256 `0f29ccbdcea93bd528c449dad6817b63af6f954259076c512a1989b32f23b2de`. [SDR pixel readbacks](hdr-pixels.jsonl)
