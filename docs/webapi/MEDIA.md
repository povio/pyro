# Media elements

`crates/lp-webapi/src/media.rs` runs `HTMLMediaElement` (`<audio>`,
`<video>`, `new Audio()`) like Chrome: the resource is fetched, its
container headers are read in pure Rust
(`crates/lp-webapi/src/media_format.rs`), and the element walks the ready
states and plays on a clock. A `<video>`'s frames are decoded on demand
(`crates/lp-media`, AV1 and H.264) for screenshots, `drawImage`,
`createImageBitmap` and `requestVideoFrameCallback`; an element's audio is
never decoded (there is no output device). Web Audio
(`crates/lp-webapi/src/webaudio.rs`) decodes audio files and renders
offline graphs. `TimeRanges`, `MediaError`, `TextTrackList` and
`VideoPlaybackQuality` are made in JS (`crates/lp-webapi/src/media.js`) on a
realm's first use.

Nothing runs before a page asks: no decoder is made, no thread started and
no table built until the first frame, `decodeAudioData` or Web Audio
interface is used. A page without media pays an empty-map test per render
snapshot and per animation frame.

## Loading

The DOM (`crates/lp-dom/src/media_element.rs`) queues what starts a load for
a document with a browsing context: `src` set or changed (the media element
load algorithm), a media element inserted or created by the parser (resource
selection, when its `networkState` is `NETWORK_EMPTY` and it has a source),
a `<source>` child inserted or removed, and a media element removed from
the document.
The queue is drained with the iframe post-connection steps, after the
native call that changed the tree and at the event loop's checkpoints.
Documents that never had a media element pay one flag test per attribute
change, insertion and removal.

- Sources: `src` (attribute mode), `srcObject` (a `Blob` is read directly;
  a `MediaStream`/`MediaSource` cannot be read and fails), or `<source>`
  children in order. A `<source>` whose `type` gets `""` from
  `canPlayType()`, or without `src`, gets an `error` event and is not
  fetched; a candidate whose fetch or probe fails gets `error` and the next
  one is tried; out of candidates the element waits for another `<source>`.
  Removing the `<source>` being tried keeps the selection's place (the next
  candidate is the one after it), as HTML's source pointer does.
