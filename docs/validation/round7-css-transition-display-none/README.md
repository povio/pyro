# Cancel transitions when ordinary display changes hide their subtree

Source `f136006498cd3d8dbabeaf97fcf7a2fd9fe00320`, parent `7813266be4ed31214669633e9df7d15283412eb5`, verified frozen ancestor `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Frozen Chrome-only clusters: ordinary transition hiding (7 assertions on 3 pages), ordinary CSS animation hiding (3 on 2), and reverse playback overrides (2 on 1). The first group has an explicit before/after display change in the transition-starting algorithm; it doesn't require interpreting sampled display values or changing layout. CSS animation starting and display interpolation remain separate.

An ordinary change to `display:none` now drops active transitions on that element and descendants, disables their queued events, and schedules cancellation at their clamped active time. The existing discrete display transition and hidden-content paths remain separate. Retained transition objects flush CSS changes on playback reads, as CSS animation objects already did.

| Run | WPT wall | Port | Strict pages | Named passes |
|---|---:|---:|---:|---:|
| Parent | 2191 ms | 53450 | 6/13 | 79/115 |
| Candidate | 2188 ms | 53576 | 7/13 | 91/115 |
| Chrome 158.0.8127.37 | 2405 ms | 54574 | 11/13 | 110/115 |

All runs observed identical page/name sets, completed all 13 harnesses with status zero, and recorded zero restarts and watchdog kills. Strict = nonempty, complete, harness OK, all named assertions pass. One warm build (21.094 s) and three focused `wptserve --testdriver` invocations, each with four jobs. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; patched JSC `0e0dd7038a7a-6f4649e395e0`. Every URL in `urls.json` was on frozen main, off withheld, and complete/OK in the round-six historical, frozen and Chrome archives. No full run.

- **Candidate relative to parent:** +1/−0 strict pages (`transitioncancel-001`), +12/−0 names. Seven are display-specific: self and ancestor transition cancellation, before phase and active phase with zero/positive/negative delays, and the `transitioncancel-001` event. Five more pass when observing the retained object: replacement, reversal, removal from transition-property, calling cancel after cancel, and calling cancel after immediate restart. All 12 pass in Chrome.
- **Candidate relative to Chrome:** +1/−5 strict pages, +3/−22 names. Candidate-only page and names: dynamic `Element.getAnimations` (start times, trimming, interleaving). Chrome-only strict pages: `CSSAnimation-canceling`, `display-none-dont-cancel`, `display-none-prevents-starting-in-subtree`, `CSSTransition-canceling`, `CSSTransition-ready`. Chrome-only names by page: respectively 2, 4, 1, 2 and 2, plus 11 remaining on `event-dispatch` (neither browser passed that page strictly). Exact directional named sets are in `directions.json`.
- **Controls:** Both discrete display exit controls stayed passing; `display:contents`, CSSAnimation-ready, transition event interface and dynamic retained-list controls remained strict. No gain claimed for CSS animation hiding or animated display. No layout, CSSOM, CSS-values or expectations changed.

SHA-256 parent binary `9c814a14577fee0903e20c1efcbcda88fe9f293cab58531b44f87f919372d0b8`; candidate `09c68eb291ecc59dfb4f95e1b077e6c17fd01647bf229b0e4123a784c127b268`.
