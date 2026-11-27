// Media Capabilities for one realm, window or worker (https://w3c.github.io/media-capabilities/):
// MediaCapabilities.decodingInfo() and encodingInfo(). The configuration is validated as the spec's
// "valid media configuration" steps say; the answer comes from what this build really decodes
// (`native.canPlay(type)`: the element's canPlayType table, `native.mseSupported(type)`: the
// MediaSource byte stream formats). There is no encoder behind a `record` or `webrtc` target (no
// MediaRecorder, no WebRTC) and no hardware decoder: encodingInfo() is unsupported. A
// keySystemConfiguration for Clear Key (`org.w3.clearkey`, eme.js) is supported when the media is and
// the key system grants access (keySystemAccess), any other key system is unsupported; powerEfficient
// is false (software decoding) and `smooth` is true exactly when the configuration is supported (frames are never dropped).
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const TypeErrorC = TypeError;
  const PromiseC = Promise;
  const illegal = () => new TypeErrorC("Failed to construct 'MediaCapabilities': Illegal constructor");
  const isObj = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  const instances = new WeakSet();

  const DECODING = ['file', 'media-source', 'webrtc'];
  const ENCODING = ['record', 'webrtc'];
  const HDR = ['smpteSt2086', 'smpteSt2094-10', 'smpteSt2094-40'];
  const GAMUT = ['srgb', 'p3', 'rec2020'];
  const TRANSFER = ['srgb', 'pq', 'hlg'];
  const REQUIREMENT = ['required', 'optional', 'not-allowed'];

  const VIDEO_CODECS = /^(avc[1-4]|hev1|hvc1|vp8$|vp9$|vp0?9|av01|theora$|mp4v|dvh[e1]|dva[v1]|vc-1|hevc$|h264$|h265$|av1$)/i;
  const AUDIO_CODECS = /^(mp4a|opus$|vorbis$|flac$|mp3$|ac-3|ec-3|alac$|pcm|ulaw$|alaw$|speex$|1$|g722|pcmu|pcma|isac|ilbc|telephone-event)/i;
  // Containers whose type alone names the one codec inside.
  const IMPLIED = new Set(['audio/mpeg', 'audio/mp3', 'audio/x-mp3', 'audio/aac', 'audio/flac', 'audio/wav', 'audio/x-wav', 'audio/wave']);

  // The MIME type and its codecs of a contentType, or a TypeError. `kind`: 'video' | 'audio'.
  const parseContentType = (value, kind, what, webrtc) => {
    const text = String(value);
    const bad = (why) => new TypeErrorC(`${what}: ${why}`);
    const parts = text.split(';');
    const essence = parts[0].trim().toLowerCase();
    const m = /^([!#$%&'*+.^_`|~0-9a-z-]+)\/([!#$%&'*+.^_`|~0-9a-z-]+)$/.exec(essence);
    if (!m) throw bad(`'${text}' is not a valid media MIME type.`);
    const params = parts.slice(1);
    // A trailing or doubled ';' leaves an empty parameter: not a valid MIME type string.
    if (params.some((p) => p.trim() === '')) throw bad(`'${text}' is not a valid media MIME type.`);
    if (params.length > 1) throw bad(`'${text}' has more than one parameter.`);
    let codecs = null;
    if (params.length === 1) {
      const eq = params[0].indexOf('=');
      const name = (eq < 0 ? params[0] : params[0].slice(0, eq)).trim().toLowerCase();
      if (eq < 0 || name !== 'codecs') throw bad(`'${text}' has a parameter that is not codecs.`);
      let v = params[0].slice(eq + 1).trim();
      if (v.length >= 2 && v[0] === '"' && v[v.length - 1] === '"') v = v.slice(1, -1);
      codecs = v.split(',').map((c) => c.trim()).filter((c) => c !== '');
    }
    const type = m[1];
    if (type !== kind && !(type === 'application' && m[2] === 'ogg')) throw bad(`'${text}' is not a ${kind} MIME type.`);
    if (codecs === null) {
      // A WebRTC type is the codec itself (video/VP8, audio/opus).
      if (!webrtc && !IMPLIED.has(essence)) throw bad(`'${text}' does not imply a single codec and has no codecs parameter.`);
      return { essence, codecs: [] };
    }
    const video = codecs.filter((c) => VIDEO_CODECS.test(c)).length;
    const audio = codecs.filter((c) => AUDIO_CODECS.test(c) && !VIDEO_CODECS.test(c)).length;
    if (kind === 'video') {
      if (codecs.length !== 1 || video !== 1) throw bad(`'${text}' must name exactly one video codec.`);
    } else if (codecs.length !== 1 || audio !== 1) {
      throw bad(`'${text}' must name exactly one audio codec.`);
    }
    return { essence, codecs };
  };

  const ulong = (v, name, what) => {
    const n = +v;
    if (!(n - n === 0)) throw new TypeErrorC(`${what}: The provided value for '${name}' is non-finite.`);
    return n;
  };
  const required = (d, name, what, section) => {
    if (d[name] === undefined) throw new TypeErrorC(`${what}: Failed to read the '${name}' property from '${section}': Required member is undefined.`);
    return d[name];
  };
  const enumOf = (v, list, name, what) => {
    const s = String(v);
    if (!list.includes(s)) throw new TypeErrorC(`${what}: The provided value '${s}' is not a valid enum value of type ${name}.`);
    return s;
  };

  // The checked configuration: {video, audio, keys, type} with the parsed content types.
  const validate = (config, types, what) => {
    if (!isObj(config)) throw new TypeErrorC(`${what}: The provided value is not of type 'MediaConfiguration'.`);
    const type = enumOf(required(config, 'type', what, 'MediaConfiguration'), types, 'MediaDecodingType', what);
    const webrtc = type === 'webrtc';
    const out = { type, video: null, audio: null, keys: null, parsed: {} };
    if (config.video === undefined && config.audio === undefined) throw new TypeErrorC(`${what}: The configuration must have an audio or video member.`);
    if (config.video !== undefined) {
      const v = config.video;
      if (!isObj(v)) throw new TypeErrorC(`${what}: The provided value is not of type 'VideoConfiguration'.`);
      const contentType = String(required(v, 'contentType', what, 'VideoConfiguration'));
      const width = ulong(required(v, 'width', what, 'VideoConfiguration'), 'width', what);
      const height = ulong(required(v, 'height', what, 'VideoConfiguration'), 'height', what);
      const bitrate = ulong(required(v, 'bitrate', what, 'VideoConfiguration'), 'bitrate', what);
      const framerate = ulong(required(v, 'framerate', what, 'VideoConfiguration'), 'framerate', what);
      if (framerate <= 0) throw new TypeErrorC(`${what}: The provided framerate must be greater than zero.`);
      if (v.hdrMetadataType !== undefined) enumOf(v.hdrMetadataType, HDR, 'HdrMetadataType', what);
      if (v.colorGamut !== undefined) enumOf(v.colorGamut, GAMUT, 'ColorGamut', what);
      if (v.transferFunction !== undefined) enumOf(v.transferFunction, TRANSFER, 'TransferFunction', what);
      if (v.scalabilityMode !== undefined) String(v.scalabilityMode);
      out.parsed.video = parseContentType(contentType, 'video', what, webrtc);
      out.video = { contentType, width: width >>> 0, height: height >>> 0, bitrate, framerate, v };
    }
    if (config.audio !== undefined) {
      const a = config.audio;
      if (!isObj(a)) throw new TypeErrorC(`${what}: The provided value is not of type 'AudioConfiguration'.`);
      const contentType = String(required(a, 'contentType', what, 'AudioConfiguration'));
      if (a.channels !== undefined) String(a.channels);
      if (a.bitrate !== undefined) ulong(a.bitrate, 'bitrate', what);
      if (a.samplerate !== undefined) ulong(a.samplerate, 'samplerate', what);
      out.parsed.audio = parseContentType(contentType, 'audio', what, webrtc);
      out.audio = { contentType, a };
    }
    return out;
  };
  const keySystem = (config, checked, what) => {
    const k = config.keySystemConfiguration;
    if (k === undefined) return;
    if (!isObj(k)) throw new TypeErrorC(`${what}: The provided value is not of type 'MediaCapabilitiesKeySystemConfiguration'.`);
    if (checked.type === 'webrtc') throw new TypeErrorC(`${what}: A keySystemConfiguration is not allowed for type webrtc.`);
    const name = String(required(k, 'keySystem', what, 'MediaCapabilitiesKeySystemConfiguration'));
    for (const key of ['distinctiveIdentifier', 'persistentState']) if (k[key] !== undefined) enumOf(k[key], REQUIREMENT, 'MediaKeysRequirement', what);
    if (k.sessionTypes !== undefined) {
      if (!isObj(k.sessionTypes) || typeof k.sessionTypes[Symbol.iterator] !== 'function') throw new TypeErrorC(`${what}: sessionTypes is not a sequence.`);
      Array.from(k.sessionTypes, String);
    }
    for (const track of ['audio', 'video']) {
      const t = k[track];
      if (t === undefined) continue;
      if (!isObj(t)) throw new TypeErrorC(`${what}: The provided value is not of type 'KeySystemTrackConfiguration'.`);
      // The track configuration needs the media configuration to have that track.
      if (!checked[track]) throw new TypeErrorC(`${what}: keySystemConfiguration.${track} needs a ${track} configuration.`);
      if (t.robustness !== undefined) String(t.robustness);
      if (t.encryptionScheme !== undefined && t.encryptionScheme !== null) String(t.encryptionScheme);
    }
    if (k.initDataType !== undefined) String(k.initDataType);
    checked.keys = name;
  };

  const MAX_DIMENSION = 8192;
  const CLEAR_KEY = 'org.w3.clearkey';
  // decodingInfo() with a keySystemConfiguration: the media must be decodable, and the key system must
  // grant access for the MediaKeySystemConfiguration the media configuration implies (the spec's
  // "create a MediaKeySystemConfiguration"): that access is the result's keySystemAccess.
  const keyedInfo = (configuration, checked, supported) => {
    const none = () => ({ supported: false, smooth: false, powerEfficient: false, keySystemAccess: null, configuration: copy(configuration) });
    if (!supported || checked.keys !== CLEAR_KEY || typeof native.requestAccess !== 'function') return PromiseC.resolve(none());
    const k = configuration.keySystemConfiguration;
    const caps = (kind, contentType) => {
      const t = k[kind] || {};
      return [{ contentType, robustness: t.robustness === undefined ? '' : String(t.robustness), encryptionScheme: t.encryptionScheme === undefined ? null : t.encryptionScheme }];
    };
    const config = {
      initDataTypes: k.initDataType === undefined || k.initDataType === '' ? [] : [String(k.initDataType)],
      distinctiveIdentifier: k.distinctiveIdentifier === undefined ? 'optional' : String(k.distinctiveIdentifier),
      persistentState: k.persistentState === undefined ? 'optional' : String(k.persistentState),
    };
    if (k.sessionTypes !== undefined) config.sessionTypes = Array.from(k.sessionTypes, String);
    if (checked.audio) config.audioCapabilities = caps('audio', checked.audio.contentType);
    if (checked.video) config.videoCapabilities = caps('video', checked.video.contentType);
    let request;
    try {
      request = native.requestAccess(CLEAR_KEY, [config]);
    } catch (e) {
      request = undefined;
    }
    if (!request) return PromiseC.resolve(none());
    return request.then(
      (keySystemAccess) => {
        return { supported: true, smooth: true, powerEfficient: false, keySystemAccess, configuration: copy(configuration) };
      },
      () => none(),
    );
  };
  const decoding = (checked) => {
    if (checked.keys !== null && checked.keys !== CLEAR_KEY) return false;
    if (checked.type === 'webrtc') return false;
    const check = (p) => {
      const type = p.essence + (p.codecs.length ? `; codecs="${p.codecs.join(',')}"` : '');
      if (checked.type === 'media-source') return native.mseSupported(type);
      return native.canPlay(type) !== '';
    };
    for (const p of [checked.parsed.video, checked.parsed.audio]) if (p && !check(p)) return false;
    if (checked.video) {
      const { width, height, v } = checked.video;
      if (width === 0 || height === 0 || width > MAX_DIMENSION || height > MAX_DIMENSION) return false;
      // The decoders produce 8-bit SDR frames: HDR transfer functions and metadata are not decodable.
      if (v.hdrMetadataType !== undefined) return false;
      if (v.transferFunction !== undefined && String(v.transferFunction) !== 'srgb') return false;
    }
    return true;
  };
  const copy = (v) => {
    if (Array.isArray(v)) return v.map(copy);
    if (isObj(v) && typeof v !== 'function') {
      const o = {};
      for (const k of O.keys(v)) if (v[k] !== undefined) o[k] = copy(v[k]);
      return o;
    }
    return v;
  };

  class MediaCapabilities {
    constructor(t) {
      if (t !== token) throw illegal();
      instances.add(this);
    }
    decodingInfo(configuration) {
      const what = "Failed to execute 'decodingInfo' on 'MediaCapabilities'";
      try {
        if (!instances.has(this)) throw new TypeErrorC(`${what}: Illegal invocation`);
        if (arguments.length < 1) throw new TypeErrorC(`${what}: 1 argument required, but only 0 present.`);
        const checked = validate(configuration, DECODING, what);
        keySystem(configuration, checked, what);
        const supported = decoding(checked);
        // Pyro decodes in software and presents every decoded frame (it has no real-time deadline to drop frames
        // against), so a supported configuration is smooth by design: no invented pixel-rate threshold. Software
        // decoding is also why powerEfficient is false: no hardware decoder is ever used.
        const smooth = supported;
        if (checked.keys !== null) return keyedInfo(configuration, checked, supported);
        return PromiseC.resolve({ supported, smooth, powerEfficient: false, keySystemAccess: null, configuration: copy(configuration) });
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    encodingInfo(configuration) {
      const what = "Failed to execute 'encodingInfo' on 'MediaCapabilities'";
      try {
        if (!instances.has(this)) throw new TypeErrorC(`${what}: Illegal invocation`);
        if (arguments.length < 1) throw new TypeErrorC(`${what}: 1 argument required, but only 0 present.`);
        const checked = validate(configuration, ENCODING, what);
        if (checked.type === 'record' && checked.video) {
          const v = checked.video.v;
          for (const k of ['scalabilityMode', 'colorGamut', 'transferFunction', 'hdrMetadataType']) {
            if (v[k] !== undefined) throw new TypeErrorC(`${what}: ${k} is not allowed for type record.`);
          }
        }
        return PromiseC.resolve({ supported: false, smooth: false, powerEfficient: false, configuration: copy(configuration) });
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
  }
  for (const k of O.getOwnPropertyNames(MediaCapabilities.prototype)) {
    if (k === 'constructor') continue;
    const d = O.getOwnPropertyDescriptor(MediaCapabilities.prototype, k);
    d.enumerable = true;
    O.defineProperty(MediaCapabilities.prototype, k, d);
  }
  O.defineProperty(MediaCapabilities.prototype, Symbol.toStringTag, { value: 'MediaCapabilities', configurable: true });

  return { MediaCapabilities, mediaCapabilities: new MediaCapabilities(token) };
});
