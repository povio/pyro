# Linguistic pseudo-classes on exposed shadow parts

Studio, 2026-11-21. Source `596cc6bfe9b197463b103b2cc294ab65806f59dc` is a direct child of freeze `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Stylo explicitly rejected `:lang()` and `:dir()` in the element-backed pseudo-element parser state. Removing that guard lets the existing matcher test the part's language and direction, not the host's. Restrictions on tree-structural, slotted and non-element-backed selectors were not changed.

Ranked from the archived positive manifests, not by executing the directory: the two non-text input focus pages had 12 Chrome-only names but depend on pointer/focus dispatch; the complexity page had seven, including an explicit timeout check. The three part pages had six Chrome-only names and an identifiable shared guard, so they were preferred for a selector-only change. The three other pages in `urls.json` are unchanged controls. All six URLs belong to the frozen main allowlist and none belong to the withheld manifest.

| Engine | Strict pages / 6 | Named passes / 88 | Runner wall | Browser CPU | wptserve port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Frozen Pyro | 2 | 81 | 0.296 s | 0.14 s | 52092 |
| Candidate | 4 | 85 | 0.157 s | 0.12 s | 61601 |
| Chrome stable 158.0.8127.37 | 3 | 64 | 0.555 s | 0.88 s | 60607 |

Frozen → candidate: **+2/-0 strict pages; +4/-0 named passes**. `parsing/parse-part.html` gained `::part(foo):lang(en)` and `::part(foo):dir(ltr)` (30 → 32 passes); `invalidation/part-dir.html` gained both attribute/property mutation assertions (0 → 2). All four gains pass on Chrome. Both `invalidation/part-lang.html` assertions still fail: the initial inherited match now succeeds, but setting the part language to `en-GB` returns a transparent background instead of green. The language-tag-length control remains a known failure. A strict page is complete, harness OK, with all named assertions passing. `outcomes.json.gz` retains every named result for all three engines; the frozen and Chrome results were checked against the archives. All harnesses completed OK; zero restarts or watchdog kills. Single-run times are not performance claims.

Matched `bench/wpt/run.mjs --kind testharness --dirs css --server wptserve --testdriver --jobs 1 --tests urls.json` with WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Patched JSC `0e0dd7038a7a-6f4649e395e0`; frozen browser SHA-256 `50c39eb1d6161c8ef97d2063bddf94c53fa73ab8b7995b4d9448f5b3fe3a0880`, candidate `71eff2d96ba530e0928b8f6956f1516295aae19404af36aba3e25d96d4ec9363`. Isolated warm `wptiter` browser build: 221 s, two Cargo jobs. Native `pyro-render` integration test `shadow_scoping::linguistic_selectors_match_the_part_not_its_host`: 1/1 passed (0.08 s; 155.18 s including test build). `scripts/vendor/regen-patch stylo --check --base 305840f4` verified forward and reverse reconstruction. No expectations changed.
