# Retained CSS animations in ordinary display:none subtrees

Source `603fffa17fc152a6b10ff4916280d5f6017f81c6`, parent `e98745f2268262991c1cd57ccabe230c1067a147`, verified frozen ancestor `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. After the previous transition fix, frozen Chrome-only groups included three ordinary hiding assertions on two pages versus two reverse-playback assertions on one. The animation list now omits ordinary hidden targets before reconciling retained objects. It uses flat-tree ancestry rather than box existence. No layout, CSSOM, CSS-values, mixins, or expectations edits.

| Run | Wall ms | Port | Strict pages / 10 | Passing names / 62 |
| --- | ---: | ---: | ---: | ---: |
| Parent | 2259 | 49174 | 6 | 53 |
| Candidate | 2200 | 62573 | 8 | 56 |
| Chrome 158.0.8127.37 | 2306 | 49295 | 9 | 59 |

`urls.json` includes only positive round-six cohort URLs, complete/OK in historical, frozen and Chrome results and not withheld. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, patched JSC `0e0dd7038a7a-6f4649e395e0`. Matched named sets; every harness completed with zero harness status, restarts and watchdog kills. Strict means all assertions passed on a nonempty complete/OK page. Warm builds at three jobs: preliminary 27.213 s, final 49.330 s. Focused WPT with three workers: parent and Chrome once each, preliminary candidate 2206 ms, then final candidate. No full run.

- Against parent: +2/−0 strict pages, +3/−0 names. Retained animations become idle on self-hide and ancestor-hide; hidden descendants do not restart on unrelated restyles.
- Against Chrome: +1/−2 strict pages, +3/−6 names. All six Chrome-only names were already failing on parent: four animated-display cases and two transition cancellation cases. The three candidate-only names cover existing animation-list identity. `directions.json` lists every directional difference.
- Additional ordinary mixed-sibling probe (`sibling.cjs`, separate from the WPT counts): loads the vetted prevention page and uses stylesheet classes. The retained animation in the hidden branch became idle, the active sibling stayed running with the same object, and the list shrank to one. Showing the branch created a fresh object. Self-hiding that replacement also kept the sibling active. Candidate and Chrome agreed exactly; parent failed both hides and reused the old object. The preliminary candidate missed stylesheet self-hide, which prompted the final revision. Control arrays record hidden play state, sibling play state, sibling identity, and subtree list length, in order.
- Scope: sampled display and potentially display-affecting custom-property effects are treated conservatively. This does not establish correct cancellation when an unrelated animated custom property coexists with an authored hide, or implement display interpolation. The existing animated-custom-property, discrete transition, display:contents and visibility controls had no named regressions.

SHA-256 parent `09c68eb291ecc59dfb4f95e1b077e6c17fd01647bf229b0e4123a784c127b268`, final candidate `ca219d481418a176c75ed19420a3b1f72ddd46a46650a6ae57fb9131d120e04b`.
