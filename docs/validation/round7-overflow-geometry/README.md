# Non-scrolling box widths in reversed writing directions

Parent `fccadd99563226c69967c6828bcf235407a2963e`, source `c4559f3c4ab9149c1f9c909afd2db5d6bcc723f0`. A non-scrolling box retains both overflow edges but does not carry a reversed scroll-origin flag. Using that flag alone caused CSSOM to measure the right edge of overflow even when the writing direction starts on the right. On the asymmetric negative-margin flow-root, horizontal RTL or vertical-rl, Pyro returned 196px instead of Chrome's 204px. The fix measures from the right padding edge for non-flex boxes with visible or clipped overflow and a right-hand writing-mode start. It does not change layout, clipping, overflow propagation, scroll offsets or vertical measurements. See [CSSOM View: scrolling area](https://drafts.csswg.org/cssom-view/#scrolling-area).

**Focused WPT:** 10 complete pages, 958 named outcomes, zero harness errors, restarts or watchdog kills on parent/candidate/candidate/parent. Passes 828 → 840; strict pages 7 → 7. Twelve Fail → Pass, all passing signed Chrome, no losses or name-set changes. Six flow-root and six grid widths on negative-margin-002: visible/clip × (horizontal-tb RTL, vertical-rl LTR, vertical-rl RTL). No extrapolation. The other 24 Chrome-passing failures here are flex variants. Applying writing-direction-only widths to flex gained those names but lost 24 existing WPT passes in reverse configurations. Flex needs independent axis/alignment treatment. A broader two-axis prototype also changed vertical probe values away from Chrome. Neither prototype is included.

**Containment is unresolved, not a gain.** All 128 names ran; the same twelve grid names failed on both builds (hidden/auto/scroll × two paddings × two borders). For example, at zero width and height without padding, visible gave 0 versus hidden 2000. This test mutates the same box through display and overflow changes and compares two readings, not a fixed pixel value. The standalone zero-width grid probe agreed with Chrome in all three overflow modes, while the WPT's display sequence still differs; not sufficient evidence to change grid track contributions or discard zero-area boxes globally. Another standalone divergence is zero-width block/hidden: 4 versus Chrome 2004 with padding. No containment change.

**Direct controls:** `controls.html` records both dimensions for 90 cases (flow-root/flex/grid, LTR/RTL, horizontal/vertical/sideways, visible/clip/hidden, plus contained boxes of zero/nonzero width). Exact Chrome pairs 71 → 83; every changed pair matches Chrome. The seven remaining mismatches were unchanged. Chrome 158.0.8127.37 on macOS, `codesign --verify` passed, Google LLC team `EQHXZ8M8AV`. Fresh Chrome WPT outcomes and actual failure messages included: all 958 statuses match the archived reference in `7c4f42242:docs/validation/studio-integrated-4126821de/refs/outcomes-chrome-main.json.gz` (archive SHA-256 `7858861e3a061f3d352b47336de17ba0dc06a89f81ceb7b48e480ce94e85a4e0`).

**Reproduce:** matched WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, runner tree `7efc9828957ae18dbd8c8c7df59cba3c59524633`. Isolated `CARGO_TARGET_DIR`, `CARGO_BUILD_JOBS=3`, `pyro-build --features pyro/render`. SHA-256 of parent/candidate binaries: `292c0e9c89e6437088d556bb1ccdc03509233310e0a1cf6edad59dbb875850fe`, `34d0a5f75abca8a5740182fdb843fedf6682c9cb97783e6399146e6acc6915d4`.

```sh
D=docs/validation/round7-overflow-geometry
bun bench/wpt/run.mjs --kind testharness --dirs css --server wptserve --testdriver --jobs 2 --tests "$D/urls.json" --wpt "$WPT_DIR" --bin "$label=$binary" --out "$label.json"
bun "$D/chrome.cjs" "http://127.0.0.1:$port"  # HTTP server rooted at matched WPT
bun "$D/probe.cjs" "http://127.0.0.1:$cdp_port"  # Pyro serve, one binary at a time
python3 "$D/verify.py"
```
