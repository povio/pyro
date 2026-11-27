// Media Session for one realm (https://w3c.github.io/mediasession/): MediaSession, MediaMetadata and
// ChapterInformation. A headless browser has no media keys, notification or lock screen to show or
// to raise actions from, as in headless Chrome: the session is its observable state (metadata,
// playbackState, position state, action handlers, camera and microphone flags), validated as the spec
// and Chrome do; no action handler is ever invoked by the user agent itself.
(function () {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const doc = document;
  const URLC = URL;
  const TypeErrorC = TypeError;
  const ArrayC = Array;
  const PromiseC = Promise;
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const invocation = (what, cls) => new TypeErrorC(`Failed to execute '${what}' on '${cls}': Illegal invocation`);
  const isObj = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  const idl = (cls, tag) => {
    for (const k of O.getOwnPropertyNames(cls.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(cls.prototype, k);
      d.enumerable = true;
      O.defineProperty(cls.prototype, k, d);
    }
    O.defineProperty(cls.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  };
  // A dictionary argument: undefined and null are the empty dictionary, other primitives a TypeError.
  const dict = (v, what) => {
    if (v === undefined || v === null) return {};
    if (!isObj(v)) throw new TypeErrorC(`${what}: The provided value is not of type 'dictionary'.`);
    return v;
  };
  // USVString: lone surrogates become U+FFFD.
  const usv = (v) => String(v).toWellFormed();

  // MediaImage: {src (resolved against the entry settings object's base URL), sizes, type}, frozen.
  const mediaImage = (v, what) => {
    if (!isObj(v)) throw new TypeErrorC(`${what}: The provided value is not of type 'MediaImage'.`);
    if (v.src === undefined) throw new TypeErrorC(`${what}: Failed to read the 'src' property from 'MediaImage': Required member is undefined.`);
    const src = usv(v.src);
    const sizes = v.sizes === undefined ? '' : String(v.sizes);
    const type = v.type === undefined ? '' : String(v.type);
    let href;
    try {
      href = new URLC(src, doc.baseURI).href;
    } catch (e) {
      throw new TypeErrorC(`${what}: '${src}' can't be resolved to a valid URL.`);
    }
    return O.freeze({ src: href, sizes, type });
  };
  const sequence = (v, what) => {
    if (!isObj(v) || typeof v[Symbol.iterator] !== 'function') throw new TypeErrorC(`${what}: The provided value cannot be converted to a sequence.`);
    return ArrayC.from(v);
  };
  const artworkList = (v, what) => O.freeze(sequence(v, what).map((i) => mediaImage(i, what)));

  const chapters = new WeakMap();
  class ChapterInformation {
    constructor(t, title, startTime, artwork) {
      if (t !== token) throw illegal('ChapterInformation');
      chapters.set(this, { title, startTime, artwork });
      O.freeze(this);
    }
    get title() {
      return chapterOf(this, 'title').title;
    }
    get startTime() {
      return chapterOf(this, 'startTime').startTime;
    }
    get artwork() {
      return chapterOf(this, 'artwork').artwork;
    }
  }
  const chapterOf = (o, what) => {
    const c = chapters.get(o);
    if (!c) throw invocation(what, 'ChapterInformation');
    return c;
  };
  idl(ChapterInformation, 'ChapterInformation');
  const chapterInfoList = (v, what) =>
    O.freeze(
      sequence(v, what).map((c) => {
        const d = dict(c, what);
        const startTime = d.startTime === undefined ? 0 : +d.startTime;
        if (!(startTime - startTime === 0)) throw new TypeErrorC(`${what}: The provided double value is non-finite.`);
        return new ChapterInformation(token, d.title === undefined ? '' : String(d.title), startTime, artworkList(d.artwork === undefined ? [] : d.artwork, what));
      }),
    );

  const metadata = new WeakMap();
  const metadataOf = (o, what) => {
    const m = metadata.get(o);
    if (!m) throw invocation(what, 'MediaMetadata');
    return m;
  };
  class MediaMetadata {
    constructor(init = {}) {
      const what = "Failed to construct 'MediaMetadata'";
      const d = dict(init, what);
      const state = {
        title: d.title === undefined ? '' : String(d.title),
        artist: d.artist === undefined ? '' : String(d.artist),
        album: d.album === undefined ? '' : String(d.album),
        artwork: artworkList(d.artwork === undefined ? [] : d.artwork, what),
        chapterInfo: chapterInfoList(d.chapterInfo === undefined ? [] : d.chapterInfo, what),
      };
      metadata.set(this, state);
    }
    get title() { return metadataOf(this, 'title').title; }
    set title(v) { metadataOf(this, 'title').title = String(v); }
    get artist() { return metadataOf(this, 'artist').artist; }
    set artist(v) { metadataOf(this, 'artist').artist = String(v); }
    get album() { return metadataOf(this, 'album').album; }
    set album(v) { metadataOf(this, 'album').album = String(v); }
    get artwork() { return metadataOf(this, 'artwork').artwork; }
    set artwork(v) {
      const m = metadataOf(this, 'artwork');
      m.artwork = artworkList(v, "Failed to set the 'artwork' property on 'MediaMetadata'");
    }
    get chapterInfo() { return metadataOf(this, 'chapterInfo').chapterInfo; }
  }
  idl(MediaMetadata, 'MediaMetadata');

  const ACTIONS = [
    'play', 'pause', 'seekbackward', 'seekforward', 'previoustrack', 'nexttrack', 'skipad', 'stop', 'seekto',
    'togglemicrophone', 'togglecamera', 'togglescreenshare', 'hangup', 'previousslide', 'nextslide',
    'enterpictureinpicture', 'voiceactivity',
  ];
  const STATES = ['none', 'paused', 'playing'];
  const sessions = new WeakMap();
  const sessionOf = (o, what) => {
    const s = sessions.get(o);
    if (!s) throw invocation(what, 'MediaSession');
    return s;
  };
  const flag = (what, key) => function (active) {
    try {
      const s = sessionOf(this, what);
      if (arguments.length < 1) throw new TypeErrorC(`Failed to execute '${what}' on 'MediaSession': 1 argument required, but only 0 present.`);
      s[key] = !!active;
      return PromiseC.resolve();
    } catch (e) {
      return PromiseC.reject(e);
    }
  };
  class MediaSession {
    constructor(t) {
      if (t !== token) throw illegal('MediaSession');
      sessions.set(this, {
        metadata: null, playbackState: 'none', handlers: new Map(), position: null,
        camera: false, microphone: false, screenshare: false,
      });
    }
    get metadata() { return sessionOf(this, 'metadata').metadata; }
    set metadata(v) {
      const s = sessionOf(this, 'metadata');
      if (v !== null && v !== undefined && !metadata.has(v)) {
        throw new TypeErrorC("Failed to set the 'metadata' property on 'MediaSession': The provided value is not of type 'MediaMetadata'.");
      }
      s.metadata = v === undefined ? null : v;
    }
    get playbackState() { return sessionOf(this, 'playbackState').playbackState; }
    set playbackState(v) {
      const s = sessionOf(this, 'playbackState');
      // An invalid enum value is ignored by the attribute setter.
      const value = String(v);
      if (STATES.includes(value)) s.playbackState = value;
    }
    setActionHandler(action, handler) {
      const s = sessionOf(this, 'setActionHandler');
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'setActionHandler' on 'MediaSession': 2 arguments required, but only ${arguments.length} present.`);
      const name = String(action);
      if (!ACTIONS.includes(name)) {
        throw new TypeErrorC(`Failed to execute 'setActionHandler' on 'MediaSession': The provided value '${name}' is not a valid enum value of type MediaSessionAction.`);
      }
      if (handler !== null && handler !== undefined && typeof handler !== 'function') {
        throw new TypeErrorC("Failed to execute 'setActionHandler' on 'MediaSession': The callback provided as parameter 2 is not a function.");
      }
      if (typeof handler === 'function') s.handlers.set(name, handler);
      else s.handlers.delete(name);
    }
    setPositionState(state = {}) {
      const s = sessionOf(this, 'setPositionState');
      const what = "Failed to execute 'setPositionState' on 'MediaSession'";
      const d = dict(state, what);
      // An empty dictionary clears the position state.
      if (d.duration === undefined && d.position === undefined && d.playbackRate === undefined) {
        s.position = null;
        return;
      }
      if (d.duration === undefined) throw new TypeErrorC(`${what}: The duration must be provided.`);
      const duration = +d.duration;
      const position = d.position === undefined ? 0 : +d.position;
      const rate = d.playbackRate === undefined ? 1 : +d.playbackRate;
      for (const [name, v] of [['position', position], ['playbackRate', rate]]) {
        if (!(v - v === 0)) throw new TypeErrorC(`${what}: The provided double value for '${name}' is non-finite.`);
      }
      if (duration !== duration || duration < 0) throw new TypeErrorC(`${what}: The provided duration cannot be negative.`);
      if (position < 0) throw new TypeErrorC(`${what}: The provided position cannot be negative.`);
      if (position > duration) throw new TypeErrorC(`${what}: The provided position cannot be greater than the duration.`);
      if (rate === 0) throw new TypeErrorC(`${what}: The provided playbackRate cannot be equal to zero.`);
      s.position = { duration, position, playbackRate: rate };
    }
    setMicrophoneActive(active) { return flag('setMicrophoneActive', 'microphone').apply(this, arguments); }
    setCameraActive(active) { return flag('setCameraActive', 'camera').apply(this, arguments); }
    setScreenshareActive(active) { return flag('setScreenshareActive', 'screenshare').apply(this, arguments); }
  }
  idl(MediaSession, 'MediaSession');

  return { MediaSession, MediaMetadata, ChapterInformation, mediaSession: new MediaSession(token) };
});
