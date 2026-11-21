# CSS animation list matching and composite order

Source `9178e7fdd9376de5ff4e23b383605b99004ef8f4` directly follows the published event checkpoint `04f5b379a11325935e35d0d0a03afa56c9d4af3e`, descended from frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Only CSS animation state and its Web Animations enumeration changed. Previously, a second occurrence of a name could reuse a run just created during the same update. Reconciliation now matches the last remaining old occurrence from the right, preserves its timeline origin, and samples in updated list order. Enumeration uses the CSS owner, not a script-replaceable effect target, to order bound animations and transitions.

Selected list reconciliation ahead of pending-promise handling and keyframe replacement: five assertions on one dynamic page exercise the same list update, with independent element and document ordering coverage. The 11 executed URLs are from the archived round-six ordinary cohort, with no withheld URL. No full suite or expectations changes. Matched runner: WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, patched JSC `0e0dd7038a7a-6f4649e395e0`, `wptiter --features pyro/render`, up to four workers per job; `wptserve --testdriver` on distinct ports.

| Run | Build wall | WPT wall | Port | Strict pages | Named passes |
|---|---:|---:|---:|---:|---:|
| Published parent (reused binary) | — | 510 ms | 57372 | 6/11 | 138/155 |
| Candidate | 22.444 s | 456 ms | 57676 | 7/11 | 151/155 |
| Final source (formatting cleanup) | 20.166 s | 454 ms | 58741 | 7/11 | 151/155 |
| Chrome 158.0.8127.37 | — | 968 ms | 57492 | 9/11 | 151/155 |

All harnesses completed successfully, with no missing names, restarts or watchdog kills. Candidate and final have identical page and named outcomes. A strict page requires a successful harness and every named assertion passing.

- Parent → final: **+1/−0 strict pages; +13/−0 named passes**. `css/css-animations/Element-getAnimations-dynamic-changes.tentative.html` became strict: all five assertions gained (preserved start time, updated order, preservation only for existing runs, trimming from the front, interleaving). Animation `Document-getAnimations` gained five (list order independent of start time, across elements, across and within elements, bound vs free, and marker ordering); animation `Element-getAnimations` gained two (duplicates and name order); transition `Document-getAnimations` gained non-marker pseudo ordering.
- Chrome vs final: three Chrome-only strict pages and three Chrome-only passes (paused state in animation element identity, marker transition count, simultaneous transition property ordering). One final-only strict page and three final-only passes (existing start times, trimming, interleaving on the dynamic page). Both failed free-animation restart ordering. Equal named-pass totals do not mean equal pass sets.
- Six strict controls stayed strict: animation name case sensitivity and inline style; animation event interface and pseudo-element; transition event interface and events-001. The first checkpoint's constructor gains remain covered.

Raw results are adjacent (`final.json.gz` is the published source). Binary SHA-256: parent `9569511b80b69fcadde2dd1be7103b93e61eec1e8541e50e3cf1a5da740f4cfe`; first candidate `4ea3ee2786db8f5269b01c416bab1025d48fdfa88616f830f0d53ff41e4acb34`; final `6543bbb754a398cdcf89f780bd867168900972805e39e94bdca8d4921263c9bd`.