- The fetch goes through the embedder (`PageHooks::fetch_media`;
  lp-browser's `crates/lp-browser/src/media.rs`) as an Audio or Video
  request: CDP reports resource type `Media`, `Sec-Fetch-Dest` is
  `audio`/`video`, and Playwright routes see it. `blob:` URLs read the
  page's blob store. Only the container headers are needed: a body whose
  `Content-Length` is at most 16 MB is read whole; a longer one, or one
  without a length (a live stream), is cut once its headers are in (from
  64 KB, at the latest at 16 MB) and the transfer stops (CDP sees the
  request canceled). An MP4 whose `moov` box comes after that prefix is
  read with `Range: bytes=<moov offset>-` requests (at most four box hops;
  a server that ignores `Range` fails the load). Starting a new load drops
  (cancels) the fetch in flight.
- Durations from a cut body: header values where the container has them
  (MP4, WebM, FLAC, WAV with a data size, MP3 Xing/VBRI); otherwise
  proportional to `Content-Length` (Ogg, ADTS, CBR MP3, WAV without a data
  size), or `Infinity` without a length.
- The body is sniffed, as Chrome does (the MIME type does not matter):
  MP4/QuickTime (`mvhd`/`mehd`/`mdhd` duration, `tkhd` size with quarter
  turns applied, sample entry codec), WebM/Matroska (`Info` duration,
  `Tracks` codec and pixel/display size), Ogg (Vorbis, Opus, FLAC; duration
  from the last granule position), WAV (PCM 8/16/24/32, float, A-law,
  µ-law), MP3 (Xing/Info/VBRI frame count, else CBR length), FLAC
  (STREAMINFO) and ADTS AAC. Codecs Chrome cannot play (Theora, MPEG-4
  Part 2, AC-3, ...) count as absent.
- A playable resource fires, one task per step: `progress`,
  `durationchange`, `resize` (video with a size), `loadedmetadata`
  (readyState 1); `loadeddata` (2); `canplay` (3, then `playing` if play()
  is pending); `canplaythrough` (4, then autoplay); `suspend` with
  `networkState` `NETWORK_IDLE`. `duration` is `Infinity` when the
  container does not say. A `currentTime` set before the metadata seeks
  once it is known.
- A network error, a non-2xx status, an empty body, or a body no demuxer
  recognizes fires `error` with a `MediaError` of code 4
  (`MEDIA_ERR_SRC_NOT_SUPPORTED`) and Chrome's message
  (`MEDIA_ELEMENT_ERROR: Format error`, `DEMUXER_ERROR_COULD_NOT_OPEN: ...`,
  `DEMUXER_ERROR_NO_SUPPORTED_STREAMS: ...`); pending play() promises reject
  with NotSupportedError.
- `preload="none"` (without `autoplay`): `loadstart`, then `suspend` and
  `NETWORK_IDLE`; nothing is fetched until `play()`.
- A media element delays its document's `load` event from resource
  selection until it reaches `HAVE_CURRENT_DATA`, fails, suspends or waits
  for a `<source>` (HTML "delaying-the-load-event flag"), so after
  `page.goto()` the metadata is known.
- A new load (new `src`, `load()`, `srcObject`) fires `abort` (while
  loading) and `emptied`, resets readyState, duration, video size and the
  playback position, pauses the element (rejecting pending `play()` promises
  with AbortError) and discards the earlier load's queued events.

## Playback

- The playback position follows the document clock (`performance.now()`,
  so `Emulation.setVirtualTimePolicy` and virtual time move it): while the
  element is potentially playing (not paused, readyState at least
  `HAVE_FUTURE_DATA`, not seeking) `currentTime` advances at
  `playbackRate`. One timer per playing element fires `timeupdate` every
  250 ms and the end; a paused, ended, seeking or rate-0 element has no
  timer. Media timers do not keep `run_until_idle` busy.
- `play()`: an element whose source failed rejects with NotSupportedError.
  Otherwise it unpauses, fires `play`, then `waiting` (no data yet) or
  `playing`, and resolves on `playing`. At the end it seeks to 0 first;
  after `preload="none"` it starts the fetch.
- The end: with `loop`, a seek to 0 (`seeking`, `timeupdate`, `seeked`) and
  playback goes on; otherwise `timeupdate`, `pause` (paused becomes true)
  and `ended`; `ended` is true.
- `currentTime` (and `fastSeek`) seeks once metadata is known: clamped to
  [0, duration], `seeking` is true until the next task, which fires
  `timeupdate` and `seeked`. Before metadata it sets the default playback
  start position.
- `pause()`, and removal from the document: `timeupdate`, `pause`, pending
  `play()` promises reject with AbortError, the clock and its timer stop.
- `buffered` and `seekable` are `[0, duration]` once the metadata is known;
  `played` holds the ranges the clock moved through.
- `volume` (IndexSizeError outside [0, 1]), `muted` (starts from the
  `muted` attribute, which `defaultMuted` reflects), `playbackRate`
  (NotSupportedError outside Chrome's range 0.0625 to 16, except 0),
  `defaultPlaybackRate` and `preservesPitch`; `volumechange` and
  `ratechange` fire when they change. Non-finite values throw TypeError.
- `videoWidth`/`videoHeight` give the video track's size once readyState is
  at least `HAVE_METADATA`; the size also becomes the `<video>`'s intrinsic
  size for layout and screenshots (`lp_dom` natural size, the render
  tree's `natural=` state).
- `canPlayType()` answers as desktop Chrome for the codecs this build
  decodes: `maybe` for a supported container without codecs (`probably`
  for `audio/mpeg`, `audio/mp3`, `audio/flac`, `audio/aac`), `probably`
  when every codec is supported (H.264 with the `h264` feature, AV1 with
  the `av1` feature, AAC, MP3, Opus, Vorbis, FLAC, PCM in the containers
  Chrome allows them in), `""` for anything else: VP8, VP9 and HEVC (no
  decoder, see "Choosing the codecs"), Theora, MPEG-4 Part 2, unknown types
  or codecs, a video codec in an `audio/` type. A `<source>` whose `type`
  gets `""` is skipped. A resource without a `type` is still read whatever
  its codec: a VP9 WebM loads its metadata and plays on the clock, it only
  paints no frame.

## Autoplay

Chrome's default policy: muted media may play; unmuted media needs sticky
user activation of its document (`PageDom::notify_user_activation`), set by
trusted keydown (not Escape), mousedown/mouseup and touchend input, and by
CDP `Runtime.evaluate`/`callFunctionOn` with `userGesture: true` (which
Playwright sends with every `page.evaluate`). Without it, `play()` rejects
with NotAllowedError and the `autoplay` attribute leaves the element paused.

## Video frames

A `<video>` whose resource has a video track keeps the fetched bytes (at
most the 16 MB prefix read above) when this build has a video decoder. The
first time a frame is needed, `lp_media::video::Player` lists the track's
samples from the container (our own readers: the MP4 `moov` sample tables
`stts`, `ctts`, `stsc`, `stsz`, `stco`/`co64`, `stss` and the edit list;
WebM/Matroska `SimpleBlock`s and `BlockGroup`s), or drops the bytes when
the codec has no decoder. `frame_at(currentTime)` decodes from the key
frame before the frame shown at that time and converts only that frame to
RGBA (BT.601/709/2020 as the stream says, limited or full range). During
playback the decoder goes on from where it stopped; a seek backwards or
past the next key frame starts again at a key frame. One decoded frame is
kept per element.

- AV1: rav1d (Rust port of dav1d) through its dav1d C API, one thread,
  one frame of delay; 8, 10 and 12-bit; 4:0:0, 4:2:0, 4:2:2, 4:4:4.
- H.264: OpenH264's decoder, Annex B units made from the `avcC` samples.
  Each sample's index rides on OpenH264's input timestamp, so B-frames come
  back in presentation order. Constrained Baseline, Main and High
  (progressive) streams decode.
- Painting: HTML's "show poster flag" is set by each load and cleared by
  `play()` and seeking. While it is set, a `<video>` with a `poster` paints
  the poster; otherwise, from `HAVE_CURRENT_DATA`, it paints the frame at
  its current position (`PageDom::video_bitmaps`, sent with the render
  snapshot like canvas bitmaps; lp-render puts the poster back when the
  frame goes away). A video without a decodable track paints its poster,
  or nothing. Blitz fits either by `object-fit` (`contain` by default). The
  intrinsic size is the video track's size once known, else the poster's,
  else 300x150.
- `drawImage(video)` and `createPattern` draw the current frame;
  `createImageBitmap(video)` copies it (InvalidStateError without one). A
  cross-origin video fetched without CORS taints the canvas.
- `requestVideoFrameCallback(cb)` / `cancelVideoFrameCallback(id)`:
  callbacks run in the window's animation frame, before the
  `requestAnimationFrame` callbacks, when the video presents a frame it had
  not presented to them (the first frame once data is there, each new frame
  during playback, the frame after a seek), with `(now, { presentationTime,
  expectedDisplayTime, width, height, mediaTime, presentedFrames })`.
  Animation frames are asked for only while a video with callbacks plays or
  may present a new frame (data, play, pause, seek). A video without a
  decoder presents a "frame" whenever its position changes.
- `getVideoPlaybackQuality()`: `totalVideoFrames` counts the frames
  presented to frame callbacks; nothing is dropped or corrupted.

### Choosing the codecs

Researched in November 2026 for a headless browser that runs tests and
scrapes pages: each codec's licence, build and binary cost against how
often pages use it.

| Codec | Crate | Licence | Build | Decision |
| --- | --- | --- | --- | --- |
| AV1 | `rav1d` 1.1 (Rust port of dav1d) | BSD-2-Clause | Pure Rust. Its assembly needs nasm on x86_64 and the published crate lacks a header its arm64 assembly includes, so it is built without (`default-features = false`). | In: cargo feature `av1`, on by default. |
| H.264 | `openh264` 0.9 (Cisco's C++ decoder, compiled from source by `openh264-sys2`) | BSD-2-Clause (see the patent note) | A C++ compiler (`cc`); nasm is optional and skipped when absent. | In: cargo feature `h264`, on by default. |
| VP8, VP9 | `vpx-rs`, `env-libvpx-sys`, `libvpx-sys` bind libvpx | BSD-3-Clause | A system libvpx (pkg-config), or libvpx's own configure/make with yasm/nasm. No pure-Rust decoder exists: `oxideav-vp9` is an empty scaffold, `image-webp`'s VP8 decodes key frames only and is private. | Out: the npm binary would need a system library or a hand-maintained libvpx build. |
| HEVC | none usable (`libde265` bindings need the C library) | LGPL-3.0, plus patent pools | C library | Out. |
| Audio | `symphonia` 0.6: MP3, AAC-LC, Vorbis, FLAC, PCM, ALAC, ADPCM in WAV, MP4, Ogg, Matroska/WebM, AIFF, CAF | MPL-2.0 | Pure Rust | In. |
| Opus | `opus-pure` 0.2 (Symphonia has no Opus decoder; `opus-decoder` 0.1 overflowed a shift on our fixtures) | BSD-3-Clause | Pure Rust, no dependencies | In. |

H.264 patents: OpenH264's code is BSD-licensed, but Cisco's royalty-free
patent licence covers only the binaries Cisco builds and distributes. A
lightningpanda binary that compiles OpenH264 from source ships an H.264
decoder without that coverage, as FFmpeg-based builds do. A distributor
who needs a binary without it builds
`cargo build --release -p lightningpanda --no-default-features --features render,av1`:
`canPlayType('video/mp4; codecs="avc1.42E01E"')` then answers `""` and
H.264 videos paint their poster.

Release binary size (macOS arm64, `cargo build --release -p
lightningpanda`, unstripped as shipped), measured on this change:

| Build | Size | Delta |
| --- | --- | --- |
| Before (no decoding) | 47.80 MB | |
| `--no-default-features --features render` (Symphonia, opus-pure, Web Audio, video plumbing; no video codec) | 50.48 MB | +2.67 MB |
| `--features render,av1` (+ rav1d) | 51.90 MB | +1.42 MB |
| default (`render,av1,h264`; + OpenH264) | 52.45 MB | +0.56 MB |
| Total | | +4.65 MB (+9.7%) |

## Web Audio

`crates/lp-webapi/src/webaudio.rs` and `webaudio.js`, made on a realm's
first use of any of `BaseAudioContext`, `AudioContext`,
`OfflineAudioContext`, `AudioBuffer`, `AudioParam`, `AudioNode`,
`AudioDestinationNode`, `AudioScheduledSourceNode`,
`AudioBufferSourceNode`, `OscillatorNode`, `ConstantSourceNode`,
`GainNode` and `OfflineAudioCompletionEvent`.

- Graph: `connect()` to nodes and to AudioParams, `disconnect()` in every
  overload. A node's input sums its sources after up/down-mixing to the
  computed channel count (`channelCount`, `channelCountMode`,
  `channelInterpretation`: speaker rules for mono, stereo, quad and 5.1,
  discrete otherwise). Rendering pulls the destination in 128-frame quanta
  and each node renders once per quantum; started sources nothing pulls
  still advance and end.
- `AudioParam`: `value` (setting it is `setValueAtTime(value,
  currentTime)`), `setValueAtTime`, `linearRampToValueAtTime`,
  `exponentialRampToValueAtTime`, `setTargetAtTime`, `setValueCurveAtTime`,
  `cancelScheduledValues`, `cancelAndHoldAtTime` with the spec's formulas;
  a-rate (per sample) or k-rate, nominal range clamping, audio-rate inputs
  from connected nodes, and Chrome's errors (RangeError, NotSupportedError
  for overlapping curves, InvalidStateError).
- `AudioBufferSourceNode`: `buffer` (acquired at `start()`), `start(when,
  offset, duration)`, `stop()`, `loop`/`loopStart`/`loopEnd`,
  `playbackRate` and `detune` (k-rate), linear interpolation between
  samples, sub-sample start times, `ended`. `OscillatorNode`: `sine`,
  `square`, `sawtooth`, `triangle` (ideal waveforms starting at 0; Chrome
  uses band-limited tables, so the last three differ near their edges),
  `frequency`, `detune`. `ConstantSourceNode`: `offset`. `GainNode`:
  `gain`.
- `OfflineAudioContext(channels, length, rate)` or `({ numberOfChannels,
  length, sampleRate })`: `startRendering()` renders in a task (state
  `running`, then `closed`), resolves with the `AudioBuffer` and fires
  `complete`; `suspend(time)` (quantum-aligned) and `resume()`; sources
  fire `ended` after the render.
- `AudioContext` has no output device. It starts `suspended` and runs (in
  a task) when made after user activation, as Chrome's autoplay policy
  allows; `resume()` without activation stays pending. While running,
  `currentTime` follows the document clock in whole quanta (virtual time
  included) and sources fire `ended` when their buffer, duration or stop
  time runs out, on a timer that does not keep the page busy; no sample is
  computed. `suspend()`, `close()`, `baseLatency`, `outputLatency`,
  `getOutputTimestamp()`. `sampleRate` is 48000 unless given.
- `decodeAudioData(buffer, success?, error?)` detaches the buffer, decodes
  it with lp-media on a thread made for the call (MP3, AAC/M4A, Ogg
  Vorbis/Opus, WebM Opus, FLAC, WAV PCM/float, AIFF, CAF, ALAC), resamples
  every channel to the context's rate (windowed sinc, `floor(frames * to /
  from)` frames as Chrome) and resolves in a task with the `AudioBuffer`;
  undecodable data rejects with EncodingError, a detached buffer with
  DataCloneError. MP3 encoder delay and padding (LAME tag) and the end of
  Ogg streams are trimmed; the Opus pre-skip is dropped.

## Gaps

- No VP8, VP9 or HEVC frames (see "Choosing the codecs"). Fragmented MP4
  (`moof`) has no frames; in a body longer than 16 MB, frames past the
  prefix are not decoded (the last one held shows), and an MP4 whose
  `moov` follows the prefix has none.
- No audio output: media elements and `AudioContext` compute no samples;
  `MediaElementAudioSourceNode`, `MediaStreamAudioDestinationNode` and
  `captureStream` are missing.
- Web Audio has only the nodes above: no `AnalyserNode`,
  `BiquadFilterNode`, `DelayNode`, `DynamicsCompressorNode`,
  `ConvolverNode`, `PannerNode`/`AudioListener`, `StereoPannerNode`,
  `WaveShaperNode`, `ChannelSplitterNode`/`ChannelMergerNode`,
  `PeriodicWave`, `IIRFilterNode` or `AudioWorklet`. Audio fingerprinting
  scripts that use a compressor fail where Chrome succeeds.
- `decodeAudioData` keeps AAC priming and padding samples and WebM Opus end
  padding: those buffers are up to a few thousand frames longer than
  Chrome's. Opus with more than two channels does not decode.
- No progressive `progress` events or buffered ranges: the metadata is read
  once from the prefix. Durations of long Ogg, ADTS and CBR MP3 resources
  are estimated from their length rather than read from the end.
  Fragmented MP4 without `mehd` reports the longest track (or `Infinity`).
- `MediaSource`/`MediaStream` sources are not playable (MSE
  `isTypeSupported()` is still false).
- No `addTextTrack`, `audioTracks`/`videoTracks`, `setSinkId`, media
  fragments (`#t=`), Media Session.
- Unmuting playing autoplayed media without activation does not pause it
  as Chrome does; activation does not propagate to ancestor frames.
- Changing `preload` from `none` does not resume a suspended load; only
  `play()` and a new load do.
- Media in documents without a browsing context (`DOMParser`,
  `createHTMLDocument`) does not load by itself.
