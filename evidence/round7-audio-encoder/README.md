# AudioEncoder and real packet round trips

On ef49d6b43 all 16 selected URLs completed and emitted their named results. The eight target URLs had 514/766 passing names on LP and 761/766 in the pinned signed browser. The eight AudioData, encoded audio chunk and packet decoder guards had 88/90 passes on LP and 90/90 in signed Chrome. This selector includes optional AAC and multichannel cases; unsupported outcomes stay in the comparison. No accepted VideoFrame or exposure probe was replayed.

The original signed reference was Google Chrome stable 158.0.8127.37 on macOS arm64, WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Parent and candidate use the pinned, statically linked JSC and the same four-worker WPT command and manifest. chrome158-map.json records all 16 original shard digests; no signed WPT re-execution was required. The exact source tree, executables, JSC archive and selector are recorded in provenance.json. The candidate runs through acquire.sh, native.sh, run.py, and the three direct drivers.

The ordinary round-trip fixture supplies 25 mono 960-sample buffers at 48 kHz, each containing the next part of a continuous 440 Hz sine wave at amplitude 0.4. Each AudioData wrapper closes immediately after encode. The output receipt includes timing, bytes and actual decoder configuration. Frequency is estimated by counting positive-going zero crossings within the middle of the concatenated signal. For signed Chrome's output, both signed Chrome and the LP parent decoded 26 packets into 24,648 samples with RMS 0.282714 and estimated frequency 437.73 Hz.

## Matched results

| Cohort | LP parent | LP candidate | Signed stable | Strict pages parent / candidate / signed |
|---|---:|---:|---:|---:|
| unique (16 URLs) | 602 | 770 | 851 | 6 / 6 / 14 |
| target (8 URLs) | 514 | 682 | 761 | 0 / 0 / 6 |
| IDL (2 URLs) | 513 | 571 | 630 | 0 / 0 / 2 |
| sourceBehaviors (6 URLs) | 1 | 111 | 131 | 0 / 0 / 4 |
| guards (8 URLs) | 88 | 88 | 90 | 6 / 6 / 8 |

All 168 new passes are shared with signed Chrome. There were zero missing, newly reported, LP-only or lost named passes. All 16 pages completed without restarts or watchdog kills. No additional strict pages passed. The candidate retains the full adjacent guard pass set.

Opus runs through the already shipped opus-pure 0.2.2 encoder, with sample buffering/resampling, packet durations, priming metadata, timestamp association, bounded configuration support and real immutable output chunks. AAC, encoding from PCM to a separate linear PCM codec, and unmapped sixteen-channel Opus encoding are unsupported. Native six-channel surround using explicit Ogg packet format is checked by tone recovery; the signed browser rejects the explicit Ogg format query. Ogg here refers to the WebCodecs audio packet and identification header representation. It does not construct a container file.

The final candidate produced real packets from the mono 440 Hz tone that signed Chrome decoded into 24,648 samples with no errors. All four encoder/decoder pairings and their waveform metrics are in the direct receipts. The 44.1 kHz stereo workload resamples to 48 kHz, preserving distinct 330 Hz and 660 Hz tones. Both browsers emit five packets for each four-input flush group; the second group's timestamps begin at the new 900000 µs input time.

At 8 kHz/120 ms, LP accepts and actually delivers 32000 bps mono CBR, and reports 256000 bps unsupported. Signed Chrome reports both supported and delivers 31333.33 and 169333.33 bps respectively. The native test exercises the actual 2460-byte low-clock packet limit and a viable repacketized shorter-subframe mode. The final implementation preserves absent Opus complexity in the supported dictionary; explicit zero remains zero. The same private default is used internally whether the Opus dictionary or only its complexity member was omitted. This correction accounted for 6 fewer matched passes than the initial candidate; the pinned WPT expects a public value where the registration has no default. Earlier measurements are retained under initial/.

Other remaining differences include codec-specific bitrate error classification, optional codec support, a pinned planar join helper assuming all output buffers are no larger than the first priming-trimmed frame, and the out-of-scope VideoEncoder interface. The normal audio tone and cross-decoder traces exercise priming, channel signals and actual amplitudes independently from WPT harness assertions.

The final LP-encoded signal decoded by signed Chrome measured RMS 0.282610, peak 0.401843, frequency 437.73 Hz. LP stereo channel RMS values were 0.282791 and 0.141505, at estimated frequencies 328.55 and 660.29 Hz.

Final native checks passed 14 Rust tests and three JavaScript control-queue tests. The native encoder tests verify independent decoding across buffer boundaries, sample rates and channel layouts, real packet durations, and strict bitrate capability bounds. Use node decode-tone.cjs candidate-on-signed with the signed binary launcher and candidate-tone.json to repeat the signed decode of LP-produced packets.

Independent evidence 3509e4c9757debe329588d3985bc2c90381f8aa9 confirms the matched candidate on immutable executable SHA-256 cfeae431670291262954b37231d842b50c2e6fea8c47638a281370dc7f7c5a2c, including named results, native-resource capability boundaries and ordinary audio controls.
