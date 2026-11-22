// WebCodecs VideoEncoder control messages. The native handle may emit packets
// from several inputs at once, including when drained for reconfiguration.
(function (native) {
  'use strict';
  const slots = new WeakMap();
  const P = Promise, DE = DOMException, apply = Reflect.apply;
  const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener;
  const report = typeof reportError === 'function' ? reportError : e => native.postTask(() => { throw e; });

  function dictionary(value) {
    if (value == null) return Object.create(null);
    if (typeof value !== 'object' && typeof value !== 'function') throw new TypeError('Expected a dictionary.');
    return value;
  }

  function unsigned(value, bits = 32) {
    const n = Math.trunc(+value);
    if (!Number.isFinite(n) || n < 0 || n >= 2 ** bits) throw new TypeError('Integer outside range.');
    return n === 0 ? 0 : n;
  }

  function enumeration(value, fallback, choices) {
    if (value === undefined) return fallback;
    const text = `${value}`;
    if (!choices.includes(text)) throw new TypeError('Invalid enumeration member.');
    return text;
  }

  function readConfig(input) {
    const d = dictionary(input), config = {};
    config.alpha = enumeration(d.alpha, 'discard', ['discard', 'keep']);
    const avc = d.avc;
    if (avc !== undefined) config.avc = { format: enumeration(dictionary(avc).format, 'avc', ['avc', 'annexb']) };
    const bitrate = d.bitrate;
    if (bitrate !== undefined) config.bitrate = unsigned(bitrate, 64);
    config.bitrateMode = enumeration(d.bitrateMode, 'variable', ['constant', 'variable', 'quantizer']);
    const codec = d.codec;
    if (codec === undefined) throw new TypeError('Missing codec.');
    config.codec = `${codec}`;
    const contentHint = d.contentHint;
    if (contentHint !== undefined) config.contentHint = `${contentHint}`;
    for (const key of ['displayHeight', 'displayWidth']) {
      const dimension = d[key];
      if (dimension !== undefined) config[key] = unsigned(dimension);
    }
    const framerate = d.framerate;
    if (framerate !== undefined) {
      config.framerate = +framerate;
      if (!Number.isFinite(config.framerate)) throw new TypeError('Expected a finite framerate.');
    }
    config.hardwareAcceleration = enumeration(d.hardwareAcceleration, 'no-preference',
      ['no-preference', 'prefer-hardware', 'prefer-software']);
    config.height = unsigned(d.height);
    const hevc = d.hevc;
    if (hevc !== undefined) config.hevc = { format: enumeration(dictionary(hevc).format, 'hevc', ['hevc', 'annexb']) };
    config.latencyMode = enumeration(d.latencyMode, 'quality', ['quality', 'realtime']);
    const scalabilityMode = d.scalabilityMode;
    if (scalabilityMode !== undefined) config.scalabilityMode = `${scalabilityMode}`;
    config.width = unsigned(d.width);
    if (!/[^\t\n\f\r ]/.test(config.codec) || !config.width || !config.height ||
        config.displayWidth === 0 || config.displayHeight === 0 || config.bitrate === 0)
      throw new TypeError('Expected a codec and positive dimensions and bitrate.');
    return config;
  }

  function readEncodeOptions(input) {
    const d = dictionary(input), options = {};
    for (const key of ['av1', 'avc', 'hevc', 'keyFrame', 'vp9']) {
      const member = d[key];
      if (key === 'keyFrame') options.keyFrame = !!member;
      else if (member !== undefined) {
        const quantizer = dictionary(member).quantizer;
        options[key] = {};
        if (quantizer !== undefined)
          options[key].quantizer = quantizer === null ? null : (+quantizer & 0xffff);
      }
    }
    return options;
  }

  const U8 = Uint8Array, isView = ArrayBuffer.isView;
  const keys = Object.keys, own = Object.hasOwn;

  function descriptionBytes(source) {
    return isView(source) ? new U8(source.buffer, source.byteOffset, source.byteLength) : new U8(source);
  }

  function sameDictionary(a, b) {
    const names = keys(a);
    if (names.length !== keys(b).length) return false;
    for (const name of names) {
      if (!own(b, name)) return false;
      if (name === 'description') {
        const x = descriptionBytes(a[name]), y = descriptionBytes(b[name]);
        if (x.length !== y.length) return false;
        for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return false;
      } else if (name === 'colorSpace') {
        if (!sameDictionary(a[name], b[name])) return false;
      } else if (a[name] !== b[name]) return false;
    }
    return true;
  }

  function outputMetadata(s, metadata) {
    if (!own(metadata, 'decoderConfig')) return metadata;
    const config = metadata.decoderConfig;
    if (s.lastDecoderConfig && sameDictionary(s.lastDecoderConfig, config)) {
      delete metadata.decoderConfig;
    } else {
      // Retain a private copy before author code can change or transfer it.
      const snapshot = { ...config };
      if (own(config, 'colorSpace')) snapshot.colorSpace = { ...config.colorSpace };
      if (own(config, 'description')) snapshot.description = new U8(descriptionBytes(config.description));
      s.lastDecoderConfig = snapshot;
    }
    return metadata;
  }

  function state(receiver) {
    const s = slots.get(receiver);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }

  function callback(fn, args) {
    try { apply(fn, undefined, args); } catch (e) { report(e); }
  }

  function invalid() { return new DE('Invalid encoder state.', 'InvalidStateError'); }

  function dequeue(s) {
    if (s.dequeueScheduled) return;
    s.dequeueScheduled = true;
    native.postTask(() => {
      try { native.fireDequeue(s.encoder); } finally { s.dequeueScheduled = false; }
    });
  }

  function clear(s, reason, closed) {
    s.epoch++;
    s.lastDecoderConfig = null;
    s.state = closed ? 'closed' : 'unconfigured';
    s.busy = false;
    for (let op = s.first; op; op = op.next) {
      if (op.kind === 'encode') native.release(op.frame);
    }
    s.first = s.last = null;
    if (s.count) { s.count = 0; dequeue(s); }
    for (const op of s.flushes) op.reject(reason);
    s.flushes.clear();
    if (s.handle) { s.handle('close'); s.handle = null; }
  }

  function enqueue(s, op) {
    if (s.last) s.last.next = op;
    else s.first = op;
    s.last = op;
    schedule(s);
  }

  function schedule(s) {
    if (s.busy || !s.first) return;
    s.busy = true;
    const epoch = s.epoch;
    native.postTask(() => {
      if (s.epoch !== epoch) return;
      const op = s.first;
      s.first = op.next;
      op.next = null;
      if (!s.first) s.last = null;

      let packets = [], errorName = 'EncodingError';
      try {
        if (op.kind === 'configure') {
          errorName = 'NotSupportedError';
          if (!native.supports(op.config)) throw new Error();
          if (s.handle) {
            errorName = 'EncodingError';
            packets = s.handle('flush');
            s.handle('close');
            s.handle = null;
          }
          errorName = 'NotSupportedError';
          s.handle = native.create(op.config);
        } else if (op.kind === 'encode') {
          s.count--;
          dequeue(s);
          try { packets = s.handle('encode', op.frame, op.options); }
          finally { native.release(op.frame); }
        } else {
          packets = s.handle('flush');
        }
      } catch {
        const reason = new DE('Video encoder ' + op.kind + ' failed.', errorName);
        clear(s, reason, true);
        callback(s.error, [reason]);
        return;
      }

      function advance() { s.busy = false; schedule(s); }
      if (!packets.length && op.kind !== 'flush') { advance(); return; }
      native.postTask(() => {
        if (s.epoch !== epoch) return;
        for (const packet of packets) {
          callback(s.output, [packet.chunk, outputMetadata(s, packet.metadata)]);
          if (s.epoch !== epoch) return;
        }
        if (op.kind === 'flush') { s.flushes.delete(op); op.resolve(); }
        advance();
      });
    });
  }

  class VideoEncoder extends EventTarget {
    constructor(init) {
      super();
      const d = dictionary(init);
      const error = d.error;
      if (typeof error !== 'function') throw new TypeError('Expected an error callback.');
      const output = d.output;
      if (typeof output !== 'function') throw new TypeError('Expected an output callback.');
      slots.set(this, { encoder: this, state: 'unconfigured', lastDecoderConfig: null, epoch: 0, count: 0,
        first: null, last: null, handle: null, busy: false, dequeueScheduled: false,
        flushes: new Set(), handler: null, listener: null, error, output });
    }
    get state() { return state(this).state; }
    get encodeQueueSize() { return state(this).count; }
    get ondequeue() { return state(this).handler; }
    set ondequeue(value) {
      const s = state(this);
      s.handler = typeof value === 'function' ? value : null;
      if (!s.handler && s.listener) {
        apply(remove, this, ['dequeue', s.listener]);
        s.listener = null;
      } else if (s.handler && !s.listener) {
        s.listener = event => apply(s.handler, this, [event]);
        apply(add, this, ['dequeue', s.listener]);
      }
    }
    configure(input) {
      const s = state(this), config = readConfig(input);
      if (s.state === 'closed') throw invalid();
      s.state = 'configured';
      s.orientation = null;
      enqueue(s, {kind: 'configure', config});
    }
    encode(input, options = undefined) {
      const s = state(this);
      if (!native.isVideoFrame(input)) throw new TypeError('Expected VideoFrame.');
      const converted = readEncodeOptions(options);
      const frame = native.capture(input);
      try {
        if (s.state !== 'configured') throw invalid();
        const orientation = native.orientation(frame);
        if (s.orientation !== null && s.orientation !== orientation)
          throw new DE('Video frame orientation changed.', 'DataError');
        s.orientation = orientation;
      } catch (error) {
        native.release(frame);
        throw error;
      }
      s.count++;
      enqueue(s, {kind: 'encode', frame, options: converted});
    }
    flush() {
      return new P((resolve, reject) => {
        const s = state(this);
        if (s.state !== 'configured') throw invalid();
        const op = {kind: 'flush', resolve, reject};
        s.flushes.add(op);
        enqueue(s, op);
      });
    }
    reset() {
      const s = state(this);
      if (s.state === 'closed') throw invalid();
      clear(s, new DE('The encoder was reset.', 'AbortError'), false);
    }
    close() {
      const s = state(this);
      if (s.state === 'closed') throw invalid();
      clear(s, new DE('The encoder was closed.', 'AbortError'), true);
    }
    static isConfigSupported(input) {
      return new P((resolve, reject) => {
        const config = readConfig(input);
        native.postTask(() => {
          try { resolve({supported: native.supports(config), config}); }
          catch (e) { reject(e); }
        });
      });
    }
  }

  for (const name of Object.getOwnPropertyNames(VideoEncoder.prototype)) {
    if (name !== 'constructor') Object.defineProperty(VideoEncoder.prototype, name, { enumerable: true });
  }
  Object.defineProperty(VideoEncoder, 'isConfigSupported', { enumerable: true });
  Object.defineProperty(VideoEncoder.prototype, Symbol.toStringTag, { value: 'VideoEncoder', configurable: true });
  return { VideoEncoder };
})
