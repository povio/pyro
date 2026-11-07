# Media elements

`crates/lp-webapi/src/media.rs` runs `HTMLMediaElement` (`<audio>`,
`<video>`, `new Audio()`) like Chrome without its decoders: the resource is
fetched, its container headers are read in pure Rust
(`crates/lp-webapi/src/media_format.rs`), and the element walks the ready
states and plays on a clock, but no sample is ever decoded. `TimeRanges`,
`MediaError` and `TextTrackList` are made in JS
(`crates/lp-webapi/src/media.js`) on a realm's first use.

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
- `canPlayType()` answers as desktop Chrome: `maybe` for a supported
  container without codecs (`probably` for `audio/mpeg`, `audio/mp3`,
  `audio/flac`, `audio/aac`), `probably` when every codec is supported
  (H.264, HEVC, VP8, VP9, AV1, AAC, MP3, Opus, Vorbis, FLAC, PCM in the
  containers Chrome allows them in), `""` for anything else (Theora,
  MPEG-4 Part 2, unknown types or codecs, a video codec in an `audio/` type).

## Autoplay

Chrome's default policy: muted media may play; unmuted media needs sticky
user activation of its document (`PageDom::notify_user_activation`), set by
trusted keydown (not Escape), mousedown/mouseup and touchend input, and by
CDP `Runtime.evaluate`/`callFunctionOn` with `userGesture: true` (which
Playwright sends with every `page.evaluate`). Without it, `play()` rejects
with NotAllowedError and the `autoplay` attribute leaves the element paused.

## Rendering

Video frames are never decoded. A `<video>` paints its `poster` image
(Blitz loads it like an `<img>` source, fitted by `object-fit`, `contain` by
default) in screenshots; without a poster it paints nothing. Its intrinsic
size is the video track's size once known, else the poster's, else
300x150.

## Gaps

- No decoding: no frames, no audio output, `requestVideoFrameCallback`,
  `getVideoPlaybackQuality`, `captureStream` and Web Audio sources are
  missing. A playing `<video>` still shows its poster.
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
