# Mixed angle/percentage conic gradient stops

Parent `0038d6580690e7ccac0a20cbc9392d5e3d64c943`; implementation `4e9ad4452b07c380c58b96a8c63d1eaf3d588882`. On the parent, `calc(50% + 90deg)` was accepted but serialized as `calc(270deg)` in the declaration. This prematurely discarded the percentage term. The conic stop consumer now retains its typed calculation through specified serialization, then resolves its percentage basis to one turn when computing the angle. Existing computed and paint representations are unchanged. No generic arithmetic or color interpolation change. See [CSS Images 4, angular color stops](https://drafts.csswg.org/css-images-4/#typedef-angular-color-stop-list) and [CSS Values 4, calculation serialization](https://drafts.csswg.org/css-values-4/#calc-serialize).

**Focused result:** all 12 pages completed, harness OK, on parent/candidate/candidate/parent. On each comparison exactly six names changed from Fail to Pass, all on `conic-gradient-calc-angle-percentage-valid.html`, all passing the signed Chrome reference. Strict pages: 10 → 11; named passes: 2,773 → 2,779 of 2,785. Six other failures remain. No full run or extrapolation.

The 12-page positive manifest covers conic stops, hints and repeating gradients; linear/radial rejection, invalid units and unitless zero; and background-image, gradient position, interpolation method and shape-outside computed/valid/invalid controls. Separately `controls.html` compares specified and computed CSSOM on eight inputs (plain angle/percentage, sums including zero, subtraction, min/max, invalid length). All eight candidate pairs matched a fresh Google Chrome 158.0.8127.37 run. All computed values were unchanged from the parent.

**Reproduction:** use the Studio `pyro-build --features pyro/render` wrapper at each revision with `CARGO_BUILD_JOBS=3`, separate `CARGO_TARGET_DIR`, and the normal patched JSC. Binary SHA-256: parent `70c36ef545cf20dcd5f3fade4cddc7246f414449b1cd8cea241e6968c3ccde5b`; candidate `21756c1a674d9b689871d2669f0c98e6b1d4e214e931140ae528c6f354126af5`. `scripts/vendor/regen-patch --base HEAD stylo` forward/reverse checks passed before commit. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The archived WPT comparator (not a fresh WPT run in Chrome) is the selected subset of `7c4f42242:docs/validation/studio-integrated-4126821de/refs/outcomes-chrome-main.json.gz`, SHA-256 `7858861e3a061f3d352b47336de17ba0dc06a89f81ceb7b48e480ce94e85a4e0`. Fresh Chrome probe: macOS `codesign --verify` passed, Google LLC team `EQHXZ8M8AV`; isolated profile, headless `--dump-dom` of the controls page. Probe Pyro via CDP `serve` (not the non-rendering fetch path).

```sh
D=docs/validation/round7-conic-gradient-stops
bun bench/wpt/run.mjs --kind testharness --dirs css --server wptserve --testdriver --jobs 2 --tests "$D/urls.json" --wpt "$WPT_DIR" --bin "$label=$binary" --out "$label.json"
bun "$D/probe.cjs" "http://127.0.0.1:$port"
python3 "$D/verify.py"
```

**Separate scroll-state triage:** four serialization pages stopped in setup because `CSS.supports('container-type:scroll-state')` is false. Each reported a harness error and the synthetic `no test suite completion` row, not an observed WPT assertion. Chrome passed five names per page that Pyro never ran. No support-bit change and no gains claimed. Dashed functions are unrelated and untouched.
