# Color conversion at the sRGB Canvas pixel boundary

The 2D surface stores unorm8 sRGB. This batch converts straight ImageData between that surface and sRGB, linear sRGB, Display P3 and linear Display P3 on reads and writes. Alpha is preserved during primary and transfer conversion. Dirty writes only convert pixels actually copied. Creating an ImageBitmap from ImageData considers the source color encoding. Valid CSS color() paint values are parsed with the already shipped color library.

All 201 ordinary selected canvas pixel-manipulation and wide-gamut URLs were measured with the pinned WPT and CI JSC inputs in inputs.json and paired to the original same-pin Chrome stable census. The original buffer-video study is documented in the [VideoFrame feasibility evidence](https://github.com/povio/pyro/tree/4d09ba7e153ee0d0ad002fded50712919c766db3/evidence/round6-buffer-video-feasibility). This source batch adds no WebCodecs frame API.

| Browser | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP aa0268b47 | 767 | 164 |
| LP 47bff30cb | 1,046 | 163 |
| Local Chrome stable | 1,434 | 188 |

This isolated experiment gained 315 named passes and lost 36. The 36 losses comprise 33 wide-gamut red samples on SVG/PNG image, Blob, bitmap clone or pattern routes and 3 P3 ImageData→ImageBitmap float16 round-trip cases. A corrected sRGB readback exposes the earlier gamut loss during image decoding, bitmap creation or rendering. None of the 36 losses recovered in this candidate. The selected 14 WebCodecs pages remained at zero named LP passes. No code from this experiment was applied to the integration worktree. canvas-summary.json retains all lost names and page outcomes. The two compressed tables preserve original names, duplicate occurrences, page and harness states and missing observations.

The prior browser read sRGB red as [255, 0, 0, 255] when a Display-P3 pixel was requested. This batch returned [234, 51, 35, 255], compared to Chrome's [234, 51, 35, 255]. For the P3 source [117,72,40,255], direct sRGB reads returned [125, 69, 32, 255] on LP and [124, 69, 32, 255] on Chrome. color-probe.cjs also checks a partially transparent crop, dirty-rectangle coverage, linear transfer and a cropped ImageBitmap. probe-errors.json records the maximum byte deviation of each mismatching vector. These probes use a real sRGB backing store.

A separate 640×360 unorm8 write/read loop on an sRGB surface recorded 11 samples per configuration. Warm median durations in milliseconds (ten observations) were {'control': {'srgb': 0.7, 'display-p3': 0.65}, 'candidate': {'srgb': 0.35, 'display-p3': 70.75}, 'chrome': {'srgb': 0.3, 'display-p3': 4.9}} on this machine, with raw results preserved. The original Display-P3 path performed no conversion, so its time covers different work. These are end-to-end JS request/read times and not an isolated arithmetic benchmark.

The LP context still identifies its backing as sRGB when Display P3 is requested. An out-of-sRGB-gamut pixel cannot round-trip through that backing at full gamut. Float16 backing stores and native P3 rendering remain open. No new dependencies or WPT expectations were introduced.

Use the published reviewed WPT runner with the pin recorded in inputs.json:

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests canvas-selection.json \
  --bin "lp=$BIN" --out "$OUT"
node color-probe.cjs colors-lp "$BIN"
node pixel-workload.cjs timing-lp "$BIN"
```

The existing native Canvas JS integration suite passed all 17 tests. video-check-summary.json and its paired tables retain the post-change WebCodecs control.
