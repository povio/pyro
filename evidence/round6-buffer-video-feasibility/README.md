# Buffer-backed WebCodecs VideoFrame feasibility

The exact LP control at aa0268b479a6b463da16910b4b743cf38901d962 completed 14 ordinary WPT URLs in Window and dedicated workers with zero browser restarts. Both local engines used the same WPT revision a421f2fea5eb7f91e0db399e44b0fb0779ee1603. LP used CI JavaScriptCore 0e0dd7038a7a-6f4649e395e0 and binary SHA-256 9623217e05de0c6866256ce4f855edabf2ca810eb0d6a3ae51a93fd5ba50f7a8. Chrome's 157.0.8103.80 macOS arm64 WPT profile results come from the [published census](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census). The direct normal probes ran separately on the local Chrome executable and on the exact LP control. No encoded media or native failure case was replayed.

LP passed 0 of 1,016 named cases, Chrome 878. The two copyTo-rgb URL variants account for 618 Chrome-only passes, 309 in each environment. The repository's Web Audio or HTMLVideoElement code does not expose a WebCodecs VideoFrame. The 354 registered names per RGB page include four-byte RGB and planar I420 inputs, source metadata, plane copying, output color spaces, canvas drawing and non-buffer source cases. Raw per-browser names and page outcomes are included.

## Normal pixel measurements

A 640×360 buffer was constructed as straight RGBA and as three-plane I420. Each iteration constructs a frame, clones it, closes the source, copies native planes and sRGB RGBA, then closes the surviving clone. The workload checks native bytes at deterministic offsets and records a checksum of the converted result. Local stable Chrome finished all 15 iterations for each format. Across 14 warm iterations the median was 0.050 ms for RGBA and 0.450 ms for I420. These are in-page timer readings for the combined operations on this machine; individual short RGBA iterations hit timer granularity. LP reported VideoFrame unavailable, so it has no comparable duration. The two-pixel-dimension probe records native layout, timestamp, native plane bytes, converted bytes and canvas results for deterministic small samples.

The original 2D canvas returned [255,0,0,255] for sRGB red read with display-p3; Chrome returned [234,51,35,255]. In the converse direction, LP read the P3 source [117,72,40,255] back as [117,72,40,255] in sRGB, Chrome as [124,69,32,255]. On the LP control, a context requested as Display P3 reports sRGB storage. A copyTo implementation that merely agreed with this canvas would give a misleading result.

## Implementation boundaries

* The current pyro-media Frame owns one already-converted, opaque RGBA buffer with its dimensions and presentation time. It does not retain original Y/U/V planes, plane offsets, strides, coded versus visible geometry, alpha, orientation or source color metadata. Its decoder-private YUV conversion supports matrix/range and subsampling; color primaries and transfer functions require a separate conversion step.
* Buffer-backed frames need checked plane layout and padded-row access, odd-size chroma dimensions, cropping and format-preserving copies. A shared frame resource must survive closing a sibling, and transferred frame objects must detach only after successful structured serialization.
* The API is exposed on Window and dedicated workers. Current canvas and media cloning paths specifically handle ImageBitmap, OffscreenCanvas and other existing objects. Canvas drawing and frame-from-canvas construction must preserve source lifetime, orientation, display dimensions and color provenance. A complete image-source implementation cannot silently clip a Display-P3 source to the existing sRGB bitmap before future frame copies.
* No new VideoFrame interface was exposed in this assessment. These measurements support separate work on the plane-backed resource, its copy and color conversion, and the canvas/structured-clone integrations before claiming the 618 names.

## Reproduction

Set WPT and MANIFEST to the pin above and its manifest, BIN to the LP executable and OUT to a result path. Use the reviewed census manifest runner.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
node normal-workload.cjs workload-lp "$BIN"
node normal-workload.cjs workload-chrome chrome
node probe.cjs colors-lp "$BIN"
```
