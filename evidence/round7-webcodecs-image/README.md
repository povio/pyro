# WebCodecs image decoding on pinned JSC

The pinned WebCodecs IDL setup fetches four-colors.png and waits for ImageDecoder track metadata before adding test objects for every WebCodecs interface. On the integrated audio parent, this left 294 signed subtest names unreported. Seven ImageDecoder target URLs had 205 passes among 360 reported names. Signed stable passed all 654 names. Seven neighboring canvas ImageBitmap, AudioData and encoded video guards passed 103/113 names in both browsers.

The implementation decodes still PNG and JPEG images and composes GIF frames using the shipped raster dependencies. It exposes ImageDecoder, ImageTrackList and ImageTrack in secure windows and dedicated workers. ReadableStream input, metadata, track selection, reset, close, frame pixels and cloned frame reservations are covered by direct or native tests. The JPEG path retains EXIF transforms as VideoFrame metadata. APNG, partial-pixel output and optional resizing remain uncovered. The desired image size is a hint; the PNG fixture returns the original 320×240 image in both browsers. Two mixed ordinary/crash-regression WPT pages and all prior holds remain excluded.

Four direct controls record normal PNG pixels and GIF frame colors, completed two-chunk PNG stream lifecycle, type/size/timing/old-track metadata, and an EXIF-mirrored JPEG on default and orientation-disabled canvases. The candidate follows the current draft for two MIME details: it retains the supplied type spelling and rejects non-image media types with TypeError in isTypeSupported. Signed stable lowercases the type and resolves false for text/plain. Decoder close releases codec resources without cancelling the caller's stream. This latter correction was checked against the specification and source. The stream control completes before close.

Both LP runs use the exact same static patched JSC archive, four page workers, generated globals, testdriver, HTTP/2 and no native retry. Chrome stable 158.0.8127.37 on macOS arm64 was projected from the original signed shards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. All 14 unique URLs are bound to the selector and original raw shard digests in chrome158-map.json. Run candidate.sh with the immutable revision, then the four direct controls, python3 run.py candidate, python3 report.py, python3 families.py and ./native.sh. acquire.sh drives the build, direct controls, matched WPT and report. Each missing, newly reported, nonpassing and passing occurrence is retained in named.jsonl.gz.

## Matched gains and remaining gaps

| Cohort | Parent passes | Candidate passes | Signed stable passes | Parent/candidate/signed strict pages |
|---|---:|---:|---:|---|
| unique (14 URLs) | 308 | 594 | 757 | 6/9/13 |
| target (7 URLs) | 205 | 491 | 654 | 0/3/7 |
| IDL (2 URLs) | 205 | 471 | 630 | 0/0/2 |
| imageBehaviors (5 URLs) | 0 | 20 | 24 | 0/3/5 |
| guards (7 URLs) | 103 | 103 | 103 | 6/6/6 |

The candidate gained 286 named passes and lost zero. Of the 286 gains, 90 were previously observed nonpasses and 196 were newly reported IDL object tests. The other 98 formerly missing names now produce nonpassing results. Both LP runs and the signed projection completed every page, with zero restarts or watchdog kills. All video frame EXIF orientation WPT cases pass. The 163 signed-only names remaining include IDL for absent encoders, the VideoFrame ImageBitmap-source constructor gap and shared-buffer input. Neighbouring guard pass sets are identical.

The initial candidate e005f3dc7 and its receipts are retained. Its +268 gains preceded the promise-getter receiver correction and the destination canvas orientation correction. The final pixels, completed-stream lifecycle and orientation JSON match the signed control byte for byte; metadata differs only on the two MIME details described above. Both EXIF-derived and explicitly authored frame orientation are suppressed during a draw to a destination with computed image-orientation:none; frame metadata and the from-image destination retain the original transform. The normal direct control alternates destinations on the same frame, on its clone and on a raw 90-degree frame.

Final verification passed 11 Rust tests and the existing Node VideoDecoder orientation fixture. Independent QA reran all 14 URLs on executable bca2df13a5ad8966cc55a0730a60774a6848d7e8f2f7253fee915933523945e6 and confirmed the gains, exact guard pass sets, four control receipts and zero prior losses. Independent evidence commit f1ee95d4460ff1b4751c42bb9da64ad6c43e9fd2.
