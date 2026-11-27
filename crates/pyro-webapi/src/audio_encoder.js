// WebCodecs AudioEncoder control messages. The native handle may emit packets
// from several inputs at once, including when drained for reconfiguration.
(function (native) {
  'use strict';
  const slots = new WeakMap();
  const P = Promise, DE = DOMException, apply = Reflect.apply;
  const closeData = AudioData.prototype.close;
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

  function readOpus(input) {
    const d = dictionary(input);
    const application = enumeration(d.application, 'audio', ['audio', 'voip', 'lowdelay']);
    const complexity = d.complexity;
    const opus = { application };
    // The encoder's default complexity (audio_encoder.rs `complexity`, as Chrome's 9 off mobile).
    opus.complexity = complexity === undefined ? 9 : unsigned(complexity);
    opus.format = enumeration(d.format, 'opus', ['opus', 'ogg']);
    const duration = d.frameDuration;
    opus.frameDuration = duration === undefined ? 20000 : unsigned(duration, 64);
    const loss = d.packetlossperc;
    opus.packetlossperc = loss === undefined ? 0 : unsigned(loss);
    opus.signal = enumeration(d.signal, 'auto', ['auto', 'music', 'voice']);
    opus.usedtx = !!d.usedtx;
    opus.useinbandfec = !!d.useinbandfec;
    return opus;
  }

  function readConfig(input) {
    const d = dictionary(input), config = {};
    const aac = d.aac;
    if (aac !== undefined) config.aac = { format: enumeration(dictionary(aac).format, 'aac', ['aac', 'adts']) };
    const bitrate = d.bitrate;
    if (bitrate !== undefined) config.bitrate = unsigned(bitrate, 64);
    config.bitrateMode = enumeration(d.bitrateMode, 'variable', ['constant', 'variable']);
    const codec = d.codec;
    if (codec === undefined) throw new TypeError('Missing codec.');
    config.codec = `${codec}`;
    config.numberOfChannels = unsigned(d.numberOfChannels);
    const opus = d.opus;
    if (opus !== undefined) config.opus = readOpus(opus);
    config.sampleRate = unsigned(d.sampleRate);
    if (!/[^\t\n\f\r ]/.test(config.codec) || !config.numberOfChannels || !config.sampleRate)
      throw new TypeError('Expected a codec and positive sample rate and channel count.');
    if (config.bitrate !== undefined) {
      // The encoders take a 32-bit signed rate; zero is no rate. Opus works between 6 kbit/s and 510 kbit/s a channel.
      if (!config.bitrate || config.bitrate > 2147483647) throw new TypeError('Invalid bitrate.');
      if (config.codec === 'opus' && (config.bitrate < 6000 || config.bitrate > 510000 * config.numberOfChannels))
        throw new TypeError('Invalid Opus bitrate.');
    }
    if (config.opus) {
      const { frameDuration, complexity, packetlossperc } = config.opus;
      if (!frameDuration || frameDuration > 120000 || frameDuration % 2500 || complexity > 10 || packetlossperc > 100)
        throw new TypeError('Invalid Opus configuration.');
    }
    return config;
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
    s.state = closed ? 'closed' : 'unconfigured';
    s.busy = false;
    for (let op = s.first; op; op = op.next) {
      if (op.kind === 'encode') apply(closeData, op.data, []);
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
          try { packets = s.handle('encode', op.data); }
          finally { apply(closeData, op.data, []); }
        } else {
          packets = s.handle('flush');
        }
      } catch {
        const reason = new DE('Audio encoder ' + op.kind + ' failed.', errorName);
        clear(s, reason, true);
        callback(s.error, [reason]);
        return;
      }

      function advance() { s.busy = false; schedule(s); }
      if (!packets.length && op.kind !== 'flush') { advance(); return; }
      native.postTask(() => {
        if (s.epoch !== epoch) return;
        for (const packet of packets) {
          callback(s.output, [packet.chunk, packet.metadata]);
          if (s.epoch !== epoch) return;
        }
        if (op.kind === 'flush') { s.flushes.delete(op); op.resolve(); }
        advance();
      });
    });
  }

  class AudioEncoder extends EventTarget {
    constructor(init) {
      super();
      const d = dictionary(init);
      const error = d.error;
      if (typeof error !== 'function') throw new TypeError('Expected an error callback.');
      const output = d.output;
      if (typeof output !== 'function') throw new TypeError('Expected an output callback.');
      slots.set(this, { encoder: this, state: 'unconfigured', epoch: 0, count: 0,
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
      enqueue(s, {kind: 'configure', config});
    }
    encode(input) {
      const s = state(this);
      if (!native.isAudioData(input)) throw new TypeError('Expected AudioData.');
      const data = native.capture(input);
      if (s.state !== 'configured') { apply(closeData, data, []); throw invalid(); }
      s.count++;
      enqueue(s, {kind: 'encode', data});
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

  for (const name of Object.getOwnPropertyNames(AudioEncoder.prototype)) {
    if (name !== 'constructor') Object.defineProperty(AudioEncoder.prototype, name, { enumerable: true });
  }
  Object.defineProperty(AudioEncoder, 'isConfigSupported', { enumerable: true });
  Object.defineProperty(AudioEncoder.prototype, Symbol.toStringTag, { value: 'AudioEncoder', configurable: true });
  return { AudioEncoder };
})
