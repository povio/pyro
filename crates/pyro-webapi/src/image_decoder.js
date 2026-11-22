// ImageDecoder's script-visible state lives in the owning realm. The image
// backend reads the encoded byte range and returns canonical VideoFrames.
(function (native) {
  'use strict';
  const decoders = new WeakMap(), lists = new WeakMap(), tracks = new WeakMap();
  const token = Symbol('image track'), U8 = Uint8Array, P = Promise, DE = DOMException;
  const {defineProperty, getOwnPropertyDescriptor, getPrototypeOf} = Object;
  const apply = Reflect.apply;
  const typed = getPrototypeOf(U8.prototype);
  const lengthOf = getOwnPropertyDescriptor(typed, 'byteLength').get;
  const bufferOf = getOwnPropertyDescriptor(typed, 'buffer').get;
  const regularLength = getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const resizable = getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable')?.get;
  const typeOfView = getOwnPropertyDescriptor(typed, Symbol.toStringTag).get;
  const getReader = ReadableStream.prototype.getReader;
  const readerRead = ReadableStreamDefaultReader.prototype.read;
  const readerCancel = ReadableStreamDefaultReader.prototype.cancel;
  const byteLength = value => apply(lengthOf, value, []);
  const invalid = () => new DE('The image decoder is closed or has no selected track.', 'InvalidStateError');
  const aborted = () => new DE('Image decoding was aborted.', 'AbortError');
  const indexError = () => new RangeError('Image frame index out of range.');
  const tooLarge = () => new DE('Encoded image exceeds the byte limit.', 'QuotaExceededError');

  // RFC 9110 media types: token subtype and optional semicolon-separated
  // parameters with token or quoted values. Parsing and codec support are
  // separate: a well-formed MIME type need not have an installed decoder.
  const imageMIME = /^image\/[!#$%&'*+.\-^_`|~a-z0-9]+(?:[\t ]*;[\t ]*(?:[!#$%&'*+.\-^_`|~a-z0-9]+=(?:[!#$%&'*+.\-^_`|~a-z0-9]+|"(?:[\t !#-\[\]-~\x80-\xff]|\\[\t -~\x80-\xff])*"))?)*$/i;
  function checkedType(value) {
    const type = `${value}`;
    if (!imageMIME.test(type)) throw new TypeError('Expected an image MIME type.');
    return type;
  }
  function backendType(type) {
    const separator = type.indexOf(';');
    return separator < 0 ? type.toLowerCase() :
        type.slice(0, separator).toLowerCase() + type.slice(separator);
  }
  function dictionary(value) {
    if (value == null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') throw new TypeError('Expected a dictionary.');
    return value;
  }
  function unsigned(value) {
    const n = Math.trunc(+value);
    if (!Number.isFinite(n) || n < 0 || n > 0xffffffff) throw new TypeError('Expected an unsigned long.');
    return n;
  }
  function slot(map, object) {
    const state = map.get(object);
    if (!state) throw new TypeError('Illegal invocation');
    return state;
  }
  function deferred(handled = false) {
    let resolve, reject;
    const promise = new P((yes, no) => { resolve = yes; reject = no; });
    if (handled) promise.catch(() => {});
    return {promise, resolve, reject};
  }
  function abortRequests(state, reason) {
    for (const req of state.pending) req.reject(reason);
    state.pending.clear();
  }
  function close(state, reason) {
    if (state.closed) return;
    state.closed = true;
    abortRequests(state, reason);
    if (!state.track) state.ready.reject(reason);
    else tracks.get(state.track).decoder = null;
    state.track = null;
    state.selected = false;
    state.source.close();
    if (!state.complete) {
      state.completed.reject(reason);
      if (state.reader) apply(readerCancel, state.reader, [reason]).catch(() => {});
    }
    state.reader = null;
  }

  function process(state) {
    if (state.closed) return;
    if (!native.supports(state.mime)) {
      close(state, new DE('Unsupported image type.', 'NotSupportedError'));
      return;
    }
    try {
      const info = state.source.inspect(state.mime, state.complete);
      if (!info) {
        if (state.complete) close(state, new DE('Image metadata could not be decoded.', 'EncodingError'));
        return;
      }
      if (!state.track) {
        state.track = new ImageTrack(token, state, info);
        state.selected = true;
        state.ready.resolve();
      } else {
        const track = tracks.get(state.track);
        track.count = info.frameCount;
        track.animated = info.animated;
        track.repetitions = info.repetitionCount;
      }

      for (const req of state.pending) {
        if (req.index >= info.frameCount) {
          if (!state.complete) continue;
          state.pending.delete(req);
          req.reject(indexError());
          continue;
        }
        // Progressive output is optional. Hold the request until this backend
        // has a complete frame, regardless of completeFramesOnly's value.
        const image = state.source.decode(state.mime, state.complete, req.index,
                                    state.width, state.height, state.conversion);
        if (!image && !state.complete) continue;
        state.pending.delete(req);
        if (image) req.resolve({image, complete: true});
        else req.reject(indexError());
      }
    } catch (error) {
      close(state, error);
    }
  }
  function schedule(state) {
    if (state.scheduled || state.closed) return;
    state.scheduled = true;
    native.postTask(() => { state.scheduled = false; process(state); });
  }
  function append(source, chunk) {
    if (byteLength(chunk) > native.maxBytes) throw tooLarge();
    // Ordinary fixed bytes may go directly to the native snapshot. Reading
    // a shared or resizable view into a private fixed view is done by JS.
    const buffer = apply(bufferOf, chunk, []);
    let shared = false;
    try { apply(regularLength, buffer, []); }
    catch { shared = true; }
    source.append(shared || (resizable && apply(resizable, buffer, [])) ? new U8(chunk) : chunk);
  }
  async function readStream(state) {
    while (!state.closed) {
      let result;
      try { result = await apply(readerRead, state.reader, []); }
      catch { close(state, new DE('Could not read the image stream.', 'NotReadableError')); return; }
      if (state.closed) return;
      if (result.done) {
        state.complete = true;
        state.completed.resolve();
        schedule(state);
        return;
      }
      if (apply(typeOfView, result.value, []) !== 'Uint8Array') {
        close(state, new DE('Expected a Uint8Array stream chunk.', 'DataError'));
        return;
      }
      try {
        append(state.source, result.value);
        schedule(state);
      }
      catch (error) { close(state, error); return; }
    }
  }

  class ImageTrack {
    constructor(...args) {
      if (args[0] !== token) throw new TypeError('Illegal constructor');
      const [, decoder, info] = args;
      tracks.set(this, {decoder, animated: info.animated, count: info.frameCount,
                        repetitions: info.repetitionCount, selected: true});
    }
    get animated() { return slot(tracks, this).animated; }
    get frameCount() { return slot(tracks, this).count; }
    get repetitionCount() { return slot(tracks, this).repetitions; }
    get selected() { return slot(tracks, this).selected; }
    set selected(value) {
      const track = slot(tracks, this), selected = !!value;
      if (!track.decoder || track.selected === selected) return;
      track.selected = selected;
      track.decoder.selected = selected;
      abortRequests(track.decoder, aborted());
    }
  }

  // A format currently decoded by this backend carries one image track,
  // with as many animation frames as were discovered. The indexed property
  // appears/disappears with that track on the same live list object.
  class ImageTrackList {
    constructor(...args) {
      if (args[0] !== token) throw new TypeError('Illegal constructor');
      const state = args[1];
      const list = new Proxy(this, {
        get(target, key, receiver) {
          if (key === '0' && state.track) return state.track;
          return Reflect.get(target, key, receiver);
        },
        has(target, key) { return (key === '0' && !!state.track) || Reflect.has(target, key); },
        getOwnPropertyDescriptor(target, key) {
          if (key === '0' && state.track)
            return {value: state.track, enumerable: true, configurable: true, writable: false};
          return Reflect.getOwnPropertyDescriptor(target, key);
        },
        ownKeys(target) { return state.track ? ['0', ...Reflect.ownKeys(target)] : Reflect.ownKeys(target); },
        defineProperty(target, key, descriptor) {
          if (typeof key === 'string' && String(key >>> 0) === key && key !== '4294967295') return false;
          return Reflect.defineProperty(target, key, descriptor);
        },
        deleteProperty(target, key) {
          return key === '0' && state.track ? false : Reflect.deleteProperty(target, key);
        },
        preventExtensions() { return false; }
      });
      lists.set(list, state);
      return list;
    }
    get length() { return slot(lists, this).track ? 1 : 0; }
    get selectedIndex() { return slot(lists, this).selected ? 0 : -1; }
    get selectedTrack() { const s = slot(lists, this); return s.selected ? s.track : null; }
    get ready() { return slot(lists, this).ready.promise; }
  }

  class ImageDecoder {
    constructor(init) {
      if (!arguments.length) throw new TypeError('ImageDecoder requires an ImageDecoderInit dictionary.');
      const d = dictionary(init);
      const color = d.colorSpaceConversion;
      const conversion = color === undefined ? 'default' : `${color}`;
      if (conversion !== 'default' && conversion !== 'none') throw new TypeError('Invalid colorSpaceConversion.');
      const data = d.data, stream = native.isStream(data);
      if (!stream) native.fixedByteView(data);
      const heightValue = d.desiredHeight;
      const height = heightValue === undefined ? undefined : unsigned(heightValue);
      const widthValue = d.desiredWidth;
      const width = widthValue === undefined ? undefined : unsigned(widthValue);
      // Read the member even though the installed codecs have one track.
      void d.preferAnimation;
      const transfers = native.chunkTransferList(d.transfer);
      const type = checkedType(d.type);
      if ((width === undefined) !== (height === undefined)) throw new TypeError('Specify both desired dimensions.');
      const input = stream ? null : native.fixedByteView(data);
      if (stream) {
        if (native.isLocked(data) || native.isDisturbed(data)) throw new TypeError('Image stream is already in use.');
      } else {
        if (!byteLength(input)) throw new TypeError('Empty image data.');
        if (byteLength(input) > native.maxBytes) throw tooLarge();
      }
      const source = native.createSource();
      try {
        // Retain the source before detaching anything: an allocation or quota
        // failure leaves every supplied transfer buffer owned by the caller.
        if (!stream) append(source, input);
        native.copyChunkInput(new U8(0), transfers);
      } catch (error) {
        source.close();
        throw error;
      }
      // Current PNG, JPEG and GIF implementations have one track; the
      // preferAnimation hint matters only when a choice between tracks exists.
      const state = {type, mime: backendType(type), width, height, conversion, source,
                     complete: !stream, completed: deferred(true), ready: deferred(true), pending: new Set(),
                     track: null, selected: false, closed: false, scheduled: false, reader: null};
      state.list = new ImageTrackList(token, state);
      decoders.set(this, state);
      if (stream) state.reader = apply(getReader, data, []);
      else state.completed.resolve();
      schedule(state);
      if (stream) readStream(state);
    }
    get tracks() { return slot(decoders, this).list; }
    get type() { return slot(decoders, this).type; }
    get complete() { return slot(decoders, this).complete; }
    get completed() { return slot(decoders, this).completed.promise; }
    decode(options = undefined) {
      try {
        const state = slot(decoders, this), d = dictionary(options);
        // This backend returns complete frames only.
        void d.completeFramesOnly;
        const indexValue = d.frameIndex;
        const index = indexValue === undefined ? 0 : unsigned(indexValue);
        if (state.closed || (state.track && !state.selected)) throw invalid();
        const req = deferred();
        req.index = index;
        state.pending.add(req);
        schedule(state);
        return req.promise;
      } catch (error) { return P.reject(error); }
    }
    reset() { abortRequests(slot(decoders, this), aborted()); }
    close() { close(slot(decoders, this), aborted()); }
    static isTypeSupported(type) {
      try {
        if (!arguments.length) throw new TypeError('isTypeSupported requires a type.');
        const mime = backendType(checkedType(type));
        const result = deferred();
        native.postTask(() => {
          try { result.resolve(native.supports(mime)); }
          catch (error) { result.reject(error); }
        });
        return result.promise;
      } catch (error) { return P.reject(error); }
    }
  }
  for (const C of [ImageDecoder, ImageTrack, ImageTrackList]) {
    defineProperty(C.prototype, Symbol.toStringTag, {value:C.name, configurable:true});
    for (const k of Object.getOwnPropertyNames(C.prototype)) {
      if (k !== 'constructor') defineProperty(C.prototype, k, {enumerable:true});
    }
  }
  defineProperty(ImageDecoder, 'isTypeSupported', {enumerable:true});
  return {ImageDecoder, ImageTrack, ImageTrackList};
})
