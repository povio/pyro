// Media Capture and Streams for one realm: MediaStream, MediaStreamTrack (+ stats, content hint),
// MediaStreamTrackEvent, CanvasCaptureMediaStreamTrack, OverconstrainedError, MediaDeviceInfo,
// InputDeviceInfo, ImageCapture, and what feeds them: `canvas.captureStream()`,
// `HTMLMediaElement.captureStream()` and getUserMedia() over the embedder's fake devices.
//
// Source protocol (what other lanes plug into: `makeTrack(kind, source)`, `trackSource(track)`):
//   source = {
//     kind: 'video' | 'audio',
//     label: string,                       // MediaStreamTrack.label
//     tracks: Set,                         // live track states using it (managed here)
//     capabilities(): object,              // MediaStreamTrack.getCapabilities()
//     settings(): object,                  // the settings a new track starts with
//     select?(norm, track): settings,      // applyConstraints(); throws OverconstrainedError
//     pullFrame?(settings): Promise<ImageBitmap> | null   // video: the current frame
//     pullAudio?(count): Float32Array[] | null            // audio: the next `count` samples per channel
//     frameRate?: number                   // native rate (stats)
//     stop?(): void                        // all tracks ended
//   }
// The track/source relation, constraints, ended/mute events and stats are generic; only the
// device models (devices_ below) are synthetic: a headless browser has no camera or microphone,
// so getUserMedia() rejects NotFoundError unless the embedder started the fake devices
// (`--use-fake-device-for-media-stream`, Chrome's switch), when it gets a test-pattern camera and
// a beeping microphone whose frames and samples are computed here.
// `native.active()` tells whether the realm's document is fully active, `native.fakeDevices()`
// whether the embedder enabled the fake devices, `native.elementKinds(el)` what a media element
// has loaded (bit 0 audio, bit 1 video, bit 2 metadata loaded).
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const EventC = Event;
  const EventTargetC = EventTarget;
  const MathC = Math;
  const setTimeoutC = setTimeout;
  // (A page may have removed or replaced the global before this module is first used.)
  const queueMicrotaskC = typeof queueMicrotask === 'function' ? queueMicrotask : (f) => { PromiseC.resolve().then(f); };
  const perfNow = () => performance.now();
  const resolve = (v) => PromiseC.resolve(v);
  const reject = (e) => PromiseC.reject(e);
  const task = (f) => {
    setTimeoutC(f, 0);
  };
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const invocation = (what, cls) => new TypeErrorC(`Failed to execute '${what}' on '${cls}': Illegal invocation`);

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
  // `onfoo` event handler attributes: one listener per object, added when the first function is set.
  const handlerState = new WeakMap();
  const handler = (cls, type, check) => {
    O.defineProperty(cls.prototype, 'on' + type, {
      configurable: true,
      enumerable: true,
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
    });
  };

  const uuid = () => {
    const hex = '0123456789abcdef';
    let s = '';
    for (let i = 0; i < 36; i++) {
      if (i === 8 || i === 13 || i === 18 || i === 23) s += '-';
      else if (i === 14) s += '4';
      else if (i === 19) s += hex[8 + ((MathC.random() * 4) | 0)];
      else s += hex[(MathC.random() * 16) | 0];
    }
    return s;
  };
  const hex64 = (text) => {
    // A stable 64-hex-digit identifier (what Chrome's hashed device ids look like).
    let out = '';
    let h = 2166136261;
    for (let round = 0; round < 8; round++) {
      const s = round + ':' + text;
      for (let i = 0; i < s.length; i++) {
        h ^= s.charCodeAt(i);
        h = MathC.imul(h, 16777619) >>> 0;
      }
      out += (h >>> 0).toString(16).padStart(8, '0');
    }
    return out;
  };

  // ---- OverconstrainedError -------------------------------------------------------------------
  const constraintOf = new WeakMap();
  class OverconstrainedError extends DOMExceptionC {
    constructor(constraint, message = '') {
      if (arguments.length === 0) throw new TypeErrorC("Failed to construct 'OverconstrainedError': 1 argument required, but only 0 present.");
      super(String(message), 'OverconstrainedError');
      constraintOf.set(this, String(constraint));
    }
    get constraint() {
      if (!constraintOf.has(this)) throw invocation('constraint', 'OverconstrainedError');
      return constraintOf.get(this);
    }
  }
  idl(OverconstrainedError, 'OverconstrainedError');
  const overconstrained = (name, message) => new OverconstrainedError(name, message || 'Constraint could not be satisfied.');

  // ---- Constraints ----------------------------------------------------------------------------
  const NUM = 1;
  const ULONG = 2;
  const STR = 3;
  const BOOL = 4;
  const BOOLSTR = 5;
  const ANY = 6;
  const VIDEO_PROPS = {
    width: ULONG, height: ULONG, aspectRatio: NUM, frameRate: NUM, facingMode: STR, resizeMode: STR,
    deviceId: STR, groupId: STR,
    whiteBalanceMode: STR, exposureMode: STR, focusMode: STR, pointsOfInterest: ANY,
    exposureCompensation: NUM, exposureTime: NUM, colorTemperature: NUM, iso: NUM, brightness: NUM,
    contrast: NUM, saturation: NUM, sharpness: NUM, focusDistance: NUM, pan: NUM, tilt: NUM, zoom: NUM, torch: BOOL,
  };
  const AUDIO_PROPS = {
    sampleRate: ULONG, sampleSize: ULONG, echoCancellation: BOOLSTR, autoGainControl: BOOL, noiseSuppression: BOOL,
    voiceIsolation: BOOL, latency: NUM, channelCount: ULONG, deviceId: STR, groupId: STR,
  };
  // Names getSupportedConstraints() reports (the audio and video ones, and Image Capture's).
  const SUPPORTED = [
    'width', 'height', 'aspectRatio', 'frameRate', 'facingMode', 'resizeMode', 'sampleRate', 'sampleSize',
    'echoCancellation', 'autoGainControl', 'noiseSuppression', 'voiceIsolation', 'latency', 'channelCount',
    'deviceId', 'groupId', 'whiteBalanceMode', 'exposureMode', 'focusMode', 'pointsOfInterest',
    'exposureCompensation', 'exposureTime', 'colorTemperature', 'iso', 'brightness', 'contrast', 'saturation',
    'sharpness', 'focusDistance', 'pan', 'tilt', 'zoom', 'torch',
  ];
  const ECHO_MODES = ['all', 'remote-only'];
  const isObj = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  const toNum = (v, name) => {
    const n = +v;
    if (!(MathC.abs(n) < Infinity)) throw new TypeErrorC(`The provided double value for '${name}' is non-finite.`);
    return n;
  };
  const toULong = (v) => {
    const n = +v;
    if (n !== n) return 0;
    return MathC.min(MathC.max(MathC.round(n), 0), 4294967295);
  };
  const toStrs = (v) => {
    if (v === undefined) return undefined;
    if (isObj(v) && typeof v[Symbol.iterator] === 'function') return Array.from(v, String);
    return [String(v)];
  };
  const parseValue = (v, type, name) => {
    const conv = type === ULONG ? toULong : (x) => toNum(x, name);
    switch (type) {
      case NUM:
      case ULONG: {
        if (!isObj(v)) return { ideal: conv(v) };
        const o = {};
        for (const k of ['exact', 'ideal', 'min', 'max']) if (v[k] !== undefined) o[k] = conv(v[k]);
        return o;
      }
      case STR: {
        if (!isObj(v) || typeof v[Symbol.iterator] === 'function') return { ideal: toStrs(v) };
        const o = {};
        const e = toStrs(v.exact);
        const i = toStrs(v.ideal);
        if (e !== undefined) o.exact = e;
        if (i !== undefined) o.ideal = i;
        return o;
      }
      case BOOL: {
        if (!isObj(v)) return { ideal: !!v };
        const o = {};
        if (v.exact !== undefined) o.exact = !!v.exact;
        if (v.ideal !== undefined) o.ideal = !!v.ideal;
        return o;
      }
      case BOOLSTR: {
        const one = (x) => {
          if (typeof x === 'string') {
            if (!ECHO_MODES.includes(x)) {
              throw new TypeErrorC(`The provided value '${x}' is not a valid enum value of type EchoCancellationModeEnum.`);
            }
            return x;
          }
          return !!x;
        };
        if (!isObj(v)) return { ideal: one(v) };
        const o = {};
        if (v.exact !== undefined) o.exact = one(v.exact);
        if (v.ideal !== undefined) o.ideal = one(v.ideal);
        return o;
      }
      default:
        return isObj(v) ? { ideal: v } : { ideal: v };
    }
  };
  // A MediaTrackConstraints dictionary as {set: {name: parsed}, advanced: [{name: parsed}]} for `kind`.
  const parseSet = (dict, props) => {
    const set = {};
    for (const name of O.keys(props)) {
      const v = dict[name];
      if (v !== undefined) set[name] = parseValue(v, props[name], name);
    }
    return set;
  };
  const parseConstraints = (dict, kind) => {
    if (dict === undefined || dict === null) dict = {};
    if (!isObj(dict)) throw new TypeErrorC("Failed to execute 'applyConstraints' on 'MediaStreamTrack': The provided value is not of type 'MediaTrackConstraints'.");
    const props = kind === 'video' ? VIDEO_PROPS : AUDIO_PROPS;
    const out = { set: parseSet(dict, props), advanced: [] };
    const adv = dict.advanced;
    if (adv !== undefined) {
      if (!isObj(adv) || typeof adv[Symbol.iterator] !== 'function') {
        throw new TypeErrorC("The provided value cannot be converted to a sequence.");
      }
      for (const a of adv) {
        if (!isObj(a)) throw new TypeErrorC('The provided value is not of type MediaTrackConstraintSet.');
        out.advanced.push(parseSet(a, props));
      }
    }
    return out;
  };
  const hasHard = (c) => c.exact !== undefined || c.min !== undefined || c.max !== undefined;
  // A copy of the dictionary as getConstraints() reports it.
  const copyConstraints = (v) => {
    if (Array.isArray(v)) return v.map(copyConstraints);
    if (isObj(v)) {
      const o = {};
      for (const k of O.keys(v)) if (v[k] !== undefined) o[k] = copyConstraints(v[k]);
      return o;
    }
    return v;
  };
  const checkLongStrings = (set) => {
    for (const name of O.keys(set)) {
      const c = set[name];
      for (const list of [c.exact, c.ideal]) {
        if (list && list.some && list.some((s) => typeof s === 'string' && s.length > 500)) {
          throw overconstrained(name, 'Constraint string is too long.');
        }
      }
    }
  };
  // Picks a number inside [lo, hi] for constraint `c`: null when the hard bounds exclude every value.
  const pickNum = (c, lo, hi, int, preferred) => {
    let a = lo;
    let b = hi;
    if (c.exact !== undefined) {
      if (c.exact < a || c.exact > b) return null;
      a = b = c.exact;
    }
    if (c.min !== undefined) a = MathC.max(a, c.min);
    if (c.max !== undefined) b = MathC.min(b, c.max);
    if (int) {
      a = MathC.ceil(a);
      b = MathC.floor(b);
    }
    if (!(a <= b)) return null;
    const target = c.ideal !== undefined ? c.ideal : preferred;
    const v = target === undefined ? b : MathC.min(MathC.max(target, a), b);
    return int ? MathC.round(v) : v;
  };
  const distance = (v, ideal) => (ideal === undefined ? 0 : Math.abs(v - ideal) / MathC.max(MathC.abs(v), MathC.abs(ideal), 1e-9));
  const strOk = (c, value) => {
    if (c.exact !== undefined && !c.exact.includes(value)) return false;
    return true;
  };
  const strDistance = (c, value) => (c.ideal === undefined || c.ideal.includes(value) ? 0 : 1);
  // Chooses a boolean/echo value from `allowed` (the device's capability list); null when `exact` is out.
  const pickBool = (c, allowed, dflt) => {
    if (c.exact !== undefined) return allowed.includes(c.exact) ? c.exact : null;
    if (c.ideal !== undefined && allowed.includes(c.ideal)) return c.ideal;
    return dflt;
  };
  // Intersects two parsed sets (an advanced set is all hard, its `ideal`s dropped).
  const hardify = (set) => {
    const out = {};
    for (const name of O.keys(set)) {
      const c = set[name];
      const h = {};
      for (const k of ['exact', 'min', 'max']) if (c[k] !== undefined) h[k] = c[k];
      if (O.keys(h).length) out[name] = h;
    }
    return out;
  };
  const merge = (a, b) => {
    const out = {};
    for (const name of new Set([...O.keys(a), ...O.keys(b)])) {
      const x = a[name] || {};
      const y = b[name] || {};
      const m = O.assign({}, x);
      if (y.exact !== undefined) {
        if (m.exact === undefined) m.exact = y.exact;
        else if (Array.isArray(m.exact)) m.exact = m.exact.filter((s) => y.exact.includes(s));
        else if (m.exact !== y.exact) m.exact = NaN;
      }
      if (y.min !== undefined) m.min = m.min === undefined ? y.min : MathC.max(m.min, y.min);
      if (y.max !== undefined) m.max = m.max === undefined ? y.max : MathC.min(m.max, y.max);
      out[name] = m;
    }
    return out;
  };

  // ---- Device models (synthetic) --------------------------------------------------------------
  // Chrome's fake video capture device offers these native formats at 20 frames per second.
  const FORMATS = [[96, 96], [320, 240], [640, 480], [1280, 720], [1920, 1080], [3840, 2160]];
  const NATIVE_FPS = 20;
  const AUDIO_RATE = 48000;
  const devices_ = () => [
    { kind: 'videoinput', id: 'fake_device_0', label: 'fake_device_0', group: 'fake_group_video_0', facing: 'user' },
    { kind: 'videoinput', id: 'fake_device_1', label: 'fake_device_1', group: 'fake_group_video_1', facing: 'environment' },
    { kind: 'audioinput', id: 'default', label: 'Default - Fake Audio Input', group: 'fake_group_audio_0' },
    { kind: 'audioinput', id: 'fake_audio_input_1', label: 'Fake Audio Input 1', group: 'fake_group_audio_1' },
    { kind: 'audioinput', id: 'fake_audio_input_2', label: 'Fake Audio Input 2', group: 'fake_group_audio_2' },
    { kind: 'audiooutput', id: 'default', label: 'Default - Fake Audio Output', group: 'fake_group_audio_0' },
    { kind: 'audiooutput', id: 'fake_audio_output_1', label: 'Fake Audio Output 1', group: 'fake_group_audio_1' },
  ];
  const deviceId = (d) => (d.id === 'default' ? 'default' : hex64('id:' + d.id));
  const groupId = (d) => hex64('group:' + d.group);
  const videoCapabilities = (d) => ({
    width: { min: 1, max: 3840 },
    height: { min: 1, max: 2160 },
    aspectRatio: { min: 1 / 2160, max: 3840 },
    frameRate: { min: 1, max: NATIVE_FPS },
    facingMode: [d.facing],
    resizeMode: ['none', 'crop-and-scale'],
    deviceId: deviceId(d),
    groupId: groupId(d),
  });
  const audioCapabilities = (d) => ({
    sampleRate: { min: AUDIO_RATE, max: AUDIO_RATE },
    sampleSize: { min: 16, max: 16 },
    echoCancellation: [true, false, 'all', 'remote-only'],
    autoGainControl: [true, false],
    noiseSuppression: [true, false],
    voiceIsolation: [false],
    latency: { min: 0.01, max: 0.01 },
    channelCount: { min: 1, max: 2 },
    deviceId: deviceId(d),
    groupId: groupId(d),
  });
  // Properties a device does not have at all: a hard constraint on them cannot be met.
  const VIDEO_CAP_NAMES = ['width', 'height', 'aspectRatio', 'frameRate', 'facingMode', 'resizeMode', 'deviceId', 'groupId'];
  const AUDIO_CAP_NAMES = ['sampleRate', 'sampleSize', 'echoCancellation', 'autoGainControl', 'noiseSuppression', 'voiceIsolation', 'latency', 'channelCount', 'deviceId', 'groupId'];
  // The numeric part of a video candidate (device, native format, resize mode) against a parsed set.
  const evalVideo = (cand, set) => {
    const [fw, fh] = cand.format;
    const crop = cand.mode === 'crop-and-scale';
    const cw = set.width;
    const ch = set.height;
    let w;
    let h;
    let fit = 0;
    if (!crop) {
      w = fw;
      h = fh;
      if (cw && pickNum(cw, w, w, true, w) === null) return { fail: 'width' };
      if (ch && pickNum(ch, h, h, true, h) === null) return { fail: 'height' };
    } else {
      if (cw) {
        w = pickNum(cw, 1, fw, true, undefined);
        if (w === null) return { fail: 'width' };
      }
      if (ch) {
        h = pickNum(ch, 1, fh, true, undefined);
        if (h === null) return { fail: 'height' };
      }
      // A dimension nobody constrained follows the other one at the native aspect ratio.
      if (w === undefined && h === undefined) {
        w = fw;
        h = fh;
      } else if (w === undefined) {
        w = MathC.min(fw, MathC.max(1, MathC.round((h * fw) / fh)));
      } else if (h === undefined) {
        h = MathC.min(fh, MathC.max(1, MathC.round((w * fh) / fw)));
      }
    }
    if (cw) fit += distance(w, cw.ideal);
    if (ch) fit += distance(h, ch.ideal);
    const ar = set.aspectRatio;
    if (ar) {
      const lo = crop ? 1 / fh : fw / fh;
      const hi = crop ? fw : fw / fh;
      const v = pickNum(ar, lo, hi, false, undefined);
      if (v === null) return { fail: 'aspectRatio' };
      // Cropping lets the output follow the asked ratio when no dimension was pinned.
      if (crop && !cw && !ch) {
        const target = MathC.min(MathC.max(ar.ideal !== undefined ? ar.ideal : fw / fh, lo), hi);
        w = target >= 1 ? fw : MathC.max(1, MathC.round(fh * target));
        h = target >= 1 ? MathC.max(1, MathC.round(fw / target)) : fh;
        h = MathC.min(h, fh);
      }
      const got = w / h;
      if ((ar.min !== undefined && got < ar.min - 1e-3) || (ar.max !== undefined && got > ar.max + 1e-3) ||
          (ar.exact !== undefined && MathC.abs(got - ar.exact) > 1e-3)) {
        return { fail: 'aspectRatio' };
      }
      fit += distance(got, ar.ideal);
    }
    let frameRate = NATIVE_FPS;
    const fr = set.frameRate;
    if (fr) {
      const v = pickNum(fr, 1, NATIVE_FPS, false, NATIVE_FPS);
      if (v === null) return { fail: 'frameRate' };
      frameRate = v;
      fit += distance(v, fr.ideal);
    }
    return { w, h, frameRate, fit };
  };
  const evalAudio = (cand, set) => {
    const d = cand.device;
    const out = { sampleRate: AUDIO_RATE, sampleSize: 16, echoCancellation: true, autoGainControl: true, noiseSuppression: true, voiceIsolation: false, latency: 0.01, channelCount: 1 };
    let fit = 0;
    const caps = audioCapabilities(d);
    for (const name of ['sampleRate', 'sampleSize', 'latency', 'channelCount']) {
      const c = set[name];
      if (!c) continue;
      const cap = caps[name];
      const v = pickNum(c, cap.min, cap.max, name !== 'latency', out[name]);
      if (v === null) return { fail: name };
      out[name] = v;
      fit += distance(v, c.ideal);
    }
    for (const name of ['echoCancellation', 'autoGainControl', 'noiseSuppression', 'voiceIsolation']) {
      const c = set[name];
      if (!c) continue;
      const v = pickBool(c, caps[name], out[name]);
      if (v === null) return { fail: name };
      out[name] = v;
      if (c.ideal !== undefined && c.ideal !== v) fit += 1;
    }
    return { settings: out, fit };
  };
  const NUMERIC_VIDEO = ['width', 'height', 'aspectRatio', 'frameRate'];
  const NUMERIC_AUDIO = ['sampleRate', 'sampleSize', 'echoCancellation', 'autoGainControl', 'noiseSuppression', 'voiceIsolation', 'latency', 'channelCount'];
  // The selection of the Media Capture spec (SelectSettings), over the candidates of `kind`.
  // Throws OverconstrainedError naming the constraint that emptied the candidate set.
  const selectSettings = (kind, candidates, norm) => {
    const props = kind === 'video' ? VIDEO_PROPS : AUDIO_PROPS;
    const capNames = kind === 'video' ? VIDEO_CAP_NAMES : AUDIO_CAP_NAMES;
    const supported = (name) => SUPPORTED.includes(name);
    checkLongStrings(norm.set);
    // 1. A hard constraint on a supported property the devices lack (torch, zoom, ...).
    for (const name of O.keys(norm.set)) {
      if (!supported(name) || capNames.includes(name)) continue;
      const c = norm.set[name];
      if (hasHard(c)) throw overconstrained(name);
    }
    let cands = candidates;
    // 2. Discrete properties, in a fixed order.
    const discrete = kind === 'video' ? ['deviceId', 'groupId', 'facingMode', 'resizeMode'] : ['deviceId', 'groupId'];
    for (const name of discrete) {
      const c = norm.set[name];
      if (!c) continue;
      const value = (cand) => {
        switch (name) {
          case 'deviceId': return deviceId(cand.device);
          case 'groupId': return groupId(cand.device);
          case 'facingMode': return cand.device.facing;
          default: return cand.mode;
        }
      };
      const kept = cands.filter((cand) => strOk(c, value(cand)));
      if (kept.length === 0) throw overconstrained(name);
      cands = kept;
    }
    // 3. The numeric and boolean properties, jointly per candidate.
    const evaluate = kind === 'video' ? evalVideo : evalAudio;
    const evalSet = (cand, set) => evaluate(cand, set);
    let scored = [];
    let firstFail = null;
    for (const cand of cands) {
      const r = evalSet(cand, norm.set);
      if (r.fail) {
        if (firstFail === null) firstFail = r.fail;
      } else {
        scored.push({ cand, r });
      }
    }
    if (scored.length === 0) throw overconstrained(firstFail || (kind === 'video' ? NUMERIC_VIDEO : NUMERIC_AUDIO)[0]);
    // 4. Advanced sets: each narrows the candidates when some candidate satisfies it.
    let basis = norm.set;
    for (const adv of norm.advanced) {
      const hard = hardify(adv);
      const merged = merge(basis, hard);
      const kept = [];
      for (const { cand } of scored) {
        const keepDiscrete = discrete.every((name) => {
          const c = adv[name];
          if (!c || c.exact === undefined) return true;
          const value = name === 'deviceId' ? deviceId(cand.device) : name === 'groupId' ? groupId(cand.device) : name === 'facingMode' ? cand.device.facing : cand.mode;
          return c.exact.includes(value);
        });
        if (!keepDiscrete) continue;
        const r = evalSet(cand, merged);
        if (!r.fail) kept.push({ cand, r });
      }
      if (kept.length) {
        scored = kept;
        basis = merged;
      }
    }
    // 5. The fittest candidate; ties go to the native resize mode and the device order.
    let best = null;
    for (const entry of scored) {
      const { cand, r } = entry;
      let fit = r.fit;
      if (kind === 'video') {
        const [fw, fh] = cand.format;
        for (const name of ['deviceId', 'facingMode', 'resizeMode']) {
          const c = norm.set[name];
          if (c) fit += strDistance(c, name === 'deviceId' ? deviceId(cand.device) : name === 'facingMode' ? cand.device.facing : cand.mode);
        }
        // Default ideal: 640x480, so an unconstrained request opens that format.
        fit += 1e-3 * (MathC.abs(fw - 640) / 640 + MathC.abs(fh - 480) / 480) + (cand.mode === 'none' ? 0 : 1e-5);
      } else {
        const c = norm.set.deviceId;
        if (c) fit += strDistance(c, deviceId(cand.device));
      }
      if (best === null || fit < best.fit - 1e-12) best = { cand, r, fit };
    }
    const { cand, r } = best;
    return { cand, r };
  };
  const videoSettingsOf = (cand, r) => ({
    deviceId: deviceId(cand.device),
    groupId: groupId(cand.device),
    width: r.w,
    height: r.h,
    aspectRatio: r.w / r.h,
    frameRate: r.frameRate,
    facingMode: cand.device.facing,
    resizeMode: cand.mode,
  });
  const audioSettingsOf = (cand, r) => O.assign({ deviceId: deviceId(cand.device), groupId: groupId(cand.device) }, r.settings);
  const videoCandidates = (devs) => {
    const out = [];
    for (const device of devs) {
      for (const format of FORMATS) {
        for (const mode of ['none', 'crop-and-scale']) out.push({ device, format, mode });
      }
    }
    return out;
  };

  // ---- Track state ----------------------------------------------------------------------------
  const tracks = new WeakMap(); // MediaStreamTrack -> state
  const streams = new WeakMap(); // MediaStream -> {id, tracks: []}
  const trackOf = (t, what) => {
    const st = tracks.get(t);
    if (!st) throw invocation(what, 'MediaStreamTrack');
    return st;
  };
  const isTrack = (v) => isObj(v) && tracks.has(v);
  const isStream = (v) => isObj(v) && streams.has(v);

  const activeMs = (st, now) => st.activeMs + (st.since === null ? 0 : now - st.since);
  const flushStats = (st) => {
    const now = perfNow();
    if (st.since !== null) {
      st.activeMs += now - st.since;
      st.since = null;
    }
  };
  const resumeStats = (st) => {
    if (st.since === null && st.readyState === 'live' && st.enabled && !st.muted) st.since = perfNow();
  };
  const fire = (target, event) => {
    task(() => target.dispatchEvent(event));
  };
  const setMuted = (st, muted) => {
    if (st.muted === muted || st.readyState !== 'live') return;
    st.muted = muted;
    if (muted) flushStats(st);
    else resumeStats(st);
    fire(st.object, new EventC(muted ? 'mute' : 'unmute'));
  };
  const endTrack = (st, byUser) => {
    if (st.readyState === 'ended') return;
    flushStats(st);
    st.readyState = 'ended';
    const source = st.source;
    source.tracks.delete(st);
    if (source.tracks.size === 0 && typeof source.stop === 'function') source.stop();
    if (!byUser) fire(st.object, new EventC('ended'));
  };

  class MediaStreamTrackEvent extends EventC {
    constructor(type, init) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'MediaStreamTrackEvent': 2 arguments required, but only ${arguments.length} present.`);
      if (!isObj(init)) throw new TypeErrorC("Failed to construct 'MediaStreamTrackEvent': The provided value is not of type 'MediaStreamTrackEventInit'.");
      super(type, init);
      const track = init.track;
      if (!isTrack(track)) throw new TypeErrorC("Failed to construct 'MediaStreamTrackEvent': Failed to read the 'track' property from 'MediaStreamTrackEventInit': Required member is undefined.");
      eventTracks.set(this, track);
    }
    get track() {
      if (!eventTracks.has(this)) throw invocation('track', 'MediaStreamTrackEvent');
      return eventTracks.get(this);
    }
  }
  const eventTracks = new WeakMap();
  idl(MediaStreamTrackEvent, 'MediaStreamTrackEvent');

  const CONTENT_HINTS = { audio: ['', 'speech', 'speech-recognition', 'music'], video: ['', 'motion', 'detail', 'text'] };

  // Stats are computed from the track's active time when read, once per task (a microtask ends the
  // snapshot, so a task that spins sees them unchanged while an awaiting loop sees them advance).
  const statsObjects = new WeakMap();
  class MediaStreamTrackVideoStats {
    constructor(t) {
      if (t !== token) throw illegal('MediaStreamTrackVideoStats');
    }
    get deliveredFrames() { return snapshot(this, 'deliveredFrames').deliveredFrames; }
    get discardedFrames() { return snapshot(this, 'discardedFrames').discardedFrames; }
    get totalFrames() { return snapshot(this, 'totalFrames').totalFrames; }
    toJSON() {
      const s = snapshot(this, 'toJSON');
      return { deliveredFrames: s.deliveredFrames, discardedFrames: s.discardedFrames, totalFrames: s.totalFrames };
    }
  }
  class MediaStreamTrackAudioStats {
    constructor(t) {
      if (t !== token) throw illegal('MediaStreamTrackAudioStats');
    }
    get deliveredFrames() { return snapshot(this, 'deliveredFrames').deliveredFrames; }
    get deliveredFramesDuration() { return snapshot(this, 'deliveredFramesDuration').deliveredFramesDuration; }
    get totalFrames() { return snapshot(this, 'totalFrames').totalFrames; }
    get totalFramesDuration() { return snapshot(this, 'totalFramesDuration').totalFramesDuration; }
    get latency() { return snapshot(this, 'latency').latency; }
    get averageLatency() { return snapshot(this, 'averageLatency').averageLatency; }
    get minimumLatency() { return snapshot(this, 'minimumLatency').minimumLatency; }
    get maximumLatency() { return snapshot(this, 'maximumLatency').maximumLatency; }
    resetLatency() {
      const rec = statsObjects.get(this);
      if (!rec) throw invocation('resetLatency', 'MediaStreamTrackAudioStats');
      const s = snapshot(this, 'resetLatency');
      rec.latencies = { sum: s.latency, count: 1, min: s.latency, max: s.latency };
      rec.snap = null;
    }
    toJSON() {
      const s = snapshot(this, 'toJSON');
      const o = {};
      for (const k of ['deliveredFrames', 'deliveredFramesDuration', 'totalFrames', 'totalFramesDuration', 'latency', 'averageLatency', 'minimumLatency', 'maximumLatency']) o[k] = s[k];
      return o;
    }
  }
  const AUDIO_PACKET_MS = 10;
  const snapshot = (obj, what) => {
    const rec = statsObjects.get(obj);
    if (!rec) throw invocation(what, 'MediaStreamTrackStats');
    if (rec.snap) return rec.snap;
    const st = rec.st;
    const ms = activeMs(st, perfNow());
    let snap;
    if (st.kind === 'video') {
      const native = (st.source.frameRate || NATIVE_FPS);
      const applied = Math.min(st.settings.frameRate || native, native);
      const total = MathC.floor((ms / 1000) * native);
      const delivered = MathC.min(total, MathC.floor((ms / 1000) * applied));
      snap = { totalFrames: total, deliveredFrames: delivered, discardedFrames: total - delivered };
    } else {
      const total = MathC.floor(ms / AUDIO_PACKET_MS);
      const latency = (st.source.latencyMs !== undefined ? st.source.latencyMs : 10);
      const l = rec.latencies;
      if (l.count === 0) {
        l.sum = latency;
        l.count = 1;
        l.min = latency;
        l.max = latency;
      }
      snap = {
        totalFrames: total, deliveredFrames: total,
        totalFramesDuration: total * AUDIO_PACKET_MS, deliveredFramesDuration: total * AUDIO_PACKET_MS,
        latency, averageLatency: l.sum / l.count, minimumLatency: MathC.min(l.min, latency), maximumLatency: MathC.max(l.max, latency),
      };
    }
    rec.snap = snap;
    queueMicrotaskC(() => {
      rec.snap = null;
    });
    return snap;
  };
  idl(MediaStreamTrackVideoStats, 'MediaStreamTrackVideoStats');
  idl(MediaStreamTrackAudioStats, 'MediaStreamTrackAudioStats');

  const trackCheck = (t) => {
    if (!tracks.has(t)) throw new TypeErrorC('Illegal invocation');
  };

  class MediaStreamTrack extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('MediaStreamTrack');
      super();
    }
    get kind() { return trackOf(this, 'kind').kind; }
    get id() { return trackOf(this, 'id').id; }
    get label() { return trackOf(this, 'label').label; }
    get enabled() { return trackOf(this, 'enabled').enabled; }
    set enabled(v) {
      const st = trackOf(this, 'enabled');
      const on = !!v;
      if (st.enabled === on) return;
      st.enabled = on;
      if (on) resumeStats(st);
      else flushStats(st);
    }
    get muted() { return trackOf(this, 'muted').muted; }
    get readyState() { return trackOf(this, 'readyState').readyState; }
    get contentHint() { return trackOf(this, 'contentHint').contentHint; }
    set contentHint(v) {
      const st = trackOf(this, 'contentHint');
      const s = String(v);
      if (CONTENT_HINTS[st.kind].includes(s)) st.contentHint = s;
    }
    get stats() {
      const st = trackOf(this, 'stats');
      if (!st.statsObject) {
        const obj = new (st.kind === 'video' ? MediaStreamTrackVideoStats : MediaStreamTrackAudioStats)(token);
        statsObjects.set(obj, { st, snap: null, latencies: { sum: 0, count: 0, min: 0, max: 0 } });
        st.statsObject = obj;
      }
      return st.statsObject;
    }
    stop() {
      endTrack(trackOf(this, 'stop'), true);
    }
    clone() {
      const st = trackOf(this, 'clone');
      const c = createTrack(st.cls, st.kind, st.source, {
        label: st.label, enabled: st.enabled, muted: st.muted, settings: O.assign({}, st.settings), constraints: copyConstraints(st.constraints),
        contentHint: st.contentHint, extra: st.extra, ended: st.readyState === 'ended',
      });
      return c;
    }
    getCapabilities() {
      const st = trackOf(this, 'getCapabilities');
      return st.source.capabilities();
    }
    getConstraints() {
      return copyConstraints(trackOf(this, 'getConstraints').constraints);
    }
    getSettings() {
      return O.assign({}, trackOf(this, 'getSettings').settings);
    }
    applyConstraints(constraints = {}) {
      let st;
      try {
        st = trackOf(this, 'applyConstraints');
        const norm = parseConstraints(constraints, st.kind);
        const settings = reconfigure(st, norm);
        return new PromiseC((res) => {
          queueMicrotaskC(() => {
            if (settings) st.settings = settings;
            st.constraints = copyConstraints(constraints === undefined || constraints === null ? {} : constraints);
            res();
          });
        });
      } catch (e) {
        return reject(e);
      }
    }
  }
  idl(MediaStreamTrack, 'MediaStreamTrack');
  handler(MediaStreamTrack, 'mute', trackCheck);
  handler(MediaStreamTrack, 'unmute', trackCheck);
  handler(MediaStreamTrack, 'ended', trackCheck);

  // New settings for `st` under `norm` (null: keep the current ones); throws OverconstrainedError.
  const reconfigure = (st, norm) => {
    const source = st.source;
    if (typeof source.select === 'function') return source.select(norm, st);
    // A source without selectable settings: only constraints on what it has can hold.
    const caps = source.capabilities();
    const check = (set, advanced) => {
      for (const name of O.keys(set)) {
        const c = set[name];
        if (!hasHard(c)) continue;
        if (!(name in caps)) {
          if (advanced) return false;
          if (SUPPORTED.includes(name)) throw overconstrained(name);
        }
      }
      return true;
    };
    check(norm.set, false);
    return null;
  };

  const createTrack = (Cls, kind, source, init) => {
    const t = new Cls(token, init.extra);
    const st = {
      object: t, cls: Cls, kind, id: uuid(), label: init.label !== undefined ? init.label : source.label || '',
      enabled: init.enabled !== undefined ? init.enabled : true, muted: !!init.muted,
      readyState: 'live', source, constraints: init.constraints || {}, settings: init.settings || source.settings(),
      contentHint: init.contentHint || '', statsObject: null, activeMs: 0, since: null, extra: init.extra,
    };
    tracks.set(t, st);
    if (init.ended) {
      st.readyState = 'ended';
    } else {
      source.tracks.add(st);
      resumeStats(st);
    }
    return t;
  };
  const makeTrack = (kind, source, init) => {
    if (!source.tracks) source.tracks = new Set();
    return createTrack(MediaStreamTrack, kind, source, init || {});
  };
  const trackSource = (track) => trackOf(track, 'trackSource').source;

  class CanvasCaptureMediaStreamTrack extends MediaStreamTrack {
    constructor(t, extra) {
      if (t !== token) throw illegal('CanvasCaptureMediaStreamTrack');
      super(t);
      canvasOf.set(this, extra);
    }
    get canvas() {
      if (!canvasOf.has(this)) throw invocation('canvas', 'CanvasCaptureMediaStreamTrack');
      return canvasOf.get(this).canvas;
    }
    requestFrame() {
      if (!canvasOf.has(this)) throw invocation('requestFrame', 'CanvasCaptureMediaStreamTrack');
      const st = tracks.get(this);
      if (st.readyState === 'live') canvasOf.get(this).requested++;
    }
  }
  const canvasOf = new WeakMap();
  idl(CanvasCaptureMediaStreamTrack, 'CanvasCaptureMediaStreamTrack');

  // ---- MediaStream ----------------------------------------------------------------------------
  const streamCheck = (s) => {
    if (!streams.has(s)) throw new TypeErrorC('Illegal invocation');
  };
  const streamOf = (s, what) => {
    const st = streams.get(s);
    if (!st) throw invocation(what, 'MediaStream');
    return st;
  };
  const addUnique = (st, track) => {
    if (!st.tracks.includes(track)) st.tracks.push(track);
  };
  const uaAddTrack = (stream, track) => {
    const st = streams.get(stream);
    if (st.tracks.includes(track)) return;
    st.tracks.push(track);
    fire(stream, new MediaStreamTrackEvent('addtrack', { track }));
  };
  const uaRemoveTrack = (stream, track) => {
    const st = streams.get(stream);
    const i = st.tracks.indexOf(track);
    if (i < 0) return;
    st.tracks.splice(i, 1);
    fire(stream, new MediaStreamTrackEvent('removetrack', { track }));
  };
  class MediaStream extends EventTargetC {
    constructor(arg = undefined) {
      super();
      const st = { id: uuid(), tracks: [] };
      streams.set(this, st);
      if (arg !== undefined) {
        if (isStream(arg)) {
          for (const t of streams.get(arg).tracks) addUnique(st, t);
        } else {
          if (!isObj(arg) || typeof arg[Symbol.iterator] !== 'function') {
            throw new TypeErrorC("Failed to construct 'MediaStream': The provided value cannot be converted to a sequence.");
          }
          for (const t of arg) {
            if (!isTrack(t)) throw new TypeErrorC("Failed to construct 'MediaStream': Failed to convert value to 'MediaStreamTrack'.");
            addUnique(st, t);
          }
        }
      }
    }
    get id() { return streamOf(this, 'id').id; }
    get active() { return streamOf(this, 'active').tracks.some((t) => tracks.get(t).readyState === 'live'); }
    getAudioTracks() { return streamOf(this, 'getAudioTracks').tracks.filter((t) => tracks.get(t).kind === 'audio'); }
    getVideoTracks() { return streamOf(this, 'getVideoTracks').tracks.filter((t) => tracks.get(t).kind === 'video'); }
    getTracks() { return streamOf(this, 'getTracks').tracks.slice(); }
    getTrackById(id) {
      const st = streamOf(this, 'getTrackById');
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'getTrackById' on 'MediaStream': 1 argument required, but only 0 present.");
      const s = String(id);
      return st.tracks.find((t) => tracks.get(t).id === s) || null;
    }
    addTrack(track) {
      const st = streamOf(this, 'addTrack');
      if (!isTrack(track)) throw new TypeErrorC("Failed to execute 'addTrack' on 'MediaStream': parameter 1 is not of type 'MediaStreamTrack'.");
      addUnique(st, track);
    }
    removeTrack(track) {
      const st = streamOf(this, 'removeTrack');
      if (!isTrack(track)) throw new TypeErrorC("Failed to execute 'removeTrack' on 'MediaStream': parameter 1 is not of type 'MediaStreamTrack'.");
      const i = st.tracks.indexOf(track);
      if (i >= 0) st.tracks.splice(i, 1);
    }
    clone() {
      const st = streamOf(this, 'clone');
      const c = new MediaStream();
      const cs = streams.get(c);
      for (const t of st.tracks) cs.tracks.push(t.clone());
      return c;
    }
  }
  idl(MediaStream, 'MediaStream');
  handler(MediaStream, 'addtrack', streamCheck);
  handler(MediaStream, 'removetrack', streamCheck);

  // ---- Sources: canvas, media element, fake devices -------------------------------------------
  const blackFrame = (w, h) => {
    const c = new OffscreenCanvas(MathC.max(1, w), MathC.max(1, h));
    const ctx = c.getContext('2d');
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, c.width, c.height);
    return resolve(c.transferToImageBitmap());
  };
  const canvasSource = (canvas, frameRate) => {
    const extra = { canvas, requested: 0 };
    const source = {
      kind: 'video', label: '', tracks: new Set(), extra,
      capabilities: () => ({}),
      settings() {
        const s = { width: canvas.width, height: canvas.height, aspectRatio: canvas.height ? canvas.width / canvas.height : 0 };
        if (frameRate !== undefined) s.frameRate = frameRate;
        return s;
      },
      frameRate: frameRate === undefined ? NATIVE_FPS : frameRate,
      // The canvas as it is now (a snapshot: what drawing into it did so far).
      pullFrame: () => {
        if (!canvas.width || !canvas.height) return null;
        return createImageBitmap(canvas);
      },
    };
    return source;
  };

  const elementSource = (el, kind) => {
    const crossOrigin = () => {
      try {
        const src = el.currentSrc;
        if (!src || src.startsWith('blob:') || src.startsWith('data:')) return false;
        return new URL(src, location.href).origin !== location.origin && !el.crossOrigin && el.crossOrigin !== '';
      } catch (e) {
        return false;
      }
    };
    const source = {
      kind, label: '', tracks: new Set(), element: el, crossOrigin: crossOrigin(),
      capabilities: () => ({}),
      settings() {
        if (kind === 'video') {
          const w = el.videoWidth;
          const h = el.videoHeight;
          return { width: w, height: h, aspectRatio: h ? w / h : 0 };
        }
        return {};
      },
      frameRate: NATIVE_FPS,
    };
    if (kind === 'video') {
      source.pullFrame = () => {
        const w = el.videoWidth;
        const h = el.videoHeight;
        if (!w || !h || el.readyState < 2) return null;
        if (source.crossOrigin) return blackFrame(w, h);
        return createImageBitmap(el);
      };
    } else {
      // An element's audio is not decoded on its own (nothing plays it): no samples ever arrive, so a track of
      // it is `muted` (a live track whose source delivers nothing, per spec) and never unmutes.
      source.pullAudio = () => null;
      source.noSamples = true;
    }
    return source;
  };

  const captureCanvas = (canvas, frameRate) => {
    if (frameRate !== undefined) {
      const f = +frameRate;
      if (f !== f || f < 0) throw new DOMExceptionC("Failed to execute 'captureStream' on 'HTMLCanvasElement': Given frame rate is not supported.", 'NotSupportedError');
    }
    const source = canvasSource(canvas, frameRate === undefined ? undefined : +frameRate);
    const track = createTrack(CanvasCaptureMediaStreamTrack, 'video', source, { extra: source.extra });
    const stream = new MediaStream();
    streams.get(stream).tracks.push(track);
    return stream;
  };

  const captureElement = (el) => {
    const stream = new MediaStream();
    const live = { audio: null, video: null };
    const drop = () => {
      for (const kind of ['audio', 'video']) {
        const t = live[kind];
        if (!t) continue;
        live[kind] = null;
        const st = tracks.get(t);
        // The track ends first, the stream drops it in a later task.
        endTrack(st, false);
        task(() => uaRemoveTrack(stream, t));
      }
    };
    const sync = () => {
      const kinds = native.elementKinds(el);
      if (!(kinds & 4) || el.ended) return;
      for (const [kind, bit] of [['video', 2], ['audio', 1]]) {
        if (!(kinds & bit) || live[kind]) continue;
        const source = elementSource(el, kind);
        const track = createTrack(MediaStreamTrack, kind, source, { muted: source.crossOrigin || !!source.noSamples });
        live[kind] = track;
        uaAddTrack(stream, track);
      }
    };
    for (const type of ['loadedmetadata', 'loadeddata', 'playing', 'play', 'seeked']) el.addEventListener(type, sync);
    el.addEventListener('ended', drop);
    el.addEventListener('emptied', drop);
    el.addEventListener('error', drop);
    sync();
    return stream;
  };

  // Fake camera: a moving test pattern with a frame counter, drawn per request from the track's
  // own timeline (frame index = elapsed time x native rate).
  const fakeCameraSource = (d, cand, r) => {
    let started = null;
    const source = {
      kind: 'video', label: d.label, tracks: new Set(), device: d, native: cand,
      frameRate: NATIVE_FPS,
      capabilities: () => videoCapabilities(d),
      settings: () => videoSettingsOf(cand, r),
      select(norm, st) {
        const cands = videoCandidates([d]);
        const { cand: c2, r: r2 } = selectSettings('video', cands, norm);
        return videoSettingsOf(c2, r2);
      },
      pullFrame(settings) {
        const w = (settings && settings.width) || cand.format[0];
        const h = (settings && settings.height) || cand.format[1];
        if (started === null) started = perfNow();
        const index = MathC.floor(((perfNow() - started) / 1000) * NATIVE_FPS);
        const c = new OffscreenCanvas(w, h);
        const ctx = c.getContext('2d');
        const bars = ['#c0c0c0', '#c0c000', '#00c0c0', '#00c000', '#c000c0', '#c00000', '#0000c0'];
        const bw = w / bars.length;
        for (let i = 0; i < bars.length; i++) {
          ctx.fillStyle = bars[i];
          ctx.fillRect(i * bw, 0, bw + 1, h * 0.75);
        }
        ctx.fillStyle = '#101010';
        ctx.fillRect(0, h * 0.75, w, h * 0.25);
        const size = MathC.max(2, MathC.min(w, h) / 8);
        const t = (index % 100) / 100;
        const x = (w - size) * (t < 0.5 ? t * 2 : 2 - t * 2);
        ctx.fillStyle = '#ffffff';
        ctx.fillRect(x, h * 0.75 - size, size, size);
        ctx.fillStyle = '#ffffff';
        ctx.font = `${MathC.max(6, MathC.round(h / 10))}px sans-serif`;
        ctx.fillText(String(index), 4, h - 4);
        return resolve(c.transferToImageBitmap());
      },
      stop() {
        started = null;
      },
    };
    return source;
  };

  // Fake microphone: a 440 Hz beep for the first 100 ms of every second, silence in between.
  const fakeMicSource = (d, cand, r) => {
    let position = 0;
    const source = {
      kind: 'audio', label: d.label, tracks: new Set(), device: d, native: cand, latencyMs: 10,
      capabilities: () => audioCapabilities(d),
      settings: () => audioSettingsOf(cand, r),
      select(norm) {
        const { cand: c2, r: r2 } = selectSettings('audio', [{ device: d }], norm);
        return audioSettingsOf(c2, r2);
      },
      pullAudio(count) {
        const channels = r.settings.channelCount;
        const data = new Float32Array(count);
        for (let i = 0; i < count; i++) {
          const n = position + i;
          const within = n % AUDIO_RATE;
          data[i] = within < AUDIO_RATE / 10 ? 0.5 * MathC.sin((2 * MathC.PI * 440 * n) / AUDIO_RATE) : 0;
        }
        position += count;
        const out = [];
        for (let c = 0; c < channels; c++) out.push(c === 0 ? data : data.slice());
        return out;
      },
    };
    return source;
  };

  // ---- Device enumeration and getUserMedia() --------------------------------------------------
  const exposed = { audioinput: false, videoinput: false, audiooutput: false };
  class MediaDeviceInfo {
    constructor(t, info) {
      if (t !== token) throw illegal('MediaDeviceInfo');
      infoOf.set(this, info);
    }
    get deviceId() { return infoFor(this, 'deviceId').deviceId; }
    get kind() { return infoFor(this, 'kind').kind; }
    get label() { return infoFor(this, 'label').label; }
    get groupId() { return infoFor(this, 'groupId').groupId; }
    toJSON() {
      const i = infoFor(this, 'toJSON');
      return { deviceId: i.deviceId, kind: i.kind, label: i.label, groupId: i.groupId };
    }
  }
  const infoOf = new WeakMap();
  const infoFor = (o, what) => {
    const i = infoOf.get(o);
    if (!i) throw invocation(what, 'MediaDeviceInfo');
    return i;
  };
  idl(MediaDeviceInfo, 'MediaDeviceInfo');
  class InputDeviceInfo extends MediaDeviceInfo {
    getCapabilities() {
      const i = infoFor(this, 'getCapabilities');
      if (!i.device || !i.deviceId) return {};
      return i.kind === 'videoinput' ? videoCapabilities(i.device) : audioCapabilities(i.device);
    }
  }
  idl(InputDeviceInfo, 'InputDeviceInfo');

  const enumerate = () => {
    if (!native.fakeDevices()) return [];
    const list = devices_();
    const out = [];
    const seenKind = {};
    for (const d of list) {
      if (exposed[d.kind]) {
        out.push(new (d.kind === 'audiooutput' ? MediaDeviceInfo : InputDeviceInfo)(token, { deviceId: deviceId(d), kind: d.kind, label: d.label, groupId: groupId(d), device: d }));
      } else if (!seenKind[d.kind]) {
        // Before a successful getUserMedia() for the kind: one entry without identifying data.
        seenKind[d.kind] = true;
        out.push(new (d.kind === 'audiooutput' ? MediaDeviceInfo : InputDeviceInfo)(token, { deviceId: '', kind: d.kind, label: '', groupId: '', device: d }));
      }
    }
    return out;
  };
  const supportedConstraints = () => {
    const o = {};
    for (const name of SUPPORTED) o[name] = true;
    return o;
  };
  const PTZ = ['pan', 'tilt', 'zoom'];
  const kindDict = (v, kind, member) => {
    if (v === undefined || v === null || v === false) return null;
    if (v === true) return { set: {}, advanced: [], raw: {} };
    if (!isObj(v)) {
      // Web IDL: (boolean or MediaTrackConstraints): any other primitive is a boolean.
      return v ? { set: {}, advanced: [], raw: {} } : null;
    }
    const norm = parseConstraints(v, kind);
    norm.raw = v;
    for (const name of PTZ) {
      const c = norm.set[name];
      if (kind === 'video' && c && hasHard(c)) throw new TypeErrorC('Pan, tilt, zoom permission is required for exact, min and max constraints.');
      for (const a of norm.advanced) {
        if (kind === 'video' && a[name] && hasHard(a[name])) throw new TypeErrorC('Pan, tilt, zoom permission is required for advanced constraints.');
      }
    }
    return norm;
  };
  const getUserMedia = (constraints) => {
    if (constraints === undefined) constraints = {};
    if (!isObj(constraints)) throw new TypeErrorC("Failed to execute 'getUserMedia' on 'MediaDevices': The provided value is not of type 'MediaStreamConstraints'.");
    const audio = kindDict(constraints.audio, 'audio');
    const video = kindDict(constraints.video, 'video');
    if (!audio && !video) throw new TypeErrorC("Failed to execute 'getUserMedia' on 'MediaDevices': At least one of audio and video must be requested");
    // Permissions Policy (`microphone` for audio, `camera` for video): refused before any device lookup.
    const microphone = audio ? native.policy('microphone') : true;
    const camera = video ? native.policy('camera') : true;
    if (!microphone || !camera) throw new DOMExceptionC('Permission denied', 'NotAllowedError');
    if (!native.fakeDevices()) throw new DOMExceptionC('Requested device not found', 'NotFoundError');
    const list = devices_();
    const picked = [];
    if (video) {
      const devs = list.filter((d) => d.kind === 'videoinput');
      const { cand, r } = selectSettings('video', videoCandidates(devs), video);
      picked.push(['video', cand, r, video]);
    }
    if (audio) {
      const devs = list.filter((d) => d.kind === 'audioinput').map((device) => ({ device }));
      const { cand, r } = selectSettings('audio', devs, audio);
      picked.push(['audio', cand, r, audio]);
    }
    const stream = new MediaStream();
    for (const [kind, cand, r, norm] of picked) {
      const d = cand.device;
      const source = kind === 'video' ? fakeCameraSource(d, cand, r) : fakeMicSource(d, cand, r);
      const track = createTrack(MediaStreamTrack, kind, source, { constraints: copyConstraints(norm.raw) });
      streams.get(stream).tracks.push(track);
      if (kind === 'video') exposed.videoinput = true;
      else exposed.audioinput = exposed.audiooutput = true;
    }
    return stream;
  };

  // ---- ImageCapture ---------------------------------------------------------------------------
  const captures = new WeakMap();
  const capturerOf = (c, what) => {
    const r = captures.get(c);
    if (!r) throw invocation(what, 'ImageCapture');
    return r;
  };
  const photoSupport = (st) => st.source && st.source.device && st.source.kind === 'video';
  class ImageCapture {
    constructor(track) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'ImageCapture': 1 argument required, but only 0 present.");
      if (!isTrack(track)) throw new TypeErrorC("Failed to construct 'ImageCapture': parameter 1 is not of type 'MediaStreamTrack'.");
      const st = tracks.get(track);
      if (st.kind !== 'video') throw new DOMExceptionC("Failed to construct 'ImageCapture': The provided track is not a video track.", 'NotSupportedError');
      captures.set(this, { track });
    }
    get track() { return capturerOf(this, 'track').track; }
    grabFrame() {
      try {
        const st = tracks.get(capturerOf(this, 'grabFrame').track);
        if (st.readyState !== 'live') throw new DOMExceptionC('The associated Track is in an invalid state.', 'InvalidStateError');
        if (!st.enabled || st.muted) throw new DOMExceptionC('The associated Track is in an invalid state.', 'InvalidStateError');
        const frame = st.source.pullFrame ? st.source.pullFrame(st.settings) : null;
        if (!frame) throw new DOMExceptionC('Failed to grab a frame: the track has no frame yet.', 'UnknownError');
        return resolve(frame);
      } catch (e) {
        return reject(e);
      }
    }
    getPhotoCapabilities() {
      try {
        const st = tracks.get(capturerOf(this, 'getPhotoCapabilities').track);
        if (st.readyState !== 'live') throw new DOMExceptionC('The associated Track is in an invalid state.', 'InvalidStateError');
        if (!photoSupport(st)) throw new DOMExceptionC('Photo capabilities are not available for this track.', 'NotSupportedError');
        return resolve({
          redEyeReduction: 'never',
          imageHeight: { min: 1, max: 2160, current: st.settings.height, step: 1 },
          imageWidth: { min: 1, max: 3840, current: st.settings.width, step: 1 },
          fillLightMode: ['off'],
        });
      } catch (e) {
        return reject(e);
      }
    }
    getPhotoSettings() {
      try {
        const st = tracks.get(capturerOf(this, 'getPhotoSettings').track);
        if (st.readyState !== 'live') throw new DOMExceptionC('The associated Track is in an invalid state.', 'InvalidStateError');
        if (!photoSupport(st)) throw new DOMExceptionC('Photo settings are not available for this track.', 'NotSupportedError');
        return resolve({ fillLightMode: 'off', imageHeight: st.settings.height, imageWidth: st.settings.width, redEyeReduction: false });
      } catch (e) {
        return reject(e);
      }
    }
    takePhoto(photoSettings = undefined) {
      try {
        const st = tracks.get(capturerOf(this, 'takePhoto').track);
        if (photoSettings !== undefined && photoSettings !== null && !isObj(photoSettings)) {
          throw new TypeErrorC("Failed to execute 'takePhoto' on 'ImageCapture': The provided value is not of type 'PhotoSettings'.");
        }
        if (st.readyState !== 'live') throw new DOMExceptionC('The associated Track is in an invalid state.', 'InvalidStateError');
        if (!photoSupport(st)) throw new DOMExceptionC('takePhoto() is not supported by this track.', 'NotSupportedError');
        const w = (photoSettings && photoSettings.imageWidth) || st.settings.width;
        const h = (photoSettings && photoSettings.imageHeight) || st.settings.height;
        return st.source.pullFrame({ width: w, height: h }).then((bitmap) => {
          const c = new OffscreenCanvas(bitmap.width, bitmap.height);
          c.getContext('2d').drawImage(bitmap, 0, 0);
          return c.convertToBlob({ type: 'image/png' });
        });
      } catch (e) {
        return reject(e);
      }
    }
  }
  idl(ImageCapture, 'ImageCapture');

  // ---- <video srcObject=MediaStream>: the element's frames -------------------------------------
  // While such an element plays (and until its first frame, which gives it its data), one frame of
  // the stream's first live video track is pulled per rendering update and handed to the element
  // (`native.setStreamFrame`): what screenshots, drawImage, createImageBitmap, VideoFrame and
  // requestVideoFrameCallback show. A muted track delivers none; a disabled one delivers black.
  const attached = new WeakMap(); // element -> { stream, have, busy, dead }
  const streamFrame = (stream) => {
    for (const t of streams.get(stream).tracks) {
      const st = tracks.get(t);
      if (st.kind !== 'video' || st.readyState !== 'live' || st.muted || !st.source.pullFrame) continue;
      if (!st.enabled) return blackFrame(st.settings.width || 1, st.settings.height || 1);
      return st.source.pullFrame(st.settings);
    }
    return null;
  };
  const toRGBA = (bitmap) => {
    const w = bitmap.width;
    const h = bitmap.height;
    if (!w || !h) return null;
    const c = new OffscreenCanvas(w, h);
    const ctx = c.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(bitmap, 0, 0);
    return ctx.getImageData(0, 0, w, h);
  };
  const pumpFrames = (el, rec) => {
    if (rec.busy || rec.dead) return;
    rec.busy = true;
    const win = (el.ownerDocument && el.ownerDocument.defaultView) || globalThis;
    const stop = () => {
      rec.busy = false;
    };
    const step = () => {
      let stream = null;
      try {
        stream = el.srcObject;
      } catch (e) {
        /* detached realm */
      }
      if (rec.dead || stream !== rec.stream || !native.active()) return stop();
      // Once it has shown a frame a paused element keeps it; play() resumes the pulls.
      if (rec.have && el.paused) return stop();
      let p = null;
      try {
        p = streamFrame(stream);
      } catch (e) {
        p = null;
      }
      if (!p) return win.requestAnimationFrame(step);
      p.then(
        (bitmap) => {
          try {
            const data = bitmap && toRGBA(bitmap);
            if (bitmap && bitmap.close) bitmap.close();
            if (data && !rec.dead && el.srcObject === rec.stream && native.setStreamFrame(el, data.width, data.height, data.data)) rec.have = true;
          } catch (e) {
            /* no frame this time */
          }
          win.requestAnimationFrame(step);
        },
        () => win.requestAnimationFrame(step),
      );
    };
    win.requestAnimationFrame(step);
  };
  const resumeFrames = (e) => {
    const rec = attached.get(e.currentTarget);
    if (rec && !rec.busy) pumpFrames(e.currentTarget, rec);
  };
  const attachElement = (el, stream) => {
    if (!isStream(stream)) return;
    const old = attached.get(el);
    if (old) old.dead = true;
    else {
      el.addEventListener('play', resumeFrames);
      el.addEventListener('playing', resumeFrames);
    }
    const rec = { stream, have: false, busy: false, dead: false };
    attached.set(el, rec);
    pumpFrames(el, rec);
  };

  // Info for HTMLMediaElement.srcObject (media.rs): [audio tracks, video tracks, width, height] of
  // a MediaStream's live tracks, or null for another object.
  const streamInfo = (obj) => {
    if (!isStream(obj)) return null;
    let audio = 0;
    let video = 0;
    let w = 0;
    let h = 0;
    for (const t of streams.get(obj).tracks) {
      const st = tracks.get(t);
      if (st.readyState !== 'live') continue;
      if (st.kind === 'audio') audio++;
      else {
        video++;
        if (!w) {
          w = st.settings.width || 0;
          h = st.settings.height || 0;
        }
      }
    }
    return [audio, video, w, h];
  };

  return {
    MediaStream, MediaStreamTrack, MediaStreamTrackEvent, CanvasCaptureMediaStreamTrack, OverconstrainedError,
    MediaStreamTrackVideoStats, MediaStreamTrackAudioStats,
    MediaDeviceInfo, InputDeviceInfo, ImageCapture,
    captureCanvas, captureElement, getUserMedia, enumerate, supportedConstraints, streamInfo, attachElement,
    makeTrack, trackSource,
  };
});
