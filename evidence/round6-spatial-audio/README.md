# Listener-relative equal-power spatial audio

LP implements the context AudioListener and equal-power PannerNode for mono and stereo input. Positions and orientations are AudioParams. The renderer uses the listener's forward/up basis, the three standard distance models and source cone gain. It computes a single geometry per quantum for static or k-rate coordinates and evaluates automated a-rate coordinates per sample. HRTF rendering remains unsupported.

| Matched ordinary Web Audio cohort | Named passes | Fully passing URLs | Timeouts | Errors |
| --- | ---: | ---: | ---: | ---: |
| LP aa0268b47 | 3,214 | 112 | 19 | 16 |
| LP b1acacb9c | 4,151 | 131 | 19 | 15 |
| Local Chrome stable 157.0.8103.80 | 5,725 | 316 | 8 | 2 |

All 348 URLs were observed. Chrome also recorded two hangs in the existing same-pin reference. LP gained 940 named passes and lost three names containing the old missing-constructor error text. The guard registered 781 new names and stopped reporting 17 earlier names. Both LP runs completed without browser restarts or watchdog kills. The full census remains tied to its published earlier binary. [Original selection and Chrome reference](https://github.com/povio/pyro/tree/fdf757052183b8df60dae474f478d20de16aa436/evidence/round6-noncss-902fe811f)

The normal rendering probe measured channel samples for ten source/listener configurations on both browsers. Nine agreed within 0.00000014. For a source coincident with the listener and a zero-width directional cone with zero outer gain, LP applies the draft cone algorithm to a zero direction vector, giving a 90-degree angle and silence. Chrome produced unity cone gain. Both rejected a legacy coordinate change at the start of an active automation curve and left the other coordinates intact. The native LP and Chrome samples are in reviewed-probe.json and chrome.json. The module-level numeric controls also exercised the exact automation frame. The standalone workload renders eight moving or static sources for two seconds and checks both output-channel checksums.

The names table stores URL, canonical name, one-based occurrence and (status, original name) for control, candidate and Chrome. Null denotes an absent name. The page table stores outer status, harness status and full-pass flag in that same browser order. Executables, runner and WPT revision are identified in provenance.json. Chrome WPT receipts are reused; the normal rendering probe was freshly run on local stable.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```
