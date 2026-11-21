# Observing CSS-driven playback changes

Source `68cac3ba6d027f44fd88250e023873405ebe1ae6` directly follows `77c4add5ff2fbc5434e9ed6df9c52857b404709f`, a verified descendant of frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Frozen Chrome-only assertions clustered on CSSAnimation-ready (5), pausing (5), canceling (3) and playState (1). Chosen cause: a retained CSSAnimation could expose pre-change state and settle its old ready promise before noticing a pending style change. This is separate from reverse playback and display-tree membership. The playback bridge now refreshes style before observable playback reads and before a pending operation settles; a changed CSS play state uses the existing play/pause machinery. Removed CSS runs cancel their animation objects. A failed script play does not take ownership of the CSS play state.

Matched read-only parent binary from the previous checkpoint and one warm build (`wptiter`, `pyro/render`, four build jobs, patched JSC `0e0dd7038a7a-6f4649e395e0`). Only the ten URLs in `urls.json` were executed. Each was on the frozen main allowlist, absent from the withheld list and had completed successfully at harness level in archived round six. Frozen WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; headless Chrome stable 158.0.8127.37; four sequential focused runs with `--jobs 4 --server wptserve --testdriver`. No full run.

| Run | WPT wall | Server port | Strict pages | Named passes |
|---|---:|---:|---:|---:|
| Parent | 336 ms | 52992 | 5/10 | 130/145 |
| Candidate | 230 ms | 62510 | 8/10 | 141/145 |
| Candidate repeat | 285 ms | 62186 | 8/10 | 141/145 |
| Chrome | 918 ms | 53112 | 9/10 | 142/145 |

Warm build wall: 21.694 s. All four completed every harness with status 0, identical observed names, zero restarts and zero watchdog kills. Both candidate runs had identical outcomes. Strict means every named assertion passed on a nonempty, successfully completed page.

- **Candidate vs parent:** +3/−0 strict pages (`CSSAnimation-playState`, `CSSAnimation-ready`, `Element-getAnimations`); +11/−0 names. Gains: CSS resume visible on read (1); ready promise changes on CSS pause and resume, redundant pause promise identity, and rejection after CSS removal or name change (5); pending pause completed by seek, running startTime does not override CSS, unsuccessful play does not override CSS (3); name removal cancels the retained object (1); retained object identity/play state (1).
- **Candidate vs Chrome:** +1/−2 strict pages; +3/−4 names. Candidate-only: `Element-getAnimations-dynamic-changes` and its existing-start-time, trim-from-front, and interleaving assertions (already parent passes). Chrome-only: `CSSAnimation-canceling` (self/ancestor display:none) and `CSSAnimation-pausing` (two reverse() override cases). All eleven newly passing names pass in Chrome.
- **Controls:** `CSSAnimation-startTime`, `CSSAnimation-animationName`, `Element-getAnimations-dynamic-changes` and both event-interface pages remained strict passes. No expectations, CSSOM, CSS-values or layout changes.

Each compressed JSON contains the raw named outcomes. Parent SHA-256 `6543bbb754a398cdcf89f780bd867168900972805e39e94bdca8d4921263c9bd`; candidate `9c814a14577fee0903e20c1efcbcda88fe9f293cab58531b44f87f919372d0b8`.
