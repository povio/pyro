# Add Party scroll/readiness reduction

Pinned engine5b467880, binary18fa9de7; exact f9f @povio/ui5.0.0-rc.10/React dependencies. Generic reduced component only; no copied app source/auth. `main.jsx` bundles with Bun build --target browser; runtime desktop breakpoint CSS required. Probe uses existing unchanged activation helper and localHTTP. Source setup artifacts preserved.

Actual TOS AddParty call site: positions-commercial-parties.e2e.ts selectAddParty lines266-274 calls clickRenderedPlaceholder(page,"Add Party"), then chooseOption("Insurance Provider"). Diagnostic retained all assertions: initialinputy27270; focus moves viewport to26033;22options mount, then changedDIV/documentscrolltasks arrive and options disappear. Original test restored,0runDBs.

Valid desktop control v3/v4: onscreen original synthetic/native activation retains2options in both engines. Offscreen synthetic activation opens then scroll closes in both. Chromium native/pre-scroll also closes; rewrite native/pre-scroll retainsoptions. Keyboard control also closes offscreen in both. These outcomes do not establish an incorrect native focus/scroll implementation.

Final event-based readiness control4/4: request scrollIntoView({block:center}); if viewportoffset changed await its actual document scroll event, otherwise resolve immediately; then unchanged activation. Onscreen/offscreen2options in bothengines. No timer delay before interaction; fixed150ms post-action observation horizon only measures retainedpopup, not a TOS wait. This reduced control covers viewport scrolling only; actual TOS nestedDIVscroll readiness still requires a separate bounded helper and unchanged fullfile validation. Not fullsuitegreen or complete layoutconformance.

Initial v1 observer assumed input remains after static replacement and threw; preserved invalid observation. V2 missing breakpointCSS selected mobile component, so it is not desktop readiness evidence. V3/v4 and final probes are the valid desktop controls; failures are not discarded.
