# WebCodecs audio on pinned JSC

The target comprises 53 URLs for AudioData, AudioDecoder, encoded audio, messaging, and the general WebCodecs interface inventory. Nine adjacent chunk/frame guards include one URL also in the target. The union has 61 URLs. Both cohort memberships and the execution selector are preserved. A separate 14-URL orientation guard exercises the shared audio/video control queue using four normal video frames over two decoder configurations.

Parent and candidate use the same patched static JSC archive and wptiter render build. Source and binary digests are in provenance.json. The signed reference reuses Chrome stable 158.0.8127.37 on macOS arm64 with WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The original acquisition used four workers and one fresh context per page. Each Chrome projection maps every URL to the original raw shard digest. One supplemental video guard source shard had a process restart with unknown URL attribution.

The direct browser controls cover a stereo PCM payload with positive and negative samples, chunk snapshot, shared and independent AudioData resources, a worker round trip, reset and close. A separate supplemental multi-codec amplitude workload exited in the signed browser before a result was returned. It remains held and has no per-codec attribution. Native fixture tests and the frozen WPT exercise normal compressed packets separately.

Use candidate.sh with an immutable source commit, run ordinary.cjs, close.cjs and transport.cjs, then run `python3 run.py candidate`, `python3 video-queue/run.py candidate` and `./native.sh`. The acquisition, reports and selector hashes are included. Raw page data preserves exact ordered subtest names, page status and optional/missing results.

A bare FLAC or Vorbis configuration passes the codec capability query in both browsers. When the application actually configures a decoder without the registration-required description, both browsers deliver NotSupportedError, reject flush with the same exception, and close. Bare pcm-s16 configures and flushes normally. The frozen codec WPT supplies descriptions for real FLAC and Vorbis packets. See capability.cjs and its three receipts.

The first candidate converted linear PCM through planar float. A same-format round trip lost integer precision, including U8 255 returning 254 and S32 −16777217 returning −16777216. The signed stable browser preserved both input format and samples. The first and intermediate candidate receipts retain this discrepancy.

## Matched results

| Cohort | URLs | Parent named passes | Candidate named passes | Signed named passes | Gains | Losses |
|---|---:|---:|---:|---:|---:|---:|
| Unique audio and neighboring video corpus | 61 | 189 | 673 | 1156 | 484 | 0 |
| Audio target | 53 | 183 | 667 | 1118 | 484 | 0 |
| Neighboring guards | 9 | 14 | 14 | 46 | 0 | 0 |
| Object and transport target | 20 | 40 | 135 | 153 | 95 | 0 |
| Decoder target | 31 | 0 | 327 | 335 | 327 | 0 |
| IDL target | 2 | 143 | 205 | 630 | 62 | 0 |
| Supplemental video queue | 14 | 12 | 12 | 28 | 0 | 0 |

The 61-URL union reaches 40 strict pages, from 6, against 60 in signed stable. Candidate page outcomes are {'complete': 60, 'error': 1}; 53/53 targets complete. The sole page error is a pre-existing neighboring VideoFrame graph serialization limitation. There are 483 signed-only passes, 0 LP-only passes and 294 missing signed occurrences. No LP restart or watchdog kill occurred in either cohort.

The final lossless PCM path retains U8/S16/S32 interleaved bytes, widens S24 to signed 32-bit, and preserves floating-point bits in planar F32 output. The five completed direct fixtures byte-match their signed stable JSON, including extrema and S32 ±16777217. An additional signed stereo-layout probe ended before returning results; this second supplemental workload is held with no codec attribution.

Known remaining differences include unavailable shared buffers in cross-origin-isolated contexts, AudioEncoder and ImageDecoder interfaces, unfinished VideoFrame serialization, and positive Opus/Vorbis priming timestamps. The packet path accounts for priming without conditioning timestamps on sign. Occurrence records include every general IDL name reported or missing on each browser.

Native checks passed 22 Rust tests and the existing Node VideoDecoder orientation fixture. They exercise five packet tests, seven whole-file codec fixtures, resampling, video decoder queue continuity, PCM format conversions, and cross-realm message transport. The compressed-packet test independently checks decoded tone amplitude and frequency. Source changes add no dependencies and leave WPT expectations unchanged.

Independent candidate-only verification used the immutable 4a0c32dc6 executable, SHA-256 8f77d1869b20c08734cecbe309c3f40e443ec89607e6b9b3ceef36b7cab58bbf. It confirmed +484/−0, no changes to the 14 video guard URLs and exact signed matches on all five direct controls. Independent evidence commit 5b7eed7c7e81b7bafebe640d2b5d7f539758d7d2.
