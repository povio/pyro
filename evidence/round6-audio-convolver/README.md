# Offline ConvolverNode

Source `f1d1e805200e2ca9f9de70329b3f8a70830e1fed` builds on DelayNode `2c9a98c9175355edd10fa4f9e02efe25c6c1ea00`. The native renderer computes the first 128 impulse taps directly. Successively larger overlap-add FFT partitions process later taps, with a frequency-domain delay line for partitions beyond 4,096 frames. Normalization uses the buffer's RMS, channel count, and sample rate at assignment. The node preserves input history across quanta, changing channel layouts, and late downstream connections. The implementation uses the existing locked rustfft 6.4.1 package. [Web Audio specification](https://webaudio.github.io/web-audio-api/#ConvolverNode)

| Matched macOS testharness scope | LP base | LP candidate | Chrome 158 stable |
| --- | ---: | ---: | ---: |
| 14 Convolver pages fully passing | 0 | 11 | 14 |
| Named passes on those pages | 72 | 258 | 262 |
| 40 adjacent Web Audio pages fully passing | 31 | 31 | 38 |
| Named passes on those pages | 671 | 672 | 692 |
| 346 selected Web Audio pages fully passing | 147 | 158 | 321 |
| Named passes on those pages | 4,305 | 4,537 | 5,757 |
| Page timeouts / errors across 346 URLs | 19 / 15 | 19 / 15 | 2 / 5 |

The 346-page comparison gained 232 named passes and 11 fully passing pages. Every previous pass was retained. Graph construction and rendering made 141 previously unreported names observable, and three formerly failing names disappeared. The two smaller selections are subsets of the 346-page run. Numeric audit messages can produce different assertion names across engines. The full outer comparison retains original names, occurrences, statuses and missing entries. The 346-page totals use the existing signed Chrome 158 census; all 14 and 40 URLs also have directly measured 158 receipts. The historical Chrome 157 artifact is separate. The shared selection excludes the two previously recorded Web Audio Chrome hangs. All LP/control and directly measured Chrome 158 scoped runs used two jobs and the pinned no-native-retry runner. [Results](summary.json) · [Provenance](provenance.json)

Four official assertions on three pages remain. Two require AudioWorklet, one requires real-time ScriptProcessor processing, and the Convolver constructor page invokes the separate DynamicsCompressorNode interface. The supported same-origin foreign AudioBuffer probe uses an active owner realm. Acceptance of an AudioBuffer first assigned after its originating iframe is disposed remains unverified. Rendering of the selected renderSizeHint cases does not establish support for configurable render quantum sizes.

All 12 candidate application-level probes passed. Chrome 158 passed 10. For a normalized impulse with RMS 0.5 at 44,100 Hz, LP produced 0.0025 using the specification's calibration constant 0.00125. Chrome produced 0.00251785037. When the convolver received an impulse before connecting its output at frame 128, LP retained the input and produced the expected 0.75 sample at frame 383. Chrome produced zero. Three native tests independently verify routing, normalization, switching channel interpretation across a mono tail, and a 9,723-tap dense response against direct f64 convolution including the full tail. [Browser probes](normal.html) · [Native results](native.log)

## Measured workload

A deterministic one-second, 48,000 Hz stereo input was convolved with a 257-tap or 32,768-tap stereo impulse using the browser's normal AudioBufferSourceNode → ConvolverNode → OfflineAudioContext path. Two sequential pairs ran in opposite browser order on the shared Mac. Each browser process discarded one warmup per impulse length. Medians summarize ten measured samples per browser and length. Node setup covers construction and response acquisition. Render elapsed time covers startRendering through completion. External wall time also includes constructing inputs, copying reference samples, checksums, direct-reference checks, and CDP overhead.

| Taps | Browser | Setup ms | Render ms | Wall ms | Checksum |
| ---: | --- | ---: | ---: | ---: | ---: |
| 257 | LP | 0.1 | 8.8 | 52.2 | 204.25956 |
| 257 | Chrome 158 | 0.1 | 2.75 | 11.7 | 204.25942 |
| 32,768 | LP | 2.0 | 63.15 | 108.4 | 207.62275 |
| 32,768 | Chrome 158 | 1.8 | 10.9 | 20.6 | 207.62259 |

Across both channels and 13 sampled frame positions per channel, LP's maximum absolute error against direct scalar convolution was 2.81e-8 for the long impulse. Chrome's was 4.26e-9. Host load varied during the census. Individual warm LP long-impulse render times ranged from 9.7 to 91.6 ms. These are offline timings on macOS arm64. [Paired measurements](paired-workload-summary.json) · [Workload](workload.mjs)

## Reproduction

From a checkout containing the pinned census runner, set WPT, MANIFEST, URLS, BIN and OUT to the local paths of the WPT checkout, its pinned manifest, a selection, an executable compatible with `pyro serve`, and the output receipt. The reference launcher and exact browser build are identified in provenance.json.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --manifest-inventory --measure-reviewed-skips --generated-globals "$MANIFEST" \
  --dirs webaudio --server wptserve --testdriver --http2 --jobs 2 \
  --tests "$URLS" --bin "measured=$BIN" --out "$OUT"
cargo test --release --offline -p pyro-media --no-default-features --test convolution
LP_BENCH_DIR="$PWD/bench" bun evidence/round6-audio-convolver/workload.mjs measured "$BIN"
```

The compressed page and name tables use control, candidate, and Chrome 158 column order for each scope. The raw directory contains the original per-page results of each direct comparison. The full reference-shard hashes are listed in provenance.json.
