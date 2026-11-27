// navigator.mediaDevices for one realm (Media Capture and Streams, MediaDevices). pyro exposes no
// capture device: without the embedder's fake devices enumerateDevices() resolves with an empty list
// and getUserMedia() rejects with NotFoundError, as a browser on a machine without cameras or
// microphones does (media_stream.js has the rest). `native.active()` tells whether the realm's
// document is fully active.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const EventTargetC = EventTarget;
  const ArrayC = Array;
  const resolve = (v) => PromiseC.resolve(v);
  const reject = (e) => PromiseC.reject(e);
  const illegal = () => new TypeErrorC("Failed to construct 'MediaDevices': Illegal constructor");
  const devices = new WeakSet();
  const handlers = new WeakMap();
  const check = (self, what) => {
    if (!devices.has(self)) throw new TypeErrorC(`Failed to execute '${what}' on 'MediaDevices': Illegal invocation`);
    if (!native.active()) throw new DOMExceptionC('The document is not fully active.', 'InvalidStateError');
  };
  class MediaDevices extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal();
      super();
      devices.add(this);
    }
    enumerateDevices() {
      try {
        check(this, 'enumerateDevices');
        return resolve(native.stream('enumerate'));
      } catch (e) {
        return reject(e);
      }
    }
    getSupportedConstraints() {
      if (!devices.has(this)) throw new TypeErrorC("Failed to execute 'getSupportedConstraints' on 'MediaDevices': Illegal invocation");
      return native.stream('supportedConstraints');
    }
    getUserMedia(constraints = {}) {
      try {
        check(this, 'getUserMedia');
        // Constraint validation, the Permissions Policy check (camera/microphone, before any device lookup),
        // the device choice and the stream are media_stream.js's; without the embedder's fake devices it
        // rejects NotFoundError, as a machine without devices does.
        return resolve(native.stream('getUserMedia', constraints));
      } catch (e) {
        return reject(e);
      }
    }
    get ondevicechange() {
      return handlers.get(this) ?? null;
    }
    set ondevicechange(f) {
      const old = handlers.get(this);
      if (old) this.removeEventListener('devicechange', old);
      if (typeof f === 'function' || (f !== null && typeof f === 'object')) {
        handlers.set(this, f);
        this.addEventListener('devicechange', f);
      } else {
        handlers.delete(this);
      }
    }
  }
  O.defineProperty(MediaDevices.prototype, Symbol.toStringTag, { value: 'MediaDevices', configurable: true });
  for (const k of O.getOwnPropertyNames(MediaDevices.prototype)) {
    if (k === 'constructor') continue;
    const d = O.getOwnPropertyDescriptor(MediaDevices.prototype, k);
    d.enumerable = true;
    O.defineProperty(MediaDevices.prototype, k, d);
  }
  return { MediaDevices, mediaDevices: new MediaDevices(token) };
});
