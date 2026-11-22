# WebCodecs H.264 VideoEncoder

The integrated parent a1ede01701dbc4a4e97cb2006af53531fa03d2ee had VideoFrame and VideoDecoder but no VideoEncoder. Both pinned IDL pages already reported the complete list, including 58 observed VideoEncoder failures. The 69 target URLs comprise every eligible video-encoder and reconfiguring-encoder variant plus the two shared IDL pages. The 17 guards cover existing VideoDecoder orientations, encoded video chunks and decoded image orientation. All original codec variants, including unsupported ones, remain in the selector.

The exact parent completed 59 target pages, returned five setup errors from missing VideoEncoder in windows and timed out on five dedicated-worker rescaling pages. Its 572/1136 target named passes compare with 1134/1136 in signed stable. All guards completed, with LP 34/50 and signed 50/50. Both runs reported every named occurrence. The parent had zero native restarts or watchdog kills. For the signed projection, one restart was recorded across its 73 backing shards and selected URL attribution is unknown.

The reference is Google Chrome stable 158.0.8127.37 on macOS arm64, WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. LP uses the pinned, patched static JSC on both builds. The selector SHA-256 is 03d45955f51c811d9107d3815ea5bfdb2790f5a36dddd7634d9c68d4e31624a5. All 86 unique URL-to-shard mappings and raw digests are in chrome158-map.json. Build/run commands and direct controls are in acquire.sh, native.sh, run.py, pattern.cjs, profiles.cjs and decode-pattern.cjs.

The normal workload encodes ten moving two-color frames in both AVCC and Annex B. It closes every source wrapper immediately after enqueue, requests a keyframe twice per five-frame group, flushes and reconfigures from 64×48 to 80×48. The decode receipt records dimensions, timestamps and representative patch pixels; each packet's bytes and decoder configuration are archived. The profile probe requests Baseline, Main and High 3.0 plus High 6.2 on 64×48 input with a software preference and reads actual AVC configuration-record bytes. Signed Chrome emits level 1.1 for these small frames, independently of the requested level ceiling.

## Matched results

| Corpus | LP parent | LP candidate | Signed stable | Strict pages parent / candidate / signed |
|---|---:|---:|---:|---:|
| unique (86 URLs) | 606 | 840 | 1184 | 9 / 26 / 84 |
| target (69 URLs) | 572 | 806 | 1134 | 0 / 17 / 67 |
| IDL (2 URLs) | 571 | 629 | 630 | 0 / 1 / 2 |
| sourceBehaviors (67 URLs) | 1 | 177 | 504 | 0 / 16 / 65 |
| guards (17 URLs) | 34 | 34 | 50 | 9 / 9 / 17 |

The 234 newly passing names all pass in signed Chrome. No previously passing name was lost and no names were added or missing. The candidate retains 344 signed-only names. The WebCodecs IDL coverage improved from 571 to 629 of 630; the one remaining failure concerns the existing VideoDecoder interface property in dedicated workers.

Page-level changes are recorded independently of named wins. LP completed 76/86 pages both before and after the change. Four H.264 rescaling pages recovered from errors/timeouts and completed. Two AV1/VP8 flush window pages transitioned from complete to harness error; both corresponding worker pages transitioned from complete to timeout. Their parent subtests failed on the missing encoder; configuring an unsupported codec now reports an asynchronous NotSupportedError. The final outcomes were 76 complete, eight harness errors and two timeouts. Three unsupported rescaling worker pages also transitioned from timeout to error. There were no LP native restarts or watchdog kills. All 17 guard pass sets were unchanged.

The candidate uses the shipped OpenH264 CPU encoder for Baseline, Main and High 8-bit I420, supporting AVCC and Annex B. The output carries real parameter sets, BT.709 limited VUI, keyframes and timestamps. The three profiles requested at level 3.0 produced actual AVC configuration bytes [1,66,192,30], [1,77,76,30] and [1,100,12,30]. Signed Chrome emitted level 1.1 for the same small inputs. The candidate rejects unsupported profile/level combinations, including High 6.2. Other codec families, hardware-only requests, and unavailable rate-control modes remain explicit limitations.

LP and signed Chrome each decoded all ten LP-produced frames in both formats with complete, original encoder metadata. Frame dimensions, timestamps and both expected color patches agreed for every frame. In each format the keyframes occurred at indices 0, 3, 5 and 8. On an identical configuration, width change and then reset, the sequence of metadata emission was true, false, true, true in LP and Chrome. The test also mutates callback-visible description bytes to verify that the next comparison uses an independent copy.

The candidate decoder accepts explicit limited-range BT.709 H.264 and returns tagged I420 planes. An independent 64×48 saturated-red stream supplied with the evidence has SMPTE 170M primaries, transfer and matrix and TV range, independently checked with FFmpeg SPS trace and ffprobe. On this differently tagged stream the candidate returns BT.709 metadata and renders [255,40,0,255] with an explicit BT.709 override; with no override or an empty dictionary it preserves the parent's RGBA [255,0,0,255] path. Signed Chrome retains the stream's SMPTE 170M and renders [254,0,0,255] for this fixture even when explicitly given BT.709. On signed Chrome's ordinary encoder output, complete metadata specifies sRGB transfer and full range. LP rejects that combination as unsupported. Initial projected-metadata readbacks omitted this color dictionary, so they are archived under initial and are excluded from the full-metadata interoperability claim.

Commands: acquire.sh runs the LP checks and WPT. To repeat the signed decode of LP-produced packets, run node decode-pattern.cjs candidate-on-signed with the signed launcher and candidate-pattern.json. make-color-source.sh builds the independent tagged fixture; color.cjs checks both the originally tagged and explicitly overridden path. Browser JSON files contain full packet bytes, frame metadata, samples and per-format errors. No shipped dependency or test expectation changed.

Native verification passed 12 Rust tests and 6 JavaScript fixture tests. These include underlying packet/keyframe/level/format checks, off-center crop and scaling, buffer retention and the independently tagged I420 override.

Independent QA evidence 60c6ac17e67035419c8acde4bee57b87309db1c2 confirms the fixed 86-URL corpus and ordinary packet, frame, color, profile and reconfiguration controls on the final executable SHA-256 6aab72137633eda70f0f30d2f0aabb1cb3f78aeb5df1b5426fcba47539e3cabe.
