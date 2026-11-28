// MediaStream Recording for one realm: MediaRecorder and BlobEvent
// (https://w3c.github.io/mediacapture-record/). What is recorded is what this build can really
// encode: H.264 (OpenH264) and Opus in fragmented MP4, so `video/mp4` and `audio/mp4` with `avc1`/`avc3`
// and `opus` codecs are supported and everything else (WebM, Matroska, VP8, VP9, AV1, HEVC, AAC) is not:
// isTypeSupported() says false and the constructor throws NotSupportedError, as a browser without those
// encoders does.
//
// The tracks' media comes through the source protocol of media_stream.js (`source.pullFrame(settings)`,
// `source.pullAudio(count)`), polled by ONE timer that exists only while a recorder is `recording` (a
// paused or stopped recorder holds no timer). A tick pulls the audio the media clock is owed, a
// picture when one is due, and flushes a chunk when the timeslice is up. Pictures are scaled into the
// recording size on an OffscreenCanvas and handed with the samples to the native session
// (media_recorder.rs), whose flushes are the chunks of the file.
//
// `native`: create(options) -> session function, canVideo(), canAudio(), active(), dispatchTrusted(target,
// event), stream (the realm's media_stream.js module).
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const EventC = Event;
  const EventTargetC = EventTarget;
  const ErrorEventC = ErrorEvent;
  const BlobC = Blob;
  const MathC = Math;
  const NumberC = Number;
  const setTimeoutC = setTimeout;
  const clearTimeoutC = clearTimeout;
  const perfNow = () => performance.now();
  const queue = (f) => {
    setTimeoutC(f, 0);
  };
  const stream = native.stream;
  const MediaStreamC = stream.MediaStream;
  const getTracksOf = MediaStreamC.prototype.getTracks;
  const trackSource = stream.trackSource;
  const OffscreenCanvasC = OffscreenCanvas;

  const invocation = (what) => new TypeErrorC(`Failed to execute '${what}' on 'MediaRecorder': Illegal invocation`);
  const domError = (name, message) => new DOMExceptionC(message, name);
  const isObj = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  // (A detached document's realm may not even answer.)
  const isActive = () => {
    try {
      return native.active();
    } catch (e) {
      return false;
    }
  };

  // IDL members are enumerable.
  const idl = (cls, tag) => {
    for (const k of O.getOwnPropertyNames(cls.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(cls.prototype, k);
      d.enumerable = true;
      O.defineProperty(cls.prototype, k, d);
    }
    O.defineProperty(cls.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  };
  const handlerState = new WeakMap();
  const handler = (cls, type, check) => {
    const name = 'on' + type;
    const accessors = {
      get() {
        check(this);
        const h = handlerState.get(this);
        return (h && h[type] && h[type].cb) || null;
      },
      set(f) {
        check(this);
        let all = handlerState.get(this);
        if (!all) handlerState.set(this, (all = {}));
        const rec = all[type] || (all[type] = { cb: null, added: false });
        rec.cb = typeof f === 'function' || (f !== null && typeof f === 'object') ? f : null;
        if (rec.cb && !rec.added) {
          rec.added = true;
          const self = this;
          this.addEventListener(type, function (e) {
            const cb = rec.cb;
            if (typeof cb === 'function') return cb.call(self, e);
            return undefined;
          });
        }
      },
    };
    O.defineProperty(accessors.get, 'name', { value: 'get ' + name });
    O.defineProperty(accessors.set, 'name', { value: 'set ' + name });
    O.defineProperty(cls.prototype, name, { configurable: true, enumerable: true, get: accessors.get, set: accessors.set });
  };

  // ---- Types ----------------------------------------------------------------------------------
  // `type/subtype` with at most a `codecs` parameter, as { audioOnly, video, audio } or null. A codec the
  // build cannot encode, a second codec of a kind, or a video codec in an audio container: null.
  const AVC = /^(avc[13])(?:\.([0-9a-f]{6}))?$/i;
  const parseType = (text) => {
    const m = /^\s*(audio|video)\/mp4\s*(?:;\s*codecs\s*=\s*(?:"([^"]*)"|'([^']*)'|([^;]*?))\s*)?$/i.exec(text);
    if (!m) return null;
    const audioOnly = m[1].toLowerCase() === 'audio';
    const list = m[2] !== undefined ? m[2] : m[3] !== undefined ? m[3] : m[4] !== undefined ? m[4] : '';
    const out = { audioOnly, video: null, audio: false };
    for (const raw of list.split(',')) {
      const codec = raw.trim();
      if (codec === '') continue;
      if (codec.toLowerCase() === 'opus') {
        if (out.audio) return null;
        out.audio = true;
        continue;
      }
      const avc = AVC.exec(codec);
      if (!avc || out.video || audioOnly) return null;
      // A bare `avc1` is the default profile; the level is ours to choose (the encoder takes the lowest
      // one that fits), so only the profile and constraint flags decide.
      out.video = { entry: avc[1].toLowerCase(), profile: avc[2] ? avc[2].slice(0, 4).toLowerCase() : '42e0' };
    }
    return out;
  };
  const typeSupported = (text) => {
    if (text === '') return true;
    const t = parseType(text);
    if (!t) return false;
    if (t.video && !native.canVideo()) return false;
    if (t.video) {
      // The profile and constraint flags must be ones the encoder writes (`create` throws otherwise).
      if (!/^(42|4d|64)/.test(t.video.profile) || !profileOk(t.video.profile)) return false;
    }
    if (!t.audioOnly && !native.canVideo()) return false;
    if ((t.audio || t.audioOnly) && !native.canAudio()) return false;
    return true;
  };
  // Profiles OpenH264 can write: Constrained Baseline/Baseline (42 with flags 00,40,80,c0,e0),
  // Main (4d, flags within 0x4c), High (64, flags within 0x0c).
  const profileOk = (p) => {
    const id = parseInt(p.slice(0, 2), 16);
    const flags = parseInt(p.slice(2), 16);
    if (id === 0x42) return [0, 0x40, 0x80, 0xc0, 0xe0].includes(flags);
    if (id === 0x4d) return (flags & ~0x4c) === 0;
    if (id === 0x64) return (flags & ~0x0c) === 0;
    return false;
  };
  // What a recorder with no (or an incomplete) mimeType records and says it records.
  // (Lower case, because a Blob's type is: `blob.type === recorder.mimeType` must hold.)
  const defaultType = (videoCodec, hasAudio) => {
    const codecs = [];
    if (videoCodec) codecs.push(videoCodec);
    if (hasAudio) codecs.push('opus');
    return (videoCodec ? 'video' : 'audio') + '/mp4;codecs=' + codecs.join(',');
  };

  // ---- Options and bitrates -----------------------------------------------------------------------
  const ulong = (v) => {
    const n = NumberC(v);
    if (n !== n || n === Infinity || n === -Infinity) return 0;
    const t = MathC.trunc(n);
    return ((t % 4294967296) + 4294967296) % 4294967296;
  };
  const DEFAULT_AUDIO_BPS = 128000;
  const DEFAULT_VIDEO_BPS = 2500000;
  // A total bitrate shared by the tracks: audio takes a tenth up to its default, video the rest, each at
  // least what an encoder can run at.
  const share = (total) => {
    const audio = MathC.min(DEFAULT_AUDIO_BPS, MathC.max(6000, MathC.floor(total / 10)));
    return { audio, video: MathC.max(1000, total - audio) };
  };

  // ---- BlobEvent --------------------------------------------------------------------------------
  const blobEvents = new WeakMap();
  class BlobEvent extends EventC {
    constructor(type, eventInitDict) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'BlobEvent': 2 arguments required, but only ${arguments.length} present.`);
      if (eventInitDict !== undefined && eventInitDict !== null && !isObj(eventInitDict)) {
        throw new TypeErrorC("Failed to construct 'BlobEvent': The provided value is not of type 'BlobEventInit'.");
      }
      super(type, eventInitDict);
      const init = eventInitDict || {};
      const data = init.data;
      if (!(data instanceof BlobC)) {
        throw new TypeErrorC("Failed to construct 'BlobEvent': Failed to read the 'data' property from 'BlobEventInit': Required member is undefined or is not of type 'Blob'.");
      }
      let timecode = NaN;
      if (init.timecode !== undefined) {
        timecode = NumberC(init.timecode);
        if (timecode !== timecode || timecode === Infinity || timecode === -Infinity) {
          throw new TypeErrorC("Failed to construct 'BlobEvent': Failed to read the 'timecode' property from 'BlobEventInit': The provided double value is non-finite.");
        }
      }
      blobEvents.set(this, { data, timecode });
    }
    get data() {
      const s = blobEvents.get(this);
      if (!s) throw new TypeErrorC("Failed to read the 'data' property from 'BlobEvent': Illegal invocation");
      return s.data;
    }
    get timecode() {
      const s = blobEvents.get(this);
      if (!s) throw new TypeErrorC("Failed to read the 'timecode' property from 'BlobEvent': Illegal invocation");
      return s.timecode;
    }
  }
  idl(BlobEvent, 'BlobEvent');

  // ---- MediaRecorder -----------------------------------------------------------------------------
  const recorders = new WeakMap(); // MediaRecorder -> state
  const recOf = (r, what) => {
    const st = recorders.get(r);
    if (!st) throw invocation(what);
    return st;
  };
  const recCheck = (r) => {
    if (!recorders.has(r)) throw new TypeErrorC('Illegal invocation');
  };
  // Events are tasks of a document that must be fully active: a document removed by an earlier listener
  // (`dataavailable` removing the iframe) gets no `stop` after it.
  const fire = (st, event) => {
    if (isActive()) native.dispatchTrusted(st.object, event);
  };
  // `parts`: the pieces of file of the chunk (null: an empty Blob). The timecode is the time of the chunk's
  // first sample on the recording clock, and 0 for the first chunk (the initialization segment).
  const fireBlob = (st, parts, mime, firstUs) => {
    let timecode = st.lastTimecode;
    if (parts) {
      timecode = st.sent ? MathC.max(st.lastTimecode, firstUs / 1000) : 0;
      st.sent = true;
    }
    st.lastTimecode = timecode;
    const data = new BlobC(parts || [], { type: mime });
    fire(st, new BlobEvent('dataavailable', { data, timecode }));
  };

  // Takes the session's pending chunk: [pieces, firstUs] or null when nothing is ready. The initialization
  // segment goes out alone first (the first fragment waits for the next chunk), so that a Blob timeline
  // starts with a small header chunk, as a browser's recorder gives.
  const takeChunk = (s, last) => {
    const pieces = s.held.concat(Array.from(s.handle('flush', last)));
    s.held = [];
    if (!pieces.length) return null;
    if (!s.headerSent && !last && pieces.length > 1) {
      s.headerSent = true;
      s.held = pieces.slice(1);
      return [pieces.slice(0, 1), 0];
    }
    s.headerSent = true;
    const firstUs = s.chunkFirstUs;
    s.chunkFirstUs = null;
    return [pieces, firstUs === null ? 0 : firstUs];
  };

  // Ends a session (stop(), all tracks ended, a track-set change or an error): the recorder is inactive
  // now; once the pictures already being fetched are encoded, the rest of the data is delivered in a task
  // (`error` first when there is one), then `stop`.
  const finish = (st, s, error) => {
    if (st.session === s) {
      st.session = null;
      st.state = 'inactive';
    }
    if (s.timer) {
      clearTimeoutC(s.timer);
      s.timer = 0;
    }
    const deliver = () => {
      queue(() => {
        if (s.closed) return;
        s.closed = true;
        let chunk = null;
        try {
          chunk = takeChunk(s, true);
        } catch (e) {
          chunk = null;
        }
        s.handle('close');
        if (error) fire(st, new ErrorEventC('error', { error: domError(error, 'The recording was stopped because the stream changed.'), message: error }));
        fireBlob(st, chunk && chunk[0], s.mime, chunk ? chunk[1] : 0);
        fire(st, new EventC('stop'));
      });
    };
    if (s.inflight > 0) s.deliver = deliver;
    else deliver();
  };
  // The page's document went away: nothing more can be delivered.
  const abandon = (st, s) => {
    if (st.session === s) {
      st.session = null;
      st.state = 'inactive';
    }
    if (s.timer) clearTimeoutC(s.timer);
    s.timer = 0;
    s.closed = true;
    try {
      s.handle('close');
    } catch (e) {
      /* closed already */
    }
  };

  const sameTracks = (a, b) => {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  };

  const encodePicture = (s, bitmap, at, disabled) => {
    const ctx = s.ctx;
    ctx.clearRect(0, 0, s.width, s.height);
    if (disabled) {
      ctx.fillStyle = '#000';
      ctx.fillRect(0, 0, s.width, s.height);
    } else {
      ctx.drawImage(bitmap, 0, 0, s.width, s.height);
    }
    const rgba = ctx.getImageData(0, 0, s.width, s.height).data;
    if (s.chunkFirstUs === null) s.chunkFirstUs = at;
    s.handle('video', rgba, at, false);
  };

  // Nothing is being fetched any more: the `start` event waiting for the first picture goes out (its
  // mimeType is the stream's real one by now), then a finished recording's data.
  const settled = (s) => {
    const announce = s.announce;
    s.announce = null;
    if (announce) announce();
    const waiting = s.waiters;
    s.waiters = [];
    for (const w of waiting) w();
    const d = s.deliver;
    s.deliver = null;
    if (d) d();
  };

  const pullVideo = (st, s, clockUs) => {
    const v = s.videoTrack;
    if (!v || s.videoBusy || v.readyState !== 'live') return;
    const source = trackSource(v);
    let p = null;
    try {
      p = source.pullFrame ? source.pullFrame(v.getSettings()) : null;
    } catch (e) {
      p = null;
    }
    if (!p) return;
    s.videoBusy = true;
    s.inflight++;
    const done = () => {
      s.videoBusy = false;
      s.inflight--;
      if (s.inflight === 0) settled(s);
    };
    p.then(
      (bitmap) => {
        try {
          if (!s.closed) encodePicture(s, bitmap, clockUs, !v.enabled);
        } catch (e) {
          /* a picture that cannot be encoded is dropped */
        }
        try {
          if (bitmap && bitmap.close) bitmap.close();
        } catch (e) {
          /* closed */
        }
        done();
      },
      done,
    );
  };

  const pullAudio = (st, s, clockUs) => {
    const a = s.audioTrack;
    if (!a || a.readyState !== 'live') return;
    const source = trackSource(a);
    const rate = s.audioRate;
    // The samples the media clock owes, at most a second of them at a time; the first pull is 40 ms so
    // that the first packets exist when `start` is fired (a requestData() then has data).
    // The audio timeline starts when its first samples are taken (not at the recorder's clock zero).
    if (s.audioFrames === 0) s.audioBase = MathC.round(clockUs);
    const owed = MathC.max(MathC.round(((clockUs - s.audioBase) * rate) / 1000000) - s.audioFrames, s.audioFrames === 0 ? MathC.ceil(rate / 25) : 0);
    if (owed < 1) return;
    const count = MathC.min(owed, rate);
    let blocks = null;
    try {
      blocks = source.pullAudio ? source.pullAudio(count) : null;
    } catch (e) {
      blocks = null;
    }
    if (!blocks || !blocks.length) return;
    const n = s.audioChannels;
    const out = new Float32Array(count * n);
    const enabled = a.enabled;
    if (enabled) {
      for (let c = 0; c < n; c++) {
        const block = blocks[c < blocks.length ? c : blocks.length - 1];
        for (let i = 0; i < count; i++) out[i * n + c] = block[i];
      }
    }
    // (Exactly the encoder's own count of the time the samples so far span: it restarts on any other value.)
    const at = s.audioBase + MathC.floor((s.audioFrames * 1000000) / rate);
    if (s.chunkFirstUs === null) s.chunkFirstUs = at;
    s.audioFrames += count;
    s.handle('audio', out, at);
  };

  const clockUs = (s) => (perfNow() - s.t0 - s.pausedTotal) * 1000;

  const tick = (st, s) => {
    if (s.timer) clearTimeoutC(s.timer);
    s.timer = 0;
    if (st.session !== s || st.state !== 'recording') return;
    if (!isActive()) return abandon(st, s);
    const tracks = getTracksOf.call(st.stream);
    if (!sameTracks(tracks, s.tracks)) return finish(st, s, 'InvalidModificationError');
    if (!tracks.some((t) => t.readyState === 'live')) return finish(st, s, null);
    const now = clockUs(s);
    try {
      pullAudio(st, s, now);
      if (s.videoTrack && now >= s.nextPicture) {
        s.nextPicture = MathC.max(s.nextPicture + s.pictureUs, now - s.pictureUs);
        pullVideo(st, s, MathC.round(now));
      }
      if (s.timeslice !== undefined && (s.timeslice === 0 || perfNow() >= s.sliceAt)) {
        const chunk = takeChunk(s, false);
        s.sliceAt = perfNow() + s.timeslice;
        if (chunk) fireBlob(st, chunk[0], s.mime, chunk[1]);
      }
    } catch (e) {
      return finish(st, s, 'EncodingError');
    }
    s.timer = setTimeoutC(() => tick(st, s), s.period);
  };

  const MIME_ERROR = 'The mimeType is not supported';

  class MediaRecorder extends EventTargetC {
    constructor(stream, options = {}) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'MediaRecorder': 1 argument required, but only 0 present.");
      if (!isActive()) throw domError('NotSupportedError', "Failed to construct 'MediaRecorder': The document is not active.");
      try {
        getTracksOf.call(stream);
      } catch (e) {
        throw new TypeErrorC("Failed to construct 'MediaRecorder': Failed to convert value to 'MediaStream'.");
      }
      if (options !== undefined && options !== null && !isObj(options)) {
        throw new TypeErrorC("Failed to construct 'MediaRecorder': The provided value is not of type 'MediaRecorderOptions'.");
      }
      const o = options || {};
      const mimeType = o.mimeType === undefined ? '' : String(o.mimeType);
      const audioBps = o.audioBitsPerSecond === undefined ? undefined : ulong(o.audioBitsPerSecond);
      const videoBps = o.videoBitsPerSecond === undefined ? undefined : ulong(o.videoBitsPerSecond);
      const totalBps = o.bitsPerSecond === undefined ? undefined : ulong(o.bitsPerSecond);
      let mode = 'variable';
      if (o.audioBitrateMode !== undefined) {
        mode = String(o.audioBitrateMode);
        if (mode !== 'constant' && mode !== 'variable') {
          throw new TypeErrorC(`Failed to construct 'MediaRecorder': The provided value '${mode}' is not a valid enum value of type BitrateMode.`);
        }
      }
      let keyMs;
      if (o.videoKeyFrameIntervalDuration !== undefined) {
        keyMs = NumberC(o.videoKeyFrameIntervalDuration);
        if (keyMs !== keyMs || keyMs === Infinity || keyMs === -Infinity) throw new TypeErrorC("Failed to construct 'MediaRecorder': The provided double value is non-finite.");
      }
      const keyCount = o.videoKeyFrameIntervalCount === undefined ? undefined : ulong(o.videoKeyFrameIntervalCount);
      if (mimeType !== '' && !typeSupported(mimeType)) throw domError('NotSupportedError', `Failed to construct 'MediaRecorder': ${MIME_ERROR}.`);
      if (keyMs !== undefined && keyCount !== undefined) {
        throw domError('NotSupportedError', "Failed to construct 'MediaRecorder': videoKeyFrameIntervalDuration and videoKeyFrameIntervalCount cannot both be set.");
      }
      super();
      // Bitrates (spec: the constructor): a total overrides the per-kind values; otherwise each kind has
      // its own or its default.
      let audioBitsPerSecond;
      let videoBitsPerSecond;
      if (totalBps !== undefined) {
        const sh = share(totalBps);
        audioBitsPerSecond = sh.audio;
        videoBitsPerSecond = sh.video;
      } else {
        audioBitsPerSecond = audioBps !== undefined ? audioBps : DEFAULT_AUDIO_BPS;
        videoBitsPerSecond = videoBps !== undefined ? videoBps : DEFAULT_VIDEO_BPS;
      }
      recorders.set(this, {
        object: this, stream, mimeType, state: 'inactive', session: null, totalBps, keyMs, keyCount, mode,
        audioBitsPerSecond, videoBitsPerSecond, parsed: mimeType === '' ? null : parseType(mimeType),
        sent: false, lastTimecode: 0,
      });
    }
    get stream() { return recOf(this, 'stream').stream; }
    get mimeType() { return recOf(this, 'mimeType').mimeType; }
    get state() { return recOf(this, 'state').state; }
    get videoBitsPerSecond() { return recOf(this, 'videoBitsPerSecond').videoBitsPerSecond; }
    get audioBitsPerSecond() { return recOf(this, 'audioBitsPerSecond').audioBitsPerSecond; }
    get audioBitrateMode() { return recOf(this, 'audioBitrateMode').mode; }

    start(timeslice = undefined) {
      const st = recOf(this, 'start');
      if (st.state !== 'inactive') throw domError('InvalidStateError', "Failed to execute 'start' on 'MediaRecorder': The MediaRecorder's state is '" + st.state + "'.");
      const slice = timeslice === undefined ? undefined : ulong(timeslice);
      if (!isActive()) throw domError('NotSupportedError', "Failed to execute 'start' on 'MediaRecorder': The document is not active.");
      const live = getTracksOf.call(st.stream).filter((t) => t.readyState === 'live');
      const videoTrack = live.find((t) => t.kind === 'video') || null;
      const audioTrack = live.find((t) => t.kind === 'audio') || null;
      if (!videoTrack && !audioTrack) throw domError('NotSupportedError', "Failed to execute 'start' on 'MediaRecorder': The stream has no live track to record.");
      const parsed = st.parsed;
      if (videoTrack && parsed && parsed.audioOnly) throw domError('NotSupportedError', "Failed to execute 'start' on 'MediaRecorder': An audio type cannot record a video track.");
      // A track whose content the page may not read (a cross-origin media element without CORS) is not recorded.
      for (const t of [videoTrack, audioTrack]) {
        if (t && trackSource(t).crossOrigin) throw domError('SecurityError', "Failed to execute 'start' on 'MediaRecorder': The stream is not same-origin.");
      }
      let audioBps = st.audioBitsPerSecond;
      let videoBps = st.videoBitsPerSecond;
      if (st.totalBps !== undefined) {
        // A total goes to the tracks there are.
        if (!videoTrack) {
          // (Opus takes up to 510 kbit/s a channel.)
          audioBps = MathC.min(st.totalBps, 510000);
          videoBps = 0;
        } else if (!audioTrack) {
          videoBps = st.totalBps;
          audioBps = 0;
        }
      }
      // The recorded size: the track's, even and inside what the encoder takes.
      let width = 0;
      let height = 0;
      let fps = 0;
      if (videoTrack) {
        const settings = videoTrack.getSettings();
        width = (settings.width | 0) || 640;
        height = (settings.height | 0) || 480;
        fps = MathC.min(30, MathC.max(1, settings.frameRate || trackSource(videoTrack).frameRate || 20));
        width = MathC.min(3840, MathC.max(16, width - (width % 2)));
        height = MathC.min(2160, MathC.max(16, height - (height % 2)));
      }
      let audioRate = 48000;
      let audioChannels = 1;
      if (audioTrack) {
        const settings = audioTrack.getSettings();
        audioRate = settings.sampleRate || 48000;
        audioChannels = MathC.min(2, MathC.max(1, settings.channelCount || 1));
      }
      let keyEvery = 0;
      if (st.keyCount !== undefined) keyEvery = st.keyCount;
      else if (st.keyMs !== undefined) keyEvery = MathC.max(1, MathC.round((st.keyMs * fps) / 1000));
      else keyEvery = MathC.round(fps * 2);
      let handle;
      try {
        handle = native.create({
          video: videoTrack
            ? {
              width, height, fps, bitrate: MathC.max(1000, videoBps), keyEvery,
              profile: (parsed && parsed.video && parsed.video.profile) || '42e0',
              entry: (parsed && parsed.video && parsed.video.entry) || 'avc1',
            }
            : null,
          audio: audioTrack ? { rate: audioRate, channels: audioChannels, bitrate: audioBps || 6000 } : null,
        });
      } catch (e) {
        throw domError('NotSupportedError', "Failed to execute 'start' on 'MediaRecorder': The stream cannot be recorded with this configuration.");
      }
      const s = {
        recorder: this, handle, mime: st.mimeType, tracks: getTracksOf.call(st.stream),
        videoTrack, audioTrack, width, height, audioRate, audioChannels, audioFrames: 0, audioBase: 0,
        pictureUs: 1000000 / (fps || 20), nextPicture: 0, videoBusy: false, inflight: 0, deliver: null,
        timer: 0, t0: perfNow(), pausedTotal: 0, pausedAt: 0, timeslice: slice, sliceAt: 0,
        chunkFirstUs: null, held: [], headerSent: false, announce: null, waiters: [], closed: false, ctx: null,
        period: audioTrack ? 20 : MathC.max(5, MathC.floor(1000 / (fps || 20))),
      };
      if (videoTrack) {
        const canvas = new OffscreenCanvasC(width, height);
        s.ctx = canvas.getContext('2d', { willReadFrequently: true });
      }
      if (slice !== undefined) s.sliceAt = perfNow() + slice;
      st.sent = false;
      st.lastTimecode = 0;
      // The bitrates the recorder now records at (what a total goes to depends on the tracks there are).
      st.audioBitsPerSecond = audioBps;
      st.videoBitsPerSecond = videoBps;
      st.session = s;
      st.state = 'recording';
      s.mime = st.mimeType === '' ? defaultType(videoTrack ? 'avc1.42c01e' : '', !!audioTrack) : st.mimeType;
      // `start` is fired in a task once the first picture and samples are encoded (as Chrome does, so that
      // a requestData() from `onstart` has data); the mimeType of a recorder without one is the real
      // stream's from then on.
      s.announce = () => {
        queue(() => {
          if (st.mimeType === '' && !s.closed) {
            let codec = '';
            try {
              codec = videoTrack ? s.handle('codec') || 'avc1.42c01e' : '';
            } catch (e) {
              codec = videoTrack ? 'avc1.42c01e' : '';
            }
            st.mimeType = defaultType(codec, !!audioTrack);
            s.mime = st.mimeType;
          }
          fire(st, new EventC('start'));
        });
      };
      // The first picture and the first samples are taken in the next task (not now: a requestData() right
      // after start() has nothing to give); the timer carries on from there.
      queue(() => {
        if (st.session !== s) {
          // Stopped before it began: `start` still comes before the data and `stop` of that.
          s.announce = null;
          fire(st, new EventC('start'));
          return;
        }
        tick(st, s);
        if (s.inflight === 0) settled(s);
      });
    }

    stop() {
      const st = recOf(this, 'stop');
      if (st.state === 'inactive') return;
      finish(st, st.session, null);
    }

    pause() {
      const st = recOf(this, 'pause');
      if (st.state === 'inactive') throw domError('InvalidStateError', "Failed to execute 'pause' on 'MediaRecorder': The MediaRecorder's state is 'inactive'.");
      if (st.state === 'paused') return;
      st.state = 'paused';
      const s = st.session;
      if (s.timer) {
        clearTimeoutC(s.timer);
        s.timer = 0;
      }
      s.pausedAt = perfNow();
      queue(() => fire(st, new EventC('pause')));
    }

    resume() {
      const st = recOf(this, 'resume');
      if (st.state === 'inactive') throw domError('InvalidStateError', "Failed to execute 'resume' on 'MediaRecorder': The MediaRecorder's state is 'inactive'.");
      if (st.state === 'recording') return;
      st.state = 'recording';
      const s = st.session;
      s.pausedTotal += perfNow() - s.pausedAt;
      if (s.timeslice !== undefined) s.sliceAt = perfNow() + s.timeslice;
      s.timer = setTimeoutC(() => tick(st, s), 0);
      queue(() => fire(st, new EventC('resume')));
    }

    requestData() {
      const st = recOf(this, 'requestData');
      if (st.state === 'inactive') throw domError('InvalidStateError', "Failed to execute 'requestData' on 'MediaRecorder': The MediaRecorder's state is 'inactive'.");
      const s = st.session;
      // What has been encoded by then: a picture being fetched is waited for; a recorder just started has
      // nothing yet (an empty Blob).
      const give = () => {
        let chunk = null;
        try {
          chunk = s.closed ? null : takeChunk(s, false);
        } catch (e) {
          chunk = null;
        }
        queue(() => fireBlob(st, chunk && chunk[0], s.mime, chunk ? chunk[1] : 0));
      };
      if (s.inflight > 0) s.waiters.push(give);
      else give();
    }

    static isTypeSupported(type) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'isTypeSupported' on 'MediaRecorder': 1 argument required, but only 0 present.");
      return typeSupported(String(type));
    }
  }
  idl(MediaRecorder, 'MediaRecorder');
  O.defineProperty(MediaRecorder, 'isTypeSupported', { enumerable: true });
  for (const type of ['start', 'stop', 'dataavailable', 'pause', 'resume', 'error']) handler(MediaRecorder, type, recCheck);

  return { MediaRecorder, BlobEvent };
});
