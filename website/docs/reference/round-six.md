# Round six compatibility progress

Pyro passed **21,688 more named assertions** and **749 more complete pages** between the first and last round-six runs of the same 8,082 CSS Web Platform Tests (WPT) pages. A page counts as passing when its harness completes successfully and every named assertion passes.

## CSS test results

| Measured build | Pages fully passing out of 8,082 | Named assertions passing |
| --- | ---: | ---: |
| Pyro at the start of round six | 5,074 <meter class="pass-count" min="0" max="8082" value="5074" aria-label="5,074 of 8,082 pages fully passing"></meter> | 160,746 |
| Pyro at the flex content sizing checkpoint | 5,792 <meter class="pass-count" min="0" max="8082" value="5792" aria-label="5,792 of 8,082 pages fully passing"></meter> | 182,015 |
| Pyro at CSS closeout | **5,823** <meter class="pass-count" min="0" max="8082" value="5823" aria-label="5,823 of 8,082 pages fully passing"></meter> | **182,434** |
| Chrome 158.0.8127.37 | 6,950 <meter class="pass-count" min="0" max="8082" value="6950" aria-label="6,950 of 8,082 pages fully passing"></meter> | 182,963 |

The CSS runs used the same pinned WPT revision on macOS. The final CSS build passed 5,921 named assertions that Chrome missed. Chrome passed 6,450 that Pyro missed. A separate 41-page selection covered workers and character encodings. Another 39 URLs remained unscored. [Starting measurements](https://github.com/povio/pyro/blob/305840f45afbc4f55bfa24f303c1e858d30e4c1d/docs/validation/studio-css-e4ca26/README.md) · [Sizing checkpoint](https://github.com/povio/pyro/blob/305840f45afbc4f55bfa24f303c1e858d30e4c1d/docs/validation/studio-css-e4ca26/flex-content-basis.md) · [CSS closeout](https://github.com/povio/pyro/blob/305840f45afbc4f55bfa24f303c1e858d30e4c1d/docs/validation/studio-css-e4ca26/cssrootcycle.md)

## Combined checkpoint

The final combined build, `305840f45`, ran a targeted check of **760 distinct URLs** after the CSS, parser, Navigator and VideoFrame changes were merged. It reproduced all 34 gains from the last CSS Values batch, including element-dependent animation timing, font metrics after web fonts load, and palette weights. All six CSS namespace and scrollbar-cycle gains also passed. The run had zero browser restarts or watchdog kills. [Combined revision](https://github.com/povio/pyro/commit/305840f45afbc4f55bfa24f303c1e858d30e4c1d) · [Exact-build checks and receipts](https://github.com/povio/pyro/tree/6f19da14ad60fb3ffede4775b14f30fda71f60a5/evidence/round6-validation-305840f45)

On 58 production-control URLs, the combined build gained 59 named passes and lost 49 against the earlier production build. Of the losses, 42 involved referrer policy, six involved subresource integrity, and one involved HTML. A CSS `if()` cycle also remains a counted difference from the pinned Chrome result. [Changes by area](https://github.com/povio/pyro/blob/6f19da14ad60fb3ffede4775b14f30fda71f60a5/evidence/round6-validation-305840f45/changes-by-area.json) · [Cycle comparison](https://github.com/povio/pyro/tree/305840f45afbc4f55bfa24f303c1e858d30e4c1d/docs/validation/css-values-a7f8d09e/if-cycle)

The WPT dashboard tracks whole-suite runs on the main branch. npm lists the available release candidates under `next`. [Whole-suite dashboard](/status/wpt.html) · [Package versions](https://www.npmjs.com/package/@povio/pyro?activeTab=versions)


<style scoped>
.pass-count { width: clamp(3.5rem, 12vw, 9rem); margin-inline-start: .5rem; vertical-align: middle; accent-color: var(--vp-c-brand-1); }
.pass-count::-webkit-meter-optimum-value { background: var(--vp-c-brand-1); }
.pass-count::-moz-meter-bar { background: var(--vp-c-brand-1); }
</style>
