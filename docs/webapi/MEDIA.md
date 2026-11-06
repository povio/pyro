# Media elements

`crates/lp-webapi/src/media.rs` runs `HTMLMediaElement` (`<audio>`,
`<video>`, `new Audio()`) like a browser that can decode nothing: the IDL
state works, and every resource fails as unsupported. Nothing is fetched.
`TimeRanges`, `MediaError` and `TextTrackList` are made in JS
(`crates/lp-webapi/src/media.js`) on a realm's first use.

## Loading

The DOM (`crates/lp-dom/src/media_element.rs`) queues what starts a load for
a document with a browsing context: `src` set or changed (the media element
load algorithm), a media element inserted or created by the parser (resource
selection, when its `networkState` is `NETWORK_EMPTY` and it has a source),
and a `<source>` child inserted. The queue is drained with the iframe
post-connection steps, after the native call that changed the tree and at
the event loop's checkpoints. Documents that never had a media element pay
one flag test per attribute change and insertion.

- `src` (or `srcObject`): `networkState` becomes `NETWORK_NO_SOURCE`, then
  `NETWORK_LOADING` and `loadstart` fires, then `error` with `error` a
  `MediaError` of code 4 (`MEDIA_ERR_SRC_NOT_SUPPORTED`) and
  `networkState` `NETWORK_NO_SOURCE`. `currentSrc` is the resolved URL.
- `<source>` children: `loadstart` at the media element, then `error` at each
  `<source>` in order; the element keeps `error` null and waits for another
  `<source>`, which fails the same way when it is added.
- A new load (new `src`, `load()`) fires `abort` (while loading) and
  `emptied`, pauses the element (rejecting pending `play()` promises with
  AbortError) and discards the earlier load's queued events.

## Playback

- `play()`: rejected at once with NotSupportedError ("The element has no
  supported sources.") after a source failure. Otherwise the element
  unpauses, `play` and `waiting` fire (readyState stays `HAVE_NOTHING`), and
  the promise is rejected with NotSupportedError when the source fails. With
  no source at all the promise stays pending, as in browsers.
- `pause()`: pauses, fires `timeupdate` and `pause`, and rejects pending
  `play()` promises with AbortError.
- `currentTime` (and `fastSeek`) sets the default playback start position:
  it reads back what was set; a new load resets it to 0.
- `volume` (IndexSizeError outside [0, 1]), `muted` (the IDL state; it
  starts from the `muted` attribute, which `defaultMuted` reflects),
  `playbackRate` (NotSupportedError outside Chrome's range 0.0625 to 16,
  except 0), `defaultPlaybackRate` and `preservesPitch` keep their values;
  `volumechange` and `ratechange` fire when they change. Non-finite values
  throw TypeError.
- `readyState` is 0, `duration` NaN, `ended` and `seeking` false;
  `buffered`, `played` and `seekable` are empty `TimeRanges` (new each read;
  `start`/`end` throw IndexSizeError); `textTracks` is an empty
  `TextTrackList`. `canPlayType()` returns `""` for every type.
- The `NETWORK_*` and `HAVE_*` constants are on the interface and its
  prototype; `MediaError` has the `MEDIA_ERR_*` constants.

## Gaps

- No decoding or fetching: no `loadedmetadata`, `canplay`, `playing`,
  `timeupdate` during playback or `ended`; `videoWidth`/`videoHeight` are 0.
- `autoplay` never plays; removing an element does not pause it.
- No `addTextTrack`, `audioTracks`/`videoTracks`, `captureStream`,
  `setSinkId`, `requestVideoFrameCallback`, `getVideoPlaybackQuality`.
- Media in documents without a browsing context (`DOMParser`,
  `createHTMLDocument`) does not load.
