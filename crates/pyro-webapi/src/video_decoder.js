// WebCodecs control messages run in order. The private native handle owns
// the decoder and emits decoded pixels in presentation order.
(function (native) {
  'use strict';
  const ET = EventTarget, DE = DOMException, P = Promise;
  const Frame = VideoFrame, ColorSpace = VideoColorSpace, U8 = Uint8Array;
  const apply = Reflect.apply, define = Object.defineProperty;
  const descriptor = Object.getOwnPropertyDescriptor;
  const chunkType = descriptor(EncodedVideoChunk.prototype, 'type').get;
  const add = ET.prototype.addEventListener, remove = ET.prototype.removeEventListener;
  const report = typeof reportError === 'function' ? reportError : e => native.postTask(() => { throw e; });
  const slots = new WeakMap();

  function state(decoder) {
    const s = slots.get(decoder);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }

  function dictionary(value) {
    if (value == null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') throw new TypeError('Expected a dictionary.');
    return value;
  }

  function positiveDimension(value) {
    const number = Math.trunc(+value);
    if (!Number.isFinite(number) || number < 0 || number > 0xffffffff)
      throw new TypeError('Dimension outside unsigned long range.');
    return number;
  }

  const typed = Object.getPrototypeOf(U8.prototype);
  const dataView = DataView.prototype;
  const byteGetters = proto => ['buffer', 'byteOffset', 'byteLength'].map(k => descriptor(proto, k).get);
  const typedView = byteGetters(typed), dataViewInfo = byteGetters(dataView);
  const bufferLength = descriptor(ArrayBuffer.prototype, 'byteLength').get;
  const sharedLength = typeof SharedArrayBuffer === 'function' && descriptor(SharedArrayBuffer.prototype, 'byteLength').get;
  const resizable = descriptor(ArrayBuffer.prototype, 'resizable')?.get;
  const growable = typeof SharedArrayBuffer === 'function' && descriptor(SharedArrayBuffer.prototype, 'growable')?.get;

  function descriptionView(source) {
    let buffer = source, offset = 0, length;
    if (ArrayBuffer.isView(source)) {
      let getters;
      try { buffer = apply(typedView[0], source, []); getters = typedView; }
      catch { buffer = apply(dataViewInfo[0], source, []); getters = dataViewInfo; }
      offset = apply(getters[1], source, []);
      length = apply(getters[2], source, []);
    }
    let size, shared = false;
    try { size = apply(bufferLength, buffer, []); }
    catch {
      if (!sharedLength) throw new TypeError('Expected a BufferSource description.');
      size = apply(sharedLength, buffer, []);
      shared = true;
    }
    const variable = shared ? growable : resizable;
    if (variable && apply(variable, buffer, [])) throw new TypeError('Expected a fixed-size description.');
    return new U8(buffer, offset, length ?? size);
  }

  function readConfig(input) {
    const d = dictionary(input), c = Object.create(null);
    // Read and convert each Web IDL member once, in dictionary order.
    const codec = d.codec;
    if (codec === undefined) throw new TypeError('VideoDecoderConfig.codec is required.');
    c.codec = `${codec}`;
    for (const k of ['codedHeight', 'codedWidth']) {
      const value = d[k];
      if (value !== undefined) c[k] = positiveDimension(value);
    }
    const color = d.colorSpace;
    if (color !== undefined) c.colorSpace = new ColorSpace(color).toJSON();
    const description = d.description;
    if (description !== undefined) descriptionView(description);
    for (const k of ['displayAspectHeight', 'displayAspectWidth']) {
      const value = d[k];
      if (value !== undefined) c[k] = positiveDimension(value);
    }
    c.flip = Boolean(d.flip);
    const hardware = d.hardwareAcceleration;
    c.hardwareAcceleration = hardware === undefined ? 'no-preference' : `${hardware}`;
    if (!['no-preference', 'prefer-software', 'prefer-hardware'].includes(c.hardwareAcceleration))
      throw new TypeError('Invalid hardwareAcceleration.');
    c.optimizeForLatency = Boolean(d.optimizeForLatency);
    const rotation = d.rotation;
    c.rotation = rotation === undefined ? 0 : +rotation;
    if (!Number.isFinite(c.rotation)) throw new TypeError('Rotation must be finite.');
    if (!/[^\t\n\f\r ]/.test(c.codec)) throw new TypeError('Empty video codec.');
    for (const [width, height] of [['codedWidth', 'codedHeight'], ['displayAspectWidth', 'displayAspectHeight']]) {
      if ((c[width] === undefined) !== (c[height] === undefined) || c[width] === 0 || c[height] === 0)
        throw new TypeError('Dimensions must be a pair of positive integers.');
    }
    // A later dictionary getter can detach a description. Snapshot its
    // exact bytes only after all script-provided getters have finished.
    if (description !== undefined) c.description = new U8(descriptionView(description)).buffer;
    return c;
  }

  function invalid() { return new DE('Invalid decoder state.', 'InvalidStateError'); }

  function callback(fn, arg) {
    try { apply(fn, undefined, [arg]); } catch (e) { report(e); }
  }

  function scheduleDequeue(decoder, s) {
    if (s.dequeuePending) return;
    s.dequeuePending = true;
    native.postTask(() => {
      try { native.fireDequeue(decoder); }
      finally { s.dequeuePending = false; }
    });
  }

  function release(s) {
    const handle = s.handle;
    s.handle = null;
    if (handle) handle('close');
  }

  function clear(decoder, s, reason, closed) {
    s.epoch++;
    s.state = closed ? 'closed' : 'unconfigured';
    s.busy = false;
    s.keyRequired = true;
    s.requested = s.active = null;
    s.queue = [];
    s.next = 0;
    if (s.count) {
      s.count = 0;
      scheduleDequeue(decoder, s);
    }
    for (const pending of s.flushes) pending.reject(reason);
    s.flushes.clear();
    release(s);
  }

  function fail(decoder, s, reason) {
    clear(decoder, s, reason, true);
    callback(s.error, reason);
  }

  function output(record, c) {
    const { data, format, codedWidth, codedHeight, timestamp, duration } = record;
    let displayWidth = codedWidth, displayHeight = codedHeight;
    if (c.displayAspectWidth !== undefined) {
      const ratio = c.displayAspectWidth / c.displayAspectHeight;
      if (codedWidth / codedHeight < ratio) displayWidth = Math.round(codedHeight * ratio);
      else displayHeight = Math.round(codedWidth / ratio);
    }
    // VideoFrameInit takes the final display dimensions. Apply orientation
    // after deriving the unrotated size from the stream's display aspect.
    const turns = Math.floor(((((c.rotation % 360) + 360) % 360) + 45) / 90);
    if (turns % 2) [displayWidth, displayHeight] = [displayHeight, displayWidth];
    // The native conversion supplies packed sRGB pixels. VideoFrame's RGBA
    // defaults describe those bytes; YUV matrix metadata wouldn't.
    return new Frame(data, { format, codedWidth, codedHeight, timestamp,
      duration: duration ?? undefined, displayWidth, displayHeight, rotation: c.rotation, flip: c.flip });
  }

  function schedule(decoder, s) {
    if (s.busy || s.next === s.queue.length) return;
    s.busy = true;
    const epoch = s.epoch;
    native.postTask(() => {
      if (epoch !== s.epoch) return;
      const op = s.queue[s.next];
      s.queue[s.next++] = null;
      if (s.next === s.queue.length) { s.queue = []; s.next = 0; }
      else if (s.next >= 64 && s.next * 2 >= s.queue.length) {
        s.queue = s.queue.slice(s.next);
        s.next = 0;
      }
      function advance() {
        if (epoch !== s.epoch) return;
        s.busy = false;
        schedule(decoder, s);
      }

      let frames, config = s.active;
      try {
        if (op.kind === 'configure') {
          if (!native.supports(op.config)) {
            fail(decoder, s, new DE('Unsupported video configuration.', 'NotSupportedError'));
            return;
          }
          // Retain already submitted outputs across reconfiguration. They
          // carry the display metadata of their original configuration.
          try { frames = s.handle ? s.handle('flush') : []; }
          catch {
            fail(decoder, s, new DE('Could not drain the previous video configuration.', 'EncodingError'));
            return;
          }
          release(s);
          s.handle = native.create(op.config);
          s.active = op.config;
        } else if (op.kind === 'decode') {
          s.count--;
          scheduleDequeue(decoder, s);
          frames = s.handle('decode', op.chunk);
        } else {
          frames = s.handle('flush');
        }
      } catch {
        const unavailable = op.kind === 'configure';
        fail(decoder, s, new DE(unavailable ? 'Decoder configuration failed.' : 'Video decoding failed.',
          unavailable ? 'NotSupportedError' : 'EncodingError'));
        return;
      }

      if (!frames.length && op.kind !== 'flush') { advance(); return; }
      native.postTask(() => {
        if (epoch !== s.epoch) return;
        for (const record of frames) {
          let frame;
          try { frame = output(record, config); }
          catch {
            fail(decoder, s, new DE('Cannot construct the decoded video frame.', 'EncodingError'));
            return;
          }
          callback(s.output, frame);
          if (epoch !== s.epoch) return;
        }
        if (op.kind === 'flush') {
          s.flushes.delete(op);
          op.resolve();
        }
        advance();
      });
    });
  }

  class VideoDecoder extends ET {
    constructor(init) {
      super();
      const d = dictionary(init);
      const error = d.error;
      if (typeof error !== 'function') throw new TypeError('VideoDecoderInit.error must be a callback.');
      const output = d.output;
      if (typeof output !== 'function') throw new TypeError('VideoDecoderInit.output must be a callback.');
      slots.set(this, { error, output, state: 'unconfigured', requested: null, active: null, handle: null,
        keyRequired: true, count: 0, queue: [], next: 0, busy: false, epoch: 0,
        flushes: new Set(), dequeuePending: false, handler: null, listener: null });
      define(this, Symbol.toStringTag, {value: 'VideoDecoder', configurable: true});
    }

    get state() { return state(this).state; }
    get decodeQueueSize() { return state(this).count; }
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
      s.keyRequired = true;
      s.requested = config;
      s.queue.push({ kind: 'configure', config });
      schedule(this, s);
    }

    decode(chunk) {
      const s = state(this), type = apply(chunkType, chunk, []);
      if (s.state !== 'configured') throw invalid();
      if (s.keyRequired) {
        if (type !== 'key' || !native.isKeyChunk(s.requested, chunk))
          throw new DE('A key chunk is required.', 'DataError');
        s.keyRequired = false;
      }
      s.count++;
      s.queue.push({ kind: 'decode', chunk });
      schedule(this, s);
    }

    flush() {
      return new P((resolve, reject) => {
        const s = state(this);
        if (s.state !== 'configured') throw invalid();
        s.keyRequired = true;
        const op = { kind: 'flush', resolve, reject };
        s.flushes.add(op);
        s.queue.push(op);
        schedule(this, s);
      });
    }

    reset() {
      const s = state(this);
      if (s.state === 'closed') throw invalid();
      clear(this, s, new DE('The decoder was reset.', 'AbortError'), false);
    }
    close() {
      const s = state(this);
      if (s.state === 'closed') throw invalid();
      clear(this, s, new DE('The decoder was closed.', 'AbortError'), true);
    }

    static isConfigSupported(input) {
      return new P((resolve, reject) => {
        const config = readConfig(input);
        native.postTask(() => {
          try { resolve({ supported: native.supports(config), config: { ...config } }); }
          catch (e) { reject(e); }
        });
      });
    }
  }

  for (const name of Object.getOwnPropertyNames(VideoDecoder.prototype)) {
    if (name !== 'constructor') define(VideoDecoder.prototype, name, { enumerable: true });
  }
  define(VideoDecoder, 'isConfigSupported', { enumerable: true });
  define(VideoDecoder.prototype, Symbol.toStringTag, { value: 'VideoDecoder', configurable: true });
  return { VideoDecoder };
});
