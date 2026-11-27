// Encrypted Media Extensions with the Clear Key key system (`org.w3.clearkey`): requestMediaKeySystemAccess,
// MediaKeySystemAccess, MediaKeys, MediaKeySession (license request JSON, JWK sets, key statuses), the
// MediaKeyMessageEvent / MediaEncryptedEvent interfaces and `HTMLMediaElement.setMediaKeys`. The keys a
// session learns live in a native store (eme.rs) that the media element decrypts its Common Encryption
// samples with; nothing here decrypts. No DRM key system exists, and no persistent storage backs
// `persistent-license`, so Clear Key supports `temporary` sessions only (as Chrome's Clear Key does).
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object, A = Array, PromiseC = Promise, TypeErrorC = TypeError, DOMExceptionC = DOMException;
  const apply = Reflect.apply, define = O.defineProperty, get = O.getOwnPropertyDescriptor;
  const U8 = Uint8Array, EventC = Event, EventTargetC = EventTarget;
  const dispatch = (target, event) => apply(EventTarget.prototype.dispatchEvent, target, [event]);
  const abByteLength = get(ArrayBuffer.prototype, 'byteLength').get;
  const KEY_SYSTEM = 'org.w3.clearkey';
  const INIT_DATA_TYPES = ['keyids', 'cenc'];
  const SCHEMES = ['cenc', 'cbcs', 'cbcs-1-9'];
  const MAX_INIT_DATA = 65536, MAX_LICENSE = 65536, MAX_KEY_ID = 512;
  // The Common SystemID: the only pssh box Clear Key reads key IDs from.
  const COMMON = '1077efecc0b24d02ace33c1e52e2fb4b';

  const reject = e => PromiseC.reject(e);
  const task = f => native.postTask(f);
  const dom = (message, name) => new DOMExceptionC(message, name);
  const quotaExceeded = message => typeof QuotaExceededError === 'function' ? new QuotaExceededError(message) : dom(message, 'QuotaExceededError');
  const illegal = name => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);

  // ---- conversions --------------------------------------------------------------------------------
  // BufferSource (not shared): a copy of its bytes, or TypeError.
  function toBytes(value, what) {
    let buffer = value, offset = 0, length;
    if (ArrayBuffer.isView(value)) { buffer = value.buffer; offset = value.byteOffset; length = value.byteLength; }
    let size;
    try { size = apply(abByteLength, buffer, []); } catch { size = -1; }
    if (size < 0) throw new TypeErrorC(`${what}: The provided value is not of type 'BufferSource', or is a shared buffer.`);
    return new U8(buffer, offset, length ?? size).slice();
  }
  function hex(bytes) { let s = ''; for (const b of bytes) s += (b < 16 ? '0' : '') + b.toString(16); return s; }
  function b64uEncode(bytes) {
    let s = '';
    for (const b of bytes) s += String.fromCharCode(b);
    return btoa(s).replace(/=+$/, '').replace(/\+/g, '-').replace(/\//g, '_');
  }
  // base64url without padding, or null.
  function b64uDecode(text) {
    if (typeof text !== 'string' || !/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) return null;
    let s = text.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    const bin = atob(s), out = new U8(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');
  function dictionary(v, what) {
    if (v === undefined || v === null) return {};
    if (!isObject(v)) throw new TypeErrorC(`${what}: The provided value is not of type dictionary.`);
    return v;
  }
  function sequence(v, convert, what) {
    if (!isObject(v) || typeof v[Symbol.iterator] !== 'function') throw new TypeErrorC(`${what}: The provided value cannot be converted to a sequence.`);
    const out = [];
    for (const item of v) out.push(convert(item));
    return out;
  }
  const str = v => { if (typeof v === 'symbol') throw new TypeErrorC('Cannot convert a Symbol value to a string'); return `${v}`; };
  function requirement(v, what) {
    if (v === undefined) return 'optional';
    const s = str(v);
    if (s !== 'required' && s !== 'optional' && s !== 'not-allowed') throw new TypeErrorC(`${what}: '${s}' is not a valid MediaKeysRequirement value.`);
    return s;
  }

  // ---- requestMediaKeySystemAccess -----------------------------------------------------------------
  function capabilities(list, kind) {
    const out = [];
    for (const cap of list) {
      const type = cap.contentType;
      const kinds = native.kinds(type);
      // An audio entry names audio codecs alone; a video entry carries a video codec.
      if (kind === 'audio' ? kinds !== 1 : !(kinds & 2)) continue;
      if (cap.robustness !== '') continue;
      if (cap.encryptionScheme !== null && !SCHEMES.includes(cap.encryptionScheme)) continue;
      out.push({ contentType: type, encryptionScheme: cap.encryptionScheme, robustness: '' });
    }
    return out;
  }
  // The spec's "get supported configuration": the configuration Clear Key supports of `candidate`, or null.
  function supported(candidate) {
    const config = { label: candidate.label, initDataTypes: [], audioCapabilities: [], videoCapabilities: [],
      distinctiveIdentifier: 'not-allowed', persistentState: 'not-allowed', sessionTypes: ['temporary'] };
    if (candidate.initDataTypes.length) {
      config.initDataTypes = candidate.initDataTypes.filter(t => INIT_DATA_TYPES.includes(t));
      if (!config.initDataTypes.length) return null;
    }
    // No distinctive identifier and no persistent state: a `required` request cannot be met.
    if (candidate.distinctiveIdentifier === 'required' || candidate.persistentState === 'required') return null;
    if (candidate.sessionTypes !== undefined && candidate.sessionTypes.some(t => t !== 'temporary')) return null;
    if (!candidate.audioCapabilities.length && !candidate.videoCapabilities.length) return null;
    if (candidate.audioCapabilities.length) {
      config.audioCapabilities = capabilities(candidate.audioCapabilities, 'audio');
      if (!config.audioCapabilities.length) return null;
    }
    if (candidate.videoCapabilities.length) {
      config.videoCapabilities = capabilities(candidate.videoCapabilities, 'video');
      if (!config.videoCapabilities.length) return null;
    }
    return config;
  }
  function toCapability(v) {
    const d = dictionary(v, 'MediaKeySystemMediaCapability');
    const scheme = d.encryptionScheme;
    return { contentType: d.contentType === undefined ? '' : str(d.contentType),
      encryptionScheme: scheme === undefined || scheme === null ? null : str(scheme),
      robustness: d.robustness === undefined ? '' : str(d.robustness) };
  }
  function toConfiguration(v) {
    const what = 'MediaKeySystemConfiguration';
    const d = dictionary(v, what);
    const list = (name, convert) => d[name] === undefined ? [] : sequence(d[name], convert, `${what}.${name}`);
    return {
      label: d.label === undefined ? '' : str(d.label),
      initDataTypes: list('initDataTypes', str),
      audioCapabilities: list('audioCapabilities', toCapability),
      videoCapabilities: list('videoCapabilities', toCapability),
      distinctiveIdentifier: requirement(d.distinctiveIdentifier, what),
      persistentState: requirement(d.persistentState, what),
      sessionTypes: d.sessionTypes === undefined ? undefined : sequence(d.sessionTypes, str, `${what}.sessionTypes`),
    };
  }

  const access = new WeakMap();
  class MediaKeySystemAccess {
    constructor(t, config) {
      if (t !== token) throw illegal('MediaKeySystemAccess');
      access.set(this, config);
    }
    get keySystem() { if (!access.has(this)) throw new TypeErrorC('Illegal invocation'); return KEY_SYSTEM; }
    getConfiguration() {
      const c = access.get(this);
      if (!c) throw new TypeErrorC('Illegal invocation');
      const caps = list => list.map(x => ({ contentType: x.contentType, encryptionScheme: x.encryptionScheme, robustness: x.robustness }));
      return { label: c.label, initDataTypes: c.initDataTypes.slice(), audioCapabilities: caps(c.audioCapabilities),
        videoCapabilities: caps(c.videoCapabilities), distinctiveIdentifier: c.distinctiveIdentifier,
        persistentState: c.persistentState, sessionTypes: c.sessionTypes.slice() };
    }
    createMediaKeys() {
      try {
        if (!access.has(this)) throw new TypeErrorC('Illegal invocation');
        if (native.opaque()) throw dom('EME is not available to an opaque origin.', 'SecurityError');
        return PromiseC.resolve(new MediaKeys(token, access.get(this)));
      } catch (e) { return reject(e); }
    }
  }

  function requestAccess(argc, keySystem, configs) {
    try {
      const what = "Failed to execute 'requestMediaKeySystemAccess' on 'Navigator'";
      if (argc < 2) throw new TypeErrorC(`${what}: 2 arguments required, but only ${argc} present.`);
      const system = str(keySystem);
      if (system === '') throw new TypeErrorC(`${what}: The key system provided is empty.`);
      const candidates = sequence(configs, toConfiguration, what);
      if (!candidates.length) throw new TypeErrorC(`${what}: The supportedConfigurations parameter is empty.`);
      if (system !== KEY_SYSTEM) throw dom('The key system specified is not supported.', 'NotSupportedError');
      // An opaque origin (a sandboxed frame) may not use EME.
      if (native.opaque()) throw dom('EME is not available to an opaque origin.', 'SecurityError');
      for (const candidate of candidates) {
        const config = supported(candidate);
        if (config) return PromiseC.resolve(new MediaKeySystemAccess(token, config));
      }
      throw dom('None of the requested configurations is supported.', 'NotSupportedError');
    } catch (e) { return reject(e); }
  }

  // ---- key status map ------------------------------------------------------------------------------
  const maps = new WeakMap();
  class MediaKeyStatusMap {
    constructor(t, entries) {
      if (t !== token) throw illegal('MediaKeyStatusMap');
      maps.set(this, entries);
    }
    get size() { return slot(this).size; }
    has(keyId) { return slot(this).has(hex(toBytes(keyId, "Failed to execute 'has' on 'MediaKeyStatusMap'"))); }
    get(keyId) { return slot(this).get(hex(toBytes(keyId, "Failed to execute 'get' on 'MediaKeyStatusMap'")))?.status; }
    entries() { return rows(this).map(([k, v]) => [k, v])[Symbol.iterator](); }
    keys() { return rows(this).map(r => r[0])[Symbol.iterator](); }
    values() { return rows(this).map(r => r[1])[Symbol.iterator](); }
    forEach(callback, ...rest) {
      if (typeof callback !== 'function') throw new TypeErrorC("Failed to execute 'forEach' on 'MediaKeyStatusMap': The callback provided as parameter 1 is not a function.");
      for (const [k, v] of rows(this)) apply(callback, rest[0], [v, k, this]);
    }
  }
  function slot(map) {
    const e = maps.get(map);
    if (!e) throw new TypeErrorC('Illegal invocation');
    return e;
  }
  // Chrome lists the key IDs sorted; each is a fresh ArrayBuffer.
  function rows(map) {
    return [...slot(map).entries()].sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)
      .map(([, v]) => [v.kid.slice().buffer, v.status]);
  }

  // ---- events --------------------------------------------------------------------------------------
  const messages = new WeakMap(), encrypteds = new WeakMap();
  class MediaKeyMessageEvent extends EventC {
    constructor(type, init) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'MediaKeyMessageEvent': 2 arguments required, but only ${arguments.length} present.`);
      const d = dictionary(init, "Failed to construct 'MediaKeyMessageEvent'");
      const types = ['license-request', 'license-renewal', 'license-release', 'individualization-request'];
      if (d.messageType === undefined) throw new TypeErrorC("Failed to construct 'MediaKeyMessageEvent': required member messageType is undefined.");
      const messageType = str(d.messageType);
      if (!types.includes(messageType)) throw new TypeErrorC(`Failed to construct 'MediaKeyMessageEvent': The provided value '${messageType}' is not a valid enum value of type MediaKeyMessageType.`);
      let size = -1;
      try { size = apply(abByteLength, d.message, []); } catch { /* not an ArrayBuffer */ }
      if (size < 0) throw new TypeErrorC("Failed to construct 'MediaKeyMessageEvent': required member message is not an ArrayBuffer.");
      super(type, d);
      messages.set(this, { messageType, message: d.message });
    }
    get messageType() { return state(messages, this).messageType; }
    get message() { return state(messages, this).message; }
  }
  class MediaEncryptedEvent extends EventC {
    constructor(type, init = {}) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'MediaEncryptedEvent': 1 argument required, but only 0 present.");
      const d = dictionary(init, "Failed to construct 'MediaEncryptedEvent'");
      const initDataType = d.initDataType === undefined ? '' : str(d.initDataType);
      let initData = d.initData === undefined ? null : d.initData;
      if (initData !== null) {
        let size = -1;
        try { size = apply(abByteLength, initData, []); } catch { /* not an ArrayBuffer */ }
        if (size < 0) throw new TypeErrorC("Failed to construct 'MediaEncryptedEvent': The provided value is not of type 'ArrayBuffer'.");
      }
      super(type, d);
      encrypteds.set(this, { initDataType, initData });
    }
    get initDataType() { return state(encrypteds, this).initDataType; }
    get initData() { return state(encrypteds, this).initData; }
  }
  function state(map, self) {
    const s = map.get(self);
    if (!s) throw new TypeErrorC('Illegal invocation');
    return s;
  }
  function fireEncrypted(element, type, hexData) {
    const bytes = new U8(hexData.length / 2);
    for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hexData.substr(i * 2, 2), 16);
    dispatch(element, new MediaEncryptedEvent('encrypted', { initDataType: type, initData: bytes.buffer }));
  }

  // ---- MediaKeys and MediaKeySession ---------------------------------------------------------------
  const keys = new WeakMap(), sessions = new WeakMap();
  const registry = typeof FinalizationRegistry === 'function' ? new FinalizationRegistry(id => native.dropStore(id)) : null;
  let nextSession = 1;
  class MediaKeys {
    constructor(t, config) {
      if (t !== token) throw illegal('MediaKeys');
      const id = native.makeStore();
      keys.set(this, { id, sessionTypes: config.sessionTypes.slice(), element: null, nextNative: 1 });
      if (registry) registry.register(this, id);
    }
    createSession(sessionType = 'temporary') {
      const k = state(keys, this);
      const type = str(sessionType);
      if (type !== 'temporary' && type !== 'persistent-license') throw new TypeErrorC(`Failed to execute 'createSession' on 'MediaKeys': The provided value '${type}' is not a valid enum value of type MediaKeySessionType.`);
      if (!k.sessionTypes.includes(type)) throw dom(`The session type '${type}' is not supported.`, 'NotSupportedError');
      return new MediaKeySession(token, this, k, type);
    }
    getStatusForPolicy(policy = {}) {
      try {
        state(keys, this);
        const d = dictionary(policy, "Failed to execute 'getStatusForPolicy' on 'MediaKeys'");
        if (d.minHdcpVersion !== undefined) str(d.minHdcpVersion);
        // Clear Key has no output protection to measure: every policy is met.
        return PromiseC.resolve('usable');
      } catch (e) { return reject(e); }
    }
    setServerCertificate(serverCertificate) {
      try {
        state(keys, this);
        if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'setServerCertificate' on 'MediaKeys': 1 argument required, but only 0 present.");
        const cert = toBytes(serverCertificate, "Failed to execute 'setServerCertificate' on 'MediaKeys'");
        if (!cert.length) throw new TypeErrorC("Failed to execute 'setServerCertificate' on 'MediaKeys': The serverCertificate parameter is empty.");
        // Clear Key has no use for a server certificate: not supported.
        return PromiseC.resolve(false);
      } catch (e) { return reject(e); }
    }
  }

  // The key IDs of a keyids / cenc request, or TypeError.
  function keyIdsOf(type, data) {
    const bad = m => new TypeErrorC(`Failed to execute 'generateRequest' on 'MediaKeySession': ${m}`);
    const ids = [];
    if (type === 'keyids') {
      let json;
      try { json = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(data)); } catch { throw bad('The initData is not valid JSON.'); }
      if (!isObject(json) || !A.isArray(json.kids) || !json.kids.length) throw bad('The initData has no kids.');
      for (const kid of json.kids) {
        const bytes = b64uDecode(kid);
        if (!bytes || bytes.length < 1 || bytes.length > MAX_KEY_ID) throw bad('The initData has an invalid key ID.');
        ids.push(bytes);
      }
    } else {
      const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
      let p = 0;
      while (p < data.length) {
        if (data.length - p < 32) throw bad('The initData is not a pssh box.');
        const size = view.getUint32(p);
        if (size < 32 || size > data.length - p || String.fromCharCode(data[p + 4], data[p + 5], data[p + 6], data[p + 7]) !== 'pssh') throw bad('The initData is not a pssh box.');
        const version = data[p + 8];
        if (version > 1) throw bad('The pssh box has an unknown version.');
        const system = hex(data.subarray(p + 12, p + 28));
        const end = p + size;
        let q = p + 28;
        if (version === 1) {
          const count = view.getUint32(q);
          q += 4;
          if (count > (end - q) / 16) throw bad('The pssh box is truncated.');
          if (system === COMMON) for (let i = 0; i < count; i++) ids.push(data.slice(q + i * 16, q + i * 16 + 16));
          q += count * 16;
        }
        if (end - q < 4) throw bad('The pssh box is truncated.');
        const length = view.getUint32(q);
        q += 4;
        if (length > end - q) throw bad('The pssh box is truncated.');
        p = end;
      }
      if (!ids.length) throw bad('The initData has no key IDs.');
    }
    return ids;
  }

  // The keys of a JWK Set license: [{kid, key}], or TypeError.
  function parseLicense(bytes) {
    const bad = m => new TypeErrorC(`Failed to execute 'update' on 'MediaKeySession': ${m}`);
    if (bytes.length > MAX_LICENSE) throw bad('The response is too long.');
    let text = '';
    for (const b of bytes) {
      if (b > 127) throw bad('The response is not ASCII.');
      text += String.fromCharCode(b);
    }
    let json;
    try { json = JSON.parse(text); } catch { throw bad('The response is not valid JSON.'); }
    if (!isObject(json) || !A.isArray(json.keys) || !json.keys.length) throw bad('The response is not a JWK Set.');
    if (json.type !== undefined && json.type !== 'temporary') throw bad("The response's type is not 'temporary'.");
    const out = [];
    for (const jwk of json.keys) {
      if (!isObject(jwk) || jwk.kty !== 'oct') throw bad('A key is not a symmetric JWK.');
      const kid = b64uDecode(jwk.kid), key = b64uDecode(jwk.k);
      if (!kid || kid.length < 1 || kid.length > MAX_KEY_ID) throw bad('A key has an invalid kid.');
      if (!key || key.length !== 16) throw bad('A key has an invalid k.');
      out.push({ kid, key });
    }
    return out;
  }

  class MediaKeySession extends EventTargetC {
    constructor(t, mediaKeys, k, type) {
      if (t !== token) throw illegal('MediaKeySession');
      super();
      let resolveClosed;
      const closed = new PromiseC(r => { resolveClosed = r; });
      const statuses = new Map();
      sessions.set(this, { mediaKeys, store: k, type, id: '', callable: false, closed, resolveClosed, isClosed: false,
        statuses, map: new MediaKeyStatusMap(token, statuses), native: k.nextNative++ });
    }
    get sessionId() { return state(sessions, this).id; }
    get expiration() { state(sessions, this); return NaN; }
    get closed() { return state(sessions, this).closed; }
    get keyStatuses() { return state(sessions, this).map; }
    generateRequest(initDataType, initData) {
      try {
        const s = state(sessions, this);
        const what = "Failed to execute 'generateRequest' on 'MediaKeySession'";
        if (arguments.length < 2) throw new TypeErrorC(`${what}: 2 arguments required, but only ${arguments.length} present.`);
        const type = str(initDataType);
        const data = toBytes(initData, what);
        if (s.isClosed || s.callable) throw dom('The session is already initialized or closed.', 'InvalidStateError');
        if (type === '') throw new TypeErrorC(`${what}: The initDataType parameter is empty.`);
        if (!data.length) throw new TypeErrorC(`${what}: The initData parameter is empty.`);
        if (!INIT_DATA_TYPES.includes(type)) throw dom(`The initialization data type '${type}' is not supported.`, 'NotSupportedError');
        if (data.length > MAX_INIT_DATA) throw new TypeErrorC(`${what}: The initData is too long.`);
        const ids = keyIdsOf(type, data);
        s.callable = true;
        s.id = `${nextSession++}`;
        const request = JSON.stringify({ kids: ids.map(b64uEncode), type: s.type });
        const message = new U8(new TextEncoder().encode(request)).buffer;
        task(() => {
          if (s.isClosed) return;
          dispatch(this, new MediaKeyMessageEvent('message', { messageType: 'license-request', message }));
        });
        return PromiseC.resolve();
      } catch (e) { return reject(e); }
    }
    load(sessionId) {
      try {
        const s = state(sessions, this);
        const what = "Failed to execute 'load' on 'MediaKeySession'";
        if (arguments.length < 1) throw new TypeErrorC(`${what}: 1 argument required, but only 0 present.`);
        const id = str(sessionId);
        if (s.isClosed || s.callable) throw dom('The session is already initialized or closed.', 'InvalidStateError');
        // Only persistent sessions load, and Clear Key has no storage to keep one in.
        if (s.type !== 'persistent-license') throw new TypeErrorC(`${what}: The session type is not 'persistent-license'.`);
        if (id === '') throw new TypeErrorC(`${what}: The sessionId parameter is empty.`);
        throw dom('Persistent sessions are not supported.', 'NotSupportedError');
      } catch (e) { return reject(e); }
    }
    update(response) {
      try {
        const s = state(sessions, this);
        const what = "Failed to execute 'update' on 'MediaKeySession'";
        if (arguments.length < 1) throw new TypeErrorC(`${what}: 1 argument required, but only 0 present.`);
        const bytes = toBytes(response, what);
        if (s.isClosed || !s.callable) throw dom('The session is closed or has no request.', 'InvalidStateError');
        if (!bytes.length) throw new TypeErrorC(`${what}: The response parameter is empty.`);
        const license = parseLicense(bytes);
        let changed = false;
        for (const { kid, key } of license) {
          const id = hex(kid), old = s.statuses.get(id);
          if (!old) changed = true;
          s.statuses.set(id, { kid, key, status: 'usable' });
        }
        publish(s);
        if (changed) task(() => { if (!s.isClosed) dispatch(this, new EventC('keystatuseschange')); });
        return PromiseC.resolve();
      } catch (e) { return reject(e); }
    }
    close() {
      try {
        const s = state(sessions, this);
        if (s.isClosed) return PromiseC.resolve();
        if (!s.callable) throw dom('The session has not been initialized.', 'InvalidStateError');
        shut(this, s);
        return PromiseC.resolve();
      } catch (e) { return reject(e); }
    }
    remove() {
      try {
        const s = state(sessions, this);
        if (s.isClosed || !s.callable) throw dom('The session is closed or has not been initialized.', 'InvalidStateError');
        // A temporary session keeps nothing to remove: the keys go and the session closes.
        shut(this, s);
        return PromiseC.resolve();
      } catch (e) { return reject(e); }
    }
  }
  // The keys this session can use, for the native store the media elements decrypt with.
  function publish(s) {
    const usable = [...s.statuses.values()].filter(v => v.status === 'usable' && v.kid.length === 16);
    const packed = new U8(usable.length * 32);
    usable.forEach((v, i) => { packed.set(v.kid, i * 32); packed.set(v.key, i * 32 + 16); });
    native.setKeys(s.store.id, s.native, packed);
  }
  // The "session closed" steps: no key remains, the closed promise settles.
  function shut(session, s) {
    s.isClosed = true;
    const had = s.statuses.size > 0;
    s.statuses.clear();
    publish(s);
    s.resolveClosed('closed-by-application');
    if (had) task(() => dispatch(session, new EventC('keystatuseschange')));
  }

  // ---- event handler attributes --------------------------------------------------------------------
  function handler(proto, type, owners) {
    const handlers = new WeakMap();
    define(proto, 'on' + type, { enumerable: true, configurable: true,
      get() { state(owners, this); return handlers.get(this)?.value ?? null; },
      set(v) {
        state(owners, this);
        const old = handlers.get(this);
        if (typeof v !== 'function') {
          if (old) { this.removeEventListener(type, old.fn); handlers.delete(this); }
        } else if (old) old.value = v;
        else {
          const entry = { value: v, fn: e => apply(entry.value, this, [e]) };
          handlers.set(this, entry);
          this.addEventListener(type, entry.fn);
        }
      } });
  }
  handler(MediaKeySession.prototype, 'keystatuseschange', sessions);
  handler(MediaKeySession.prototype, 'message', sessions);

  for (const C of [MediaKeySystemAccess, MediaKeys, MediaKeySession, MediaKeyStatusMap, MediaKeyMessageEvent, MediaEncryptedEvent]) {
    define(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = get(C.prototype, k);
      d.enumerable = true;
      define(C.prototype, k, d);
    }
  }
  define(MediaKeyStatusMap.prototype, Symbol.iterator, { value: MediaKeyStatusMap.prototype.entries, writable: true, enumerable: false, configurable: true });
  for (const C of [MediaKeySystemAccess, MediaKeys, MediaKeySession, MediaKeyStatusMap]) define(C, 'length', { value: 0, configurable: true });
  define(MediaKeyMessageEvent, 'length', { value: 2, configurable: true });
  define(MediaEncryptedEvent, 'length', { value: 1, configurable: true });

  // ---- HTMLMediaElement.setMediaKeys ---------------------------------------------------------------
  const attached = new WeakMap();
  function setMediaKeys(element, mediaKeys, argc) {
    try {
      if (argc < 1) throw new TypeErrorC("Failed to execute 'setMediaKeys' on 'HTMLMediaElement': 1 argument required, but only 0 present.");
      if (mediaKeys === undefined) mediaKeys = null;
      if (mediaKeys !== null && !keys.has(mediaKeys)) throw new TypeErrorC("Failed to execute 'setMediaKeys' on 'HTMLMediaElement': Failed to convert value to 'MediaKeys'.");
      const current = attached.get(element) ?? null;
      if (current === mediaKeys) return PromiseC.resolve();
      const next = mediaKeys && keys.get(mediaKeys);
      // One MediaKeys object serves one media element at a time.
      if (next && next.element && next.element !== element) throw quotaExceeded('The MediaKeys object is already in use by another media element.');
      if (current) keys.get(current).element = null;
      if (next) next.element = element;
      if (mediaKeys) attached.set(element, mediaKeys); else attached.delete(element);
      native.attach(element, next ? next.id : null);
      return PromiseC.resolve();
    } catch (e) { return reject(e); }
  }
  const getMediaKeys = element => attached.get(element) ?? null;

  return { MediaKeySystemAccess, MediaKeys, MediaKeySession, MediaKeyStatusMap, MediaKeyMessageEvent, MediaEncryptedEvent,
    requestAccess, setMediaKeys, getMediaKeys, fireEncrypted };
});
