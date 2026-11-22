// TimeRanges, MediaError and VideoPlaybackQuality for one
// realm, made on first use (media.rs). TimeRanges hold what media.rs
// computed.
(function () {
  'use strict';
  const secret = Symbol('pyro');
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  function illegal() { return new TypeErrorC('Illegal constructor'); }
  function webidl(C, tag) {
    for (const name of getOwnPropertyNames(C.prototype)) {
      if (name === 'constructor') continue;
      const desc = getOwnPropertyDescriptor(C.prototype, name);
      desc.enumerable = true;
      defineProperty(C.prototype, name, desc);
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  const ranges = new WeakMap();
  function rangesOf(self) {
    const r = ranges.get(self);
    if (!r) throw new TypeErrorC('Illegal invocation');
    return r;
  }
  function index(r, i, what) {
    const n = +i >>> 0;
    if (n >= r.length) {
      throw new DOMExceptionC(`Failed to execute '${what}' on 'TimeRanges': The index provided (${n}) is greater than or equal to the maximum bound (${r.length}).`, 'IndexSizeError');
    }
    return n;
  }
  class TimeRanges {
    constructor(token, list) {
      if (token !== secret) throw illegal();
      ranges.set(this, list);
    }
    get length() { return rangesOf(this).length; }
    start(i) { const r = rangesOf(this); return r[index(r, i, 'start')][0]; }
    end(i) { const r = rangesOf(this); return r[index(r, i, 'end')][1]; }
  }
  webidl(TimeRanges, 'TimeRanges');

  const errors = new WeakMap();
  function errorOf(self) {
    const e = errors.get(self);
    if (!e) throw new TypeErrorC('Illegal invocation');
    return e;
  }
  class MediaError {
    constructor(token, code, message) {
      if (token !== secret) throw illegal();
      errors.set(this, { code, message });
    }
    get code() { return errorOf(this).code; }
    get message() { return errorOf(this).message; }
  }
  webidl(MediaError, 'MediaError');
  const CODES = { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 };
  for (const name of Object.keys(CODES)) {
    for (const target of [MediaError, MediaError.prototype]) {
      defineProperty(target, name, { value: CODES[name], enumerable: true });
    }
  }

  // HTMLVideoElement.getVideoPlaybackQuality(): nothing is ever dropped.
  const qualities = new WeakMap();
  function qualityOf(self) {
    const q = qualities.get(self);
    if (!q) throw new TypeErrorC('Illegal invocation');
    return q;
  }
  class VideoPlaybackQuality {
    constructor(token, creationTime, total) {
      if (token !== secret) throw illegal();
      qualities.set(this, { creationTime, total });
    }
    get creationTime() { return qualityOf(this).creationTime; }
    get droppedVideoFrames() { qualityOf(this); return 0; }
    get totalVideoFrames() { return qualityOf(this).total; }
    get corruptedVideoFrames() { qualityOf(this); return 0; }
  }
  webidl(VideoPlaybackQuality, 'VideoPlaybackQuality');

  return {
    TimeRanges, MediaError, VideoPlaybackQuality,
    playbackQuality(creationTime, total) { return new VideoPlaybackQuality(secret, creationTime, total); },
    timeRanges(flat) {
      const list = [];
      for (let i = 0; i + 1 < flat.length; i += 2) list.push([flat[i], flat[i + 1]]);
      return new TimeRanges(secret, list);
    },
    mediaError(code, message) { return new MediaError(secret, code, message); },
  };
})
