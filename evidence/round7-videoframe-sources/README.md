# Canvas and bitmap sources for VideoFrame

Before this change VideoFrame accepted a byte buffer or another frame. Constructing a frame from an HTML canvas, an OffscreenCanvas or ImageBitmap raised TypeError. The general WebCodecs IDL runner already emitted all names on this parent; failures in VideoFrame object assertions included construction from a real ImageBitmap. The 16 target URLs reported 537/795 passing names and two strictly passing pages on LP. Signed Chrome stable 158.0.8127.37 reported 751/795 and ten. The twelve neighboring guards reported LP 123/137 (nine strict) and signed 127/137 (eleven strict).

The exact shared parent is de702d8d69a3c46046f6ca865a0deaa0ec3d9e3a, tree b97125ce6f819af3e8c711385768d098984c6228. The parent was built once for the editing and VideoFrame work. Its copied and independently rehashed binary was 7ec8b3bb80cae88d7f10acdb0a9a94640750a9ae7c1f8775f7bf98d896325a02. Both matched runs use the same pinned static JSC, four page jobs, manifest-generated globals and no native retry. The original signed receipts are bound to the WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 shard digests and browser metadata in chrome158-map.json. The selector SHA-256 is a205769188f5f409080c7f065259978b254d50d23e7d475c262dc1a51893329c.

The merged WebCodecs and AudioPlaybackStats secure-context exposure was checked with that same parent binary. In a trustworthy HTTP loopback Window, isSecureContext was true and all four globals (AudioPlaybackStats, ImageDecoder, ImageTrackList, ImageTrack) were present functions. In a machine-local non-loopback HTTP Window, isSecureContext was false and all four were absent and undefined. See exposure.cjs, base-exposure.json and base-provenance.txt.

Direct controls repaint the source after frame construction, close the original frame and inspect the clone's rendered pixels, display dimensions and timing. They cover DOM canvas, OffscreenCanvas, ImageBitmap and a raw buffer frame, plus cropped and transformed canvas frames with alpha keep/discard. Commands are in acquire.sh, run.py, native.sh and the direct scripts. Cross-origin policy fixtures, native fault holds, and unowned HTML video/image/GPU sources remain outside this cohort.

## Results

| Corpus | LP parent | LP candidate | Signed Chrome | Strict pages parent / candidate / signed |
|---|---:|---:|---:|---:|
| unique (28 URLs) | 660 | 767 | 878 | 11 / 18 / 21 |
| target (16 URLs) | 537 | 644 | 751 | 2 / 9 / 10 |
| IDL (2 URLs) | 471 | 513 | 630 | 0 / 0 / 2 |
| sourceBehaviors (14 URLs) | 66 | 131 | 121 | 2 / 9 / 8 |
| guards (12 URLs) | 123 | 123 | 127 | 9 / 9 / 11 |

The candidate gained 107 existing named passes with no losses and no newly reported or missing occurrences. Eighty-one gains are shared with signed Chrome. Thirteen in each execution environment are alpha-discard assertions that LP passes and signed Chrome fails. There are 149 signed-only and 38 LP-only names across the 28 URLs. All pages completed without restarts or watchdog kills.

The direct snapshot and Web IDL conversion receipts match signed Chrome exactly. Display dimensions truncate finite fractional input and reject out-of-range values before later dictionary getters or bitmap capture. A 6×4 explicit display size with 90° rotation reports 4×6. The geometry receipt has one pixel-channel difference: clearing the alpha channel with alpha:"discard" yields opaque alpha 255 when LP copies the frame to RGBA, while signed Chrome reports 0 for the transparent source pixel. Remaining differences also include absent encoders, HTML video/image element construction, shared buffers and the existing close-time colorSpace behavior. The canvas store currently reports RGBA/sRGB conservatively; it does not expose independent opacity metadata. The confirmed secure-context union smoke used the exact shared parent and made no additional build.

Native verification passed 13 Rust tests and two JavaScript fixture tests, including retained raster accounting through the last frame close, source repaint independence, placeholder canvas updates, frame orientation, the unchanged byte-buffer path and Web IDL conversion order.

Independent QA committed 08f6f6bbd599b6162c1d8840b5bbc27b0e638106 after rerunning all 28 URLs and the three direct controls on executable 1fd5d2a0da8da44219ac4f3c888376194c96e3df8d429923c86b400db137382f. Named occurrences, guard outcomes and the direct JSON results agree with the author run.
