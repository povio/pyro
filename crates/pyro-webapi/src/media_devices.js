// navigator.mediaDevices for one realm (Media Capture and Streams, MediaDevices). pyro exposes no
// capture device: enumerateDevices() resolves with an empty list and getUserMedia() rejects with
// NotFoundError, as a browser on a machine without cameras or microphones does. `native.active()`
// tells whether the realm's document is fully active.
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
        return resolve(ArrayC.of());
      } catch (e) {
        return reject(e);
      }
    }
    getSupportedConstraints() {
      if (!devices.has(this)) throw new TypeErrorC("Failed to execute 'getSupportedConstraints' on 'MediaDevices': Illegal invocation");
      // No device, so no constraint is supported.
      return {};
    }
    getUserMedia(constraints = {}) {
      try {
        check(this, 'getUserMedia');
        if (constraints === null || (typeof constraints !== 'object' && typeof constraints !== 'function')) {
          throw new TypeErrorC("Failed to execute 'getUserMedia' on 'MediaDevices': The provided value is not of type 'MediaStreamConstraints'.");
        }
        const { audio = false, video = false } = constraints;
        if (!audio && !video) {
          throw new TypeErrorC("Failed to execute 'getUserMedia' on 'MediaDevices': At least one of audio and video must be requested");
        }
        // Permissions Policy (`microphone` for audio, `camera` for video): refused before any device lookup.
        const microphone = audio ? native.policy('microphone') : true;
        const camera = video ? native.policy('camera') : true;
        if (!microphone || !camera) throw new DOMExceptionC('Permission denied', 'NotAllowedError');
        return reject(new DOMExceptionC('Requested device not found', 'NotFoundError'));
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
