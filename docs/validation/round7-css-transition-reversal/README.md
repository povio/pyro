# Interrupted CSS transition reversal

Source `eefe04af583c61a95802a2a280db9c7ed23f4faf`, parent `12e5d59b0b7fca6bf076f824a8902394a44485ef` (frozen ancestor `305840f45afbc4f55bfa24f303c1e858d30e4c1d`). CSS Transitions 1 §3.1 defines faster reversal using the interrupted transition's transformed progress and reversing shortening factor. CSS Transitions 2 §3.1 associates reversal state with the transition, not its replaceable effect. The new run scales its duration and negative delay, but not a positive delay. An ordinary interrupted longhand starts from its sampled value.

## Selection

Offline batch12 (`8695db84513be44b2aedc700e3e042ee9354b2b4`) and pinned Chrome, ordinary complete/OK pages: **five named tests on three pages** first failed the reduced-duration check (`CSSTransition-currentTime`, `CSSTransition-effect`, three on `KeyframeEffect-setKeyframes`). This outranked the two CSS animation `reverse()` override assertions on one page by coverage. Eleven transition event-dispatch failures on one page span different phase changes and timeline detachment; the count alone does not establish a shared multi-page cause. Event-to-animation identity failures on two pages concern event interfaces rather than playback or ordering. No values/color, CSSOM, layout, mixins or expectations edits.

## Matched outcomes

| Engine | Strict pages / 8 | Passing names / 53 | Focused wall time |
| --- | ---: | ---: | ---: |
| Prior checkpoint | 5 | 45 | 2.282 s |
| Candidate | 7 | 47 | 2.183 s |
| Chrome 158.0.8127.37 | 8 | 53 | reused frozen results; no focused timing |

Candidate versus parent: **+2/−0 strict pages, +2/−0 names**. Both the seek with nonlinear easing and the effect replacement now pass. Three more tests on the setKeyframes page pass their shortening assertion and then fail reading the replacement's exposed first keyframe; these are **not** counted as gains. Candidate versus Chrome: **+0/−1 strict pages, +0/−6 names**, all on `KeyframeEffect-setKeyframes.tentative.html`. No general fix for script-replaced keyframe sampling, composite ordering or exposing CSS keyframes is claimed. Exact sets: `directions.json`; regenerate with `python3 compare.py`.

Pinned WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; patched JSC `0e0dd7038a7a-6f4649e395e0`. All eight URLs are in the frozen main positive manifest and disjoint from withheld; all were complete/OK on frozen, historical and Chrome. Both new WPT jobs used identical manifest, `--dirs css/css-transitions --jobs 2 --server wptserve --testdriver`, ports 61543 and 61651. All harnesses complete/OK; zero restarts, zero watchdog kills. Chrome compact rows extracted from `frozen-evidence/outcomes-chrome-main.json.gz`, whose provenance verifies the pinned stable raw reports. Two warm three-job Cargo builds: 60 s preliminary, 31.74 s final. No full run.

Independent native CDP control (`probe.cjs`, `control.js`) used a local blank document, not another WPT run. At a quarter-progress reversal, candidate and pinned Chrome both start at opacity 0.25 and produce durations `[100000,25000,87500,100000]` ms for initial/reverse/reverse-again/unrelated-retarget. Parent starts at 1 and returns four full durations. A −10 s delay shortens to −2.5 s, while +10 s stays +10 s. JSON outputs retained for all three engines.

Parent SHA-256 `ca219d481418a176c75ed19420a3b1f72ddd46a46650a6ae57fb9131d120e04b`; candidate `8b96c75b13c07c746ab0e9e690bf71252da98222cc8a41f134a41a3f703e253e`.
