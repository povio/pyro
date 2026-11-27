// Credential Management and Web Authentication for one realm: CredentialsContainer (navigator.credentials),
// Credential, PasswordCredential, FederatedCredential, PublicKeyCredential, AuthenticatorResponse,
// AuthenticatorAttestationResponse and AuthenticatorAssertionResponse. The WebIDL argument conversion, the
// client steps of create() / get() (RP ID, client data, timeout, abort, permissions policy, user activation,
// extension inputs and outputs) and the credential objects are here; the authenticators are native
// (webauthn/): `native.authenticate(requestJson)` answers {status: 'ok' | 'error' | 'pending', ...} with
// bytes as base64url strings, and the client waits out a 'pending' request (no authenticator can answer it)
// until its timeout or abort, as a browser waits for a security key nobody touches.
//   native.active() -> fully active; native.frame() -> JSON {origin, host, same, policyGet, policyCreate,
//   activation}; native.rpId(candidate) -> valid RP ID or null; native.consumeActivation();
//   native.platform() -> JSON {uvpaa, conditional}; native.signal(json) -> JSON.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const Uint8ArrayC = Uint8Array;
  const ArrayC = Array;
  const JSONC = JSON;
  const SymbolIterator = Symbol.iterator;
  const apply = Reflect.apply;
  const setTimeoutC = setTimeout;
  const clearTimeoutC = clearTimeout;
  const resolve = (v) => PromiseC.resolve(v);
  const reject = (e) => PromiseC.reject(e);
  const dom = (message, name) => new DOMExceptionC(message, name);
  const typeError = (message) => new TypeErrorC(message);
  const illegal = (name) => typeError(`Failed to construct '${name}': Illegal constructor`);
  const invocation = () => typeError('Illegal invocation');
  const isObj = (v) => v !== null && (typeof v === 'object' || typeof v === 'function');
  const tag = (C, name) => O.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });

  // ---- bytes

  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  const REV = new O();
  for (let i = 0; i < 64; i++) REV[B64[i]] = i;
  // base64url without padding.
  const encode = (u8) => {
    let out = '';
    let i = 0;
    for (; i + 2 < u8.length; i += 3) {
      const n = (u8[i] << 16) | (u8[i + 1] << 8) | u8[i + 2];
      out += B64[n >> 18] + B64[(n >> 12) & 63] + B64[(n >> 6) & 63] + B64[n & 63];
    }
    const rest = u8.length - i;
    if (rest === 1) {
      const n = u8[i] << 16;
      out += B64[n >> 18] + B64[(n >> 12) & 63];
    } else if (rest === 2) {
      const n = (u8[i] << 16) | (u8[i + 1] << 8);
      out += B64[n >> 18] + B64[(n >> 12) & 63] + B64[(n >> 6) & 63];
    }
    return out;
  };
  // A Uint8Array, or null when `text` is not base64url without padding.
  const decode = (text) => {
    if (text.length % 4 === 1) return null;
    const out = new Uint8ArrayC(((text.length * 3) >> 2));
    let acc = 0, bits = 0, n = 0;
    for (let i = 0; i < text.length; i++) {
      const v = REV[text[i]];
      if (v === undefined) return null;
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out[n++] = (acc >> bits) & 255;
      }
    }
    return out;
  };
  const buffer = (u8) => u8.buffer.byteLength === u8.length ? u8.buffer : u8.slice().buffer;
  const bufferOf = (b64) => buffer(decode(b64));
  const abLength = O.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const isArrayBuffer = (v) => { try { apply(abLength, v, []); return true; } catch { return false; } };
  const isView = ArrayBuffer.isView;
  // WebIDL BufferSource (not [AllowShared]): a copy of the bytes of an ArrayBuffer or view.
  const bytes = (v, what) => {
    if (isObj(v)) {
      if (isArrayBuffer(v)) return new Uint8ArrayC(v).slice();
      if (isView(v)) {
        if (isArrayBuffer(v.buffer)) return new Uint8ArrayC(v.buffer, v.byteOffset, v.byteLength).slice();
        throw typeError(`${what} must not be backed by a SharedArrayBuffer`);
      }
    }
    throw typeError(`${what} is not of type 'BufferSource'`);
  };

  // ---- WebIDL conversions

  const dict = (v, what) => {
    if (v === undefined || v === null) return {};
    if (!isObj(v)) throw typeError(`${what} is not an object`);
    return v;
  };
  const str = (v) => `${v}`;
  const usv = (v) => { const s = `${v}`; return typeof s.toWellFormed === 'function' ? s.toWellFormed() : s; };
  const required = (v, what) => { if (v === undefined) throw typeError(`${what} is required`); return v; };
  const sequence = (v, what) => {
    if (!isObj(v) || typeof v[SymbolIterator] !== 'function') throw typeError(`${what} is not a sequence`);
    const out = [];
    for (const item of v) out[out.length] = item;
    return out;
  };
  const unsignedLong = (v) => (+v) >>> 0;
  const strings = (v, what) => (v === undefined ? [] : sequence(v, what).map(str));
  const enumOf = (v, values, what) => {
    const s = `${v}`;
    if (!values.includes(s)) throw typeError(`The provided value '${s}' is not a valid enum value of ${what}`);
    return s;
  };

  const signalAborted = O.getOwnPropertyDescriptor(AbortSignal.prototype, 'aborted').get;
  const signalReason = O.getOwnPropertyDescriptor(AbortSignal.prototype, 'reason').get;
  const isSignal = (v) => { try { apply(signalAborted, v, []); return true; } catch { return false; } };
  const addListener = EventTarget.prototype.addEventListener;
  const removeListener = EventTarget.prototype.removeEventListener;
  const signalOf = (v) => {
    if (v === undefined) return undefined;
    if (!isSignal(v)) throw typeError("'signal' is not of type 'AbortSignal'");
    return v;
  };

  // The extension inputs a client understands, converted. Unknown ones are ignored.
  const prfValues = (v, what) => {
    const d = dict(v, what);
    const out = { first: bytes(required(d.first, `${what}.first`), `${what}.first`) };
    if (d.second !== undefined) out.second = bytes(d.second, `${what}.second`);
    return out;
  };
  const extensionsOf = (v) => {
    const d = dict(v, 'extensions');
    const out = {};
    if (d.appid !== undefined) out.appid = str(d.appid);
    if (d.appidExclude !== undefined) out.appidExclude = str(d.appidExclude);
    if (d.credBlob !== undefined) out.credBlob = bytes(d.credBlob, 'credBlob');
    if (d.credProps !== undefined) out.credProps = !!d.credProps;
    if (d.credentialProtectionPolicy !== undefined) out.credentialProtectionPolicy = str(d.credentialProtectionPolicy);
    if (d.enforceCredentialProtectionPolicy !== undefined) out.enforceCredentialProtectionPolicy = !!d.enforceCredentialProtectionPolicy;
    if (d.getCredBlob !== undefined) out.getCredBlob = !!d.getCredBlob;
    if (d.hmacCreateSecret !== undefined) out.hmacCreateSecret = !!d.hmacCreateSecret;
    if (d.largeBlob !== undefined) {
      const lb = dict(d.largeBlob, 'largeBlob');
      const blob = {};
      if (lb.read !== undefined) blob.read = !!lb.read;
      if (lb.support !== undefined) blob.support = enumOf(lb.support, ['required', 'preferred'], 'LargeBlobSupport');
      if (lb.write !== undefined) blob.write = bytes(lb.write, 'largeBlob.write');
      out.largeBlob = blob;
    }
    if (d.minPinLength !== undefined) out.minPinLength = !!d.minPinLength;
    if (d.payment !== undefined) { dict(d.payment, 'payment'); out.payment = true; }
    if (d.prf !== undefined) {
      const p = dict(d.prf, 'prf');
      const prf = {};
      if (p.eval !== undefined) prf.eval = prfValues(p.eval, 'prf.eval');
      if (p.evalByCredential !== undefined) {
        const table = dict(p.evalByCredential, 'prf.evalByCredential');
        if (p.evalByCredential === null) throw typeError('prf.evalByCredential is null');
        prf.evalByCredential = O.keys(table).map((key) => [key, prfValues(table[key], 'prf.evalByCredential value')]);
      }
      out.prf = prf;
    }
    return out;
  };

  const descriptor = (v, what) => {
    const d = dict(v, what);
    const out = { type: str(required(d.type, `${what}.type`)), id: bytes(required(d.id, `${what}.id`), `${what}.id`) };
    if (d.transports !== undefined) out.transports = strings(d.transports, `${what}.transports`);
    return out;
  };
  const descriptors = (v, what) => (v === undefined ? [] : sequence(v, what).map((item) => descriptor(item, `${what} entry`)));

  // PublicKeyCredentialCreationOptions
  const creationOptions = (v) => {
    const d = dict(v, 'publicKey');
    const out = {};
    out.attestation = d.attestation === undefined ? 'none' : str(d.attestation);
    out.attestationFormats = strings(d.attestationFormats, 'attestationFormats');
    if (d.authenticatorSelection !== undefined) {
      const sel = dict(d.authenticatorSelection, 'authenticatorSelection');
      out.selection = {
        attachment: sel.authenticatorAttachment === undefined ? undefined : str(sel.authenticatorAttachment),
        residentKey: sel.residentKey === undefined ? undefined : str(sel.residentKey),
        requireResidentKey: !!sel.requireResidentKey,
        userVerification: sel.userVerification === undefined ? 'preferred' : str(sel.userVerification),
      };
    } else {
      out.selection = { attachment: undefined, residentKey: undefined, requireResidentKey: false, userVerification: 'preferred' };
    }
    out.challenge = bytes(required(d.challenge, 'challenge'), 'challenge');
    out.exclude = descriptors(d.excludeCredentials, 'excludeCredentials');
    if (d.extensions !== undefined) out.extensions = extensionsOf(d.extensions);
    out.hints = strings(d.hints, 'hints');
    out.params = sequence(required(d.pubKeyCredParams, 'pubKeyCredParams'), 'pubKeyCredParams').map((p) => {
      const e = dict(p, 'pubKeyCredParams entry');
      return { type: str(required(e.type, 'pubKeyCredParams type')), alg: required(e.alg, 'pubKeyCredParams alg') | 0 };
    });
    const rp = dict(required(d.rp, 'rp'), 'rp');
    out.rp = { id: rp.id === undefined ? undefined : str(rp.id), name: str(required(rp.name, 'rp.name')) };
    if (d.timeout !== undefined) out.timeout = unsignedLong(d.timeout);
    const user = dict(required(d.user, 'user'), 'user');
    out.user = {
      id: bytes(required(user.id, 'user.id'), 'user.id'),
      name: str(required(user.name, 'user.name')),
      displayName: str(required(user.displayName, 'user.displayName')),
    };
    if (out.user.id.length < 1 || out.user.id.length > 64) throw typeError('The length of user.id must be between 1 and 64 bytes');
    return out;
  };

  // PublicKeyCredentialRequestOptions
  const requestOptions = (v) => {
    const d = dict(v, 'publicKey');
    const out = {};
    out.allow = descriptors(d.allowCredentials, 'allowCredentials');
    out.challenge = bytes(required(d.challenge, 'challenge'), 'challenge');
    if (d.extensions !== undefined) out.extensions = extensionsOf(d.extensions);
    out.hints = strings(d.hints, 'hints');
    if (d.rpId !== undefined) out.rpId = str(d.rpId);
    if (d.timeout !== undefined) out.timeout = unsignedLong(d.timeout);
    out.userVerification = d.userVerification === undefined ? 'preferred' : str(d.userVerification);
    return out;
  };

  // ---- WebAuthn client steps

  const notAllowed = (message) => dom(message || 'The operation either timed out or was not allowed.', 'NotAllowedError');
  const info = () => JSONC.parse(native.frame());
  const clientData = (type, challenge, f) =>
    `{"type":${JSONC.stringify(type)},"challenge":${JSONC.stringify(encode(challenge))},"origin":${JSONC.stringify(f.origin)},"crossOrigin":${f.same ? 'false' : 'true'}}`;
  // The RP ID a request names: its own, else the origin's effective domain; SecurityError if the origin
  // may not use it.
  const rpIdOf = (given, f) => {
    if (f.origin === null || f.host === null) throw dom('The origin of the document is not a valid domain.', 'SecurityError');
    const id = native.rpId(given === undefined ? f.host : given);
    if (id === null) throw dom('The relying party ID is not a registrable domain suffix of, nor equal to the current domain.', 'SecurityError');
    return id;
  };
  const extensionsRequest = (ext) => {
    const out = {};
    for (const k of ['credProps', 'minPinLength', 'hmacCreateSecret', 'getCredBlob']) if (ext[k] !== undefined) out[k] = ext[k];
    if (ext.credBlob !== undefined) out.credBlob = encode(ext.credBlob);
    if (ext.largeBlob !== undefined) {
      const lb = {};
      if (ext.largeBlob.support !== undefined) lb.support = ext.largeBlob.support;
      if (ext.largeBlob.read) lb.read = true;
      if (ext.largeBlob.write !== undefined) lb.write = encode(ext.largeBlob.write);
      out.largeBlob = lb;
    }
    if (ext.prf !== undefined) {
      const values = (v) => (v.second === undefined ? { first: encode(v.first) } : { first: encode(v.first), second: encode(v.second) });
      const prf = {};
      if (ext.prf.eval !== undefined) prf.eval = values(ext.prf.eval);
      if (ext.prf.evalByCredential !== undefined) {
        prf.evalByCredential = {};
        for (const [key, v] of ext.prf.evalByCredential) prf.evalByCredential[key] = values(v);
      }
      out.prf = prf;
    }
    return out;
  };
  // The extension outputs of a response (base64url bytes in JSON) as the dictionary a page reads.
  const outputsOf = (json) => {
    const out = {};
    if (json.appid !== undefined) out.appid = json.appid;
    if (json.credProps !== undefined) out.credProps = { ...json.credProps };
    if (json.credBlob !== undefined) out.credBlob = json.credBlob;
    if (json.getCredBlob !== undefined) out.getCredBlob = bufferOf(json.getCredBlob);
    if (json.largeBlob !== undefined) {
      const lb = {};
      if (json.largeBlob.supported !== undefined) lb.supported = json.largeBlob.supported;
      if (json.largeBlob.blob !== undefined) lb.blob = bufferOf(json.largeBlob.blob);
      if (json.largeBlob.written !== undefined) lb.written = json.largeBlob.written;
      out.largeBlob = lb;
    }
    if (json.prf !== undefined) {
      const prf = {};
      if (json.prf.enabled !== undefined) prf.enabled = json.prf.enabled;
      if (json.prf.results !== undefined) {
        prf.results = { first: bufferOf(json.prf.results.first) };
        if (json.prf.results.second !== undefined) prf.results.second = bufferOf(json.prf.results.second);
      }
      out.prf = prf;
    }
    return out;
  };

  // Runs an authenticator request: resolves with `build(response)`, rejects with the authenticator's error, and
  // waits out a request nobody can answer until `timeout` (NotAllowedError) or `signal` aborts.
  const runRequest = (request, signal, timeout, build, immediate) => new PromiseC((resolvePromise, rejectPromise) => {
    let timer;
    let onAbort;
    const cleanup = () => {
      if (timer !== undefined) clearTimeoutC(timer);
      if (signal !== undefined && onAbort !== undefined) apply(removeListener, signal, ['abort', onAbort]);
    };
    const fail = (error) => { cleanup(); rejectPromise(error); };
    if (signal !== undefined) {
      onAbort = () => fail(apply(signalReason, signal, []));
      apply(addListener, signal, ['abort', onAbort]);
    }
    let response;
    try {
      response = JSONC.parse(native.authenticate(JSONC.stringify(request)));
    } catch (e) {
      fail(e);
      return;
    }
    if (response.status === 'ok') {
      cleanup();
      try { resolvePromise(build(response)); } catch (e) { rejectPromise(e); }
    } else if (response.status === 'error') {
      fail(dom(response.message, response.name));
    } else if (immediate) {
      fail(notAllowed());
    } else {
      timer = setTimeoutC(() => fail(notAllowed()), timeout);
    }
  });
  // Chrome bounds a request's timeout at ten minutes; the default is five.
  const timeoutOf = (t) => (t === undefined ? 300000 : t > 600000 ? 600000 : t);

  const createPublicKey = (pk, signal) => {
    const f = info();
    if (!f.policyCreate) {
      throw notAllowed("The 'publickey-credentials-create' feature is not enabled in this document. Permissions Policy may be used to delegate Web Authentication capabilities to cross-origin child frames.");
    }
    const rpId = rpIdOf(pk.rp.id, f);
    let algs = pk.params.filter((p) => p.type === 'public-key').map((p) => p.alg);
    if (pk.params.length > 0 && algs.length === 0) throw dom('No pubKeyCredParams entry has the type "public-key".', 'NotSupportedError');
    if (pk.params.length === 0) algs = [-7, -257];
    const ext = pk.extensions || {};
    if (ext.appid !== undefined) throw dom("The 'appid' extension is only supported by get().", 'NotSupportedError');
    if (ext.payment !== undefined) throw notAllowed("The 'payment' extension is not supported.");
    if (ext.largeBlob !== undefined && (ext.largeBlob.read !== undefined || ext.largeBlob.write !== undefined)) {
      throw dom("The 'largeBlob' read and write members are only supported by get().", 'NotSupportedError');
    }
    if (ext.prf !== undefined && ext.prf.evalByCredential !== undefined) {
      throw dom("The 'prf' evalByCredential member is only supported by get().", 'NotSupportedError');
    }
    if (!f.same) {
      // A cross-origin frame needs (and spends) a user activation of its own.
      if (!f.activation) throw notAllowed('A user activation is required to create a credential in a cross-origin iframe.');
      native.consumeActivation();
    }
    const sel = pk.selection;
    const residentKey = ['required', 'preferred', 'discouraged'].includes(sel.residentKey) ? sel.residentKey : sel.requireResidentKey ? 'required' : 'discouraged';
    const data = clientData('webauthn.create', pk.challenge, f);
    const request = {
      op: 'create',
      rpId,
      clientData: data,
      user: { id: encode(pk.user.id), name: pk.user.name, displayName: pk.user.displayName },
      algs,
      exclude: pk.exclude.filter((d) => d.type === 'public-key').map((d) => encode(d.id)),
      residentKey,
      userVerification: ['required', 'discouraged'].includes(sel.userVerification) ? sel.userVerification : 'preferred',
      attachment: sel.attachment === 'platform' || sel.attachment === 'cross-platform' ? sel.attachment : null,
      attestation: ['none', 'indirect', 'direct', 'enterprise'].includes(pk.attestation) ? pk.attestation : 'none',
      extensions: extensionsRequest(ext),
    };
    return runRequest(request, signal, timeoutOf(pk.timeout), (r) => new PublicKeyCredential(token, {
      id: r.id,
      rawId: decode(r.id),
      attachment: r.attachment,
      extensions: r.extensions,
      response: new AuthenticatorAttestationResponse(token, {
        clientDataJSON: decode(r.clientDataJSON),
        attestationObject: decode(r.attestationObject),
        authenticatorData: decode(r.authenticatorData),
        publicKey: r.publicKey === null ? null : decode(r.publicKey),
        publicKeyAlgorithm: r.publicKeyAlgorithm,
        transports: r.transports,
      }),
    }), false);
  };

  const getPublicKey = (pk, signal, mediation, uiMode) => {
    const f = info();
    if (!f.policyGet) {
      throw notAllowed("The 'publickey-credentials-get' feature is not enabled in this document. Permissions Policy may be used to delegate Web Authentication capabilities to cross-origin child frames.");
    }
    const immediate = uiMode === 'immediate';
    if (immediate && mediation === 'conditional') throw dom("Immediate mediation can't be combined with conditional mediation.", 'NotSupportedError');
    const rpId = rpIdOf(pk.rpId, f);
    const ext = pk.extensions || {};
    if (ext.credProps !== undefined) throw dom("The 'credProps' extension is only supported by create().", 'NotSupportedError');
    if (ext.payment !== undefined) throw notAllowed("The 'payment' extension is only supported by create().");
    if (ext.largeBlob !== undefined && (ext.largeBlob.support !== undefined || (ext.largeBlob.read && ext.largeBlob.write !== undefined))) {
      throw dom("The 'largeBlob' support member is only supported by create(), and read and write are exclusive.", 'NotSupportedError');
    }
    const allow = pk.allow.filter((d) => d.type === 'public-key');
    if (ext.prf !== undefined && ext.prf.evalByCredential !== undefined && ext.prf.evalByCredential.length > 0) {
      if (pk.allow.length === 0) throw dom("The 'prf' evalByCredential member needs allowCredentials.", 'NotSupportedError');
      for (const [key] of ext.prf.evalByCredential) {
        const id = decode(key);
        if (id === null) throw dom('A prf evalByCredential key is not valid base64url.', 'SyntaxError');
        if (!allow.some((d) => encode(d.id) === key)) throw dom('A prf evalByCredential key is not in allowCredentials.', 'SyntaxError');
      }
    }
    if (immediate) {
      if (pk.allow.length > 0) throw notAllowed();
      if (!f.activation) throw notAllowed('Immediate mediation needs a user activation.');
      native.consumeActivation();
    }
    const data = clientData('webauthn.get', pk.challenge, f);
    const request = {
      op: 'get',
      rpId,
      clientData: data,
      allow: pk.allow.length === 0 ? null : allow.map((d) => encode(d.id)),
      userVerification: ['required', 'discouraged'].includes(pk.userVerification) ? pk.userVerification : 'preferred',
      extensions: extensionsRequest(ext),
      immediate,
      conditional: mediation === 'conditional',
    };
    return runRequest(request, signal, timeoutOf(pk.timeout), (r) => new PublicKeyCredential(token, {
      id: r.id,
      rawId: decode(r.id),
      attachment: r.attachment,
      extensions: r.extensions,
      response: new AuthenticatorAssertionResponse(token, {
        clientDataJSON: decode(r.clientDataJSON),
        authenticatorData: decode(r.authenticatorData),
        signature: decode(r.signature),
        userHandle: r.userHandle === null ? null : decode(r.userHandle),
      }),
    }), immediate);
  };

  // ---- credential classes

  const credentialSlots = new WeakMap();
  const responseSlots = new WeakMap();
  const slotOf = (self) => { const s = credentialSlots.get(self); if (s === undefined) throw invocation(); return s; };
  const responseOf = (self) => { const s = responseSlots.get(self); if (s === undefined) throw invocation(); return s; };
  const platform = () => JSONC.parse(native.platform());
  const activeOrReject = () => (native.active() ? null : dom('The document is not fully active.', 'InvalidStateError'));

  class Credential {
    constructor(...args) {
      if (args[0] !== token) throw illegal('Credential');
    }
    get id() { return slotOf(this).id; }
    get type() { return slotOf(this).type; }
    static isConditionalMediationAvailable() {
      try {
        return resolve(platform().conditional);
      } catch (e) {
        return reject(e);
      }
    }
  }
  tag(Credential, 'Credential');

  class PasswordCredential extends Credential {
    constructor(data) {
      super(token);
      const init = passwordInit(data);
      credentialSlots.set(this, { id: init.id, type: 'password', name: init.name, iconURL: init.iconURL, password: init.password });
    }
    get password() { return slotOf(this).password; }
    get name() { return slotOf(this).name; }
    get iconURL() { return slotOf(this).iconURL; }
  }
  tag(PasswordCredential, 'PasswordCredential');

  class FederatedCredential extends Credential {
    constructor(data) {
      super(token);
      const init = federatedInit(data);
      credentialSlots.set(this, { id: init.id, type: 'federated', name: init.name, iconURL: init.iconURL, provider: init.provider, protocol: init.protocol });
    }
    get provider() { return slotOf(this).provider; }
    get protocol() { return slotOf(this).protocol; }
    get name() { return slotOf(this).name; }
    get iconURL() { return slotOf(this).iconURL; }
  }
  tag(FederatedCredential, 'FederatedCredential');

  // PasswordCredentialInit: an HTMLFormElement (its autocomplete fields) or PasswordCredentialData.
  const autocompleteTokens = (el) => {
    const value = el.getAttribute('autocomplete');
    return value === null ? [] : value.toLowerCase().split(/\s+/).filter((t) => t !== '');
  };
  const passwordInit = (v) => {
    if (!isObj(v)) throw typeError("The provided value is not of type 'PasswordCredentialInit'");
    if (typeof v.nodeType === 'number' && v.nodeType === 1 && typeof v.tagName === 'string' && v.tagName.toLowerCase() === 'form') {
      const init = { id: undefined, password: undefined, name: '', iconURL: '' };
      for (const el of v.elements) {
        if (typeof el.getAttribute !== 'function') continue;
        const tokens = autocompleteTokens(el);
        if (tokens.includes('username') && init.id === undefined) init.id = usv(el.value);
        else if ((tokens.includes('current-password') || tokens.includes('new-password')) && init.password === undefined) init.password = usv(el.value);
        else if (tokens.includes('photo') && init.iconURL === '') init.iconURL = usv(el.value);
        else if (tokens.includes('name') && init.name === '') init.name = usv(el.value);
      }
      if (init.id === undefined || init.password === undefined) throw typeError('The form has no username and password field.');
      return init;
    }
    return {
      id: usv(required(v.id, 'id')),
      password: usv(required(v.password, 'password')),
      name: v.name === undefined ? '' : usv(v.name),
      iconURL: v.iconURL === undefined ? '' : usv(v.iconURL),
    };
  };
  const federatedInit = (v) => {
    if (!isObj(v)) throw typeError("The provided value is not of type 'FederatedCredentialInit'");
    return {
      id: usv(required(v.id, 'id')),
      provider: usv(required(v.provider, 'provider')),
      name: v.name === undefined ? '' : usv(v.name),
      iconURL: v.iconURL === undefined ? '' : usv(v.iconURL),
      protocol: v.protocol === undefined ? null : str(v.protocol),
    };
  };

  class AuthenticatorResponse {
    constructor(...args) {
      if (args[0] !== token) throw illegal('AuthenticatorResponse');
      const data = args[1];
      responseSlots.set(this, { clientDataJSON: buffer(data.clientDataJSON) });
    }
    get clientDataJSON() { return responseOf(this).clientDataJSON; }
  }
  tag(AuthenticatorResponse, 'AuthenticatorResponse');

  class AuthenticatorAttestationResponse extends AuthenticatorResponse {
    constructor(...args) {
      if (args[0] !== token) throw illegal('AuthenticatorAttestationResponse');
      const data = args[1];
      super(token, data);
      O.assign(responseSlots.get(this), {
        attestationObject: buffer(data.attestationObject),
        authenticatorData: data.authenticatorData,
        publicKey: data.publicKey,
        publicKeyAlgorithm: data.publicKeyAlgorithm,
        transports: data.transports,
        kind: 'attestation',
      });
    }
    get attestationObject() { return responseOf(this).attestationObject; }
    getTransports() { return ArrayC.from(responseOf(this).transports); }
    getAuthenticatorData() { return buffer(responseOf(this).authenticatorData.slice()); }
    getPublicKey() {
      const key = responseOf(this).publicKey;
      return key === null ? null : buffer(key.slice());
    }
    getPublicKeyAlgorithm() { return responseOf(this).publicKeyAlgorithm; }
  }
  tag(AuthenticatorAttestationResponse, 'AuthenticatorAttestationResponse');

  class AuthenticatorAssertionResponse extends AuthenticatorResponse {
    constructor(...args) {
      if (args[0] !== token) throw illegal('AuthenticatorAssertionResponse');
      const data = args[1];
      super(token, data);
      O.assign(responseSlots.get(this), {
        authenticatorData: buffer(data.authenticatorData),
        signature: buffer(data.signature),
        userHandle: data.userHandle === null ? null : buffer(data.userHandle),
        kind: 'assertion',
      });
    }
    get authenticatorData() { return responseOf(this).authenticatorData; }
    get signature() { return responseOf(this).signature; }
    get userHandle() { return responseOf(this).userHandle; }
  }
  tag(AuthenticatorAssertionResponse, 'AuthenticatorAssertionResponse');

  // The attestation / assertion response as JSON (WebAuthn registration / authentication response JSON).
  const responseJSON = (response) => {
    const r = responseOf(response);
    if (r.kind === 'attestation') {
      const out = {
        clientDataJSON: encode(new Uint8ArrayC(r.clientDataJSON)),
        authenticatorData: encode(r.authenticatorData),
        transports: ArrayC.from(r.transports),
        publicKeyAlgorithm: r.publicKeyAlgorithm,
        attestationObject: encode(new Uint8ArrayC(r.attestationObject)),
      };
      if (r.publicKey !== null) out.publicKey = encode(r.publicKey);
      return out;
    }
    const out = {
      clientDataJSON: encode(new Uint8ArrayC(r.clientDataJSON)),
      authenticatorData: encode(new Uint8ArrayC(r.authenticatorData)),
      signature: encode(new Uint8ArrayC(r.signature)),
    };
    if (r.userHandle !== null) out.userHandle = encode(new Uint8ArrayC(r.userHandle));
    return out;
  };

  class PublicKeyCredential extends Credential {
    constructor(...args) {
      if (args[0] !== token) throw illegal('PublicKeyCredential');
      const data = args[1];
      super(token);
      credentialSlots.set(this, {
        id: data.id,
        type: 'public-key',
        rawId: buffer(data.rawId),
        response: data.response,
        attachment: data.attachment,
        extensions: data.extensions,
      });
    }
    get rawId() { return slotOf(this).rawId; }
    get response() { return slotOf(this).response; }
    get authenticatorAttachment() { return slotOf(this).attachment; }
    getClientExtensionResults() { return outputsOf(slotOf(this).extensions); }
    toJSON() {
      const s = slotOf(this);
      const out = {
        id: s.id,
        rawId: s.id,
        response: responseJSON(s.response),
        clientExtensionResults: JSONC.parse(JSONC.stringify(s.extensions)),
        type: 'public-key',
      };
      if (s.attachment !== null) out.authenticatorAttachment = s.attachment;
      return out;
    }
    static isConditionalMediationAvailable() {
      try {
        return resolve(platform().conditional);
      } catch (e) {
        return reject(e);
      }
    }
    static isUserVerifyingPlatformAuthenticatorAvailable() {
      try {
        return resolve(platform().uvpaa);
      } catch (e) {
        return reject(e);
      }
    }
    static getClientCapabilities() {
      try {
        const p = platform();
        // Sorted by key: what this client does, with the context's authenticators.
        const entries = [
          ['conditionalCreate', p.conditional],
          ['conditionalGet', p.conditional],
          ['extension:appid', false],
          ['extension:appidExclude', false],
          ['extension:credBlob', true],
          ['extension:credProps', true],
          ['extension:credentialProtectionPolicy', false],
          ['extension:enforceCredentialProtectionPolicy', false],
          ['extension:getCredBlob', true],
          ['extension:hmacCreateSecret', false],
          ['extension:largeBlob', true],
          ['extension:minPinLength', true],
          ['extension:prf', true],
          ['hybridTransport', false],
          ['immediateGet', true],
          ['passkeyPlatformAuthenticator', p.conditional],
          ['relatedOrigins', false],
          ['signalAllAcceptedCredentials', true],
          ['signalCurrentUserDetails', true],
          ['signalUnknownCredential', true],
          ['userVerifyingPlatformAuthenticator', p.uvpaa],
        ];
        const out = {};
        for (const [key, value] of entries) out[key] = !!value;
        return resolve(out);
      } catch (e) {
        return reject(e);
      }
    }
    static parseCreationOptionsFromJSON(options) {
      const d = dict(options, 'options');
      const bin = (v, what) => {
        const b = decode(str(v));
        if (b === null) throw dom(`${what} is not valid base64url.`, 'EncodingError');
        return buffer(b);
      };
      const out = {};
      const rp = dict(required(d.rp, 'rp'), 'rp');
      out.rp = { name: str(required(rp.name, 'rp.name')) };
      if (rp.id !== undefined) out.rp.id = str(rp.id);
      const user = dict(required(d.user, 'user'), 'user');
      out.user = { id: bin(required(user.id, 'user.id'), 'user.id'), name: str(required(user.name, 'user.name')), displayName: str(required(user.displayName, 'user.displayName')) };
      out.challenge = bin(required(d.challenge, 'challenge'), 'challenge');
      out.pubKeyCredParams = sequence(required(d.pubKeyCredParams, 'pubKeyCredParams'), 'pubKeyCredParams').map((p) => {
        const e = dict(p, 'pubKeyCredParams entry');
        return { type: str(required(e.type, 'type')), alg: required(e.alg, 'alg') | 0 };
      });
      if (d.timeout !== undefined) out.timeout = unsignedLong(d.timeout);
      out.excludeCredentials = jsonDescriptors(d.excludeCredentials, bin);
      if (d.authenticatorSelection !== undefined) {
        const sel = dict(d.authenticatorSelection, 'authenticatorSelection');
        out.authenticatorSelection = { requireResidentKey: !!sel.requireResidentKey, userVerification: sel.userVerification === undefined ? 'preferred' : str(sel.userVerification) };
        if (sel.authenticatorAttachment !== undefined) out.authenticatorSelection.authenticatorAttachment = str(sel.authenticatorAttachment);
        if (sel.residentKey !== undefined) out.authenticatorSelection.residentKey = str(sel.residentKey);
      }
      out.hints = strings(d.hints, 'hints');
      out.attestation = d.attestation === undefined ? 'none' : str(d.attestation);
      out.attestationFormats = strings(d.attestationFormats, 'attestationFormats');
      if (d.extensions !== undefined) out.extensions = jsonExtensions(d.extensions, bin);
      return out;
    }
    static parseRequestOptionsFromJSON(options) {
      const d = dict(options, 'options');
      const bin = (v, what) => {
        const b = decode(str(v));
        if (b === null) throw dom(`${what} is not valid base64url.`, 'EncodingError');
        return buffer(b);
      };
      const out = {};
      out.challenge = bin(required(d.challenge, 'challenge'), 'challenge');
      if (d.timeout !== undefined) out.timeout = unsignedLong(d.timeout);
      if (d.rpId !== undefined) out.rpId = str(d.rpId);
      out.allowCredentials = jsonDescriptors(d.allowCredentials, bin);
      out.userVerification = d.userVerification === undefined ? 'preferred' : str(d.userVerification);
      out.hints = strings(d.hints, 'hints');
      if (d.extensions !== undefined) out.extensions = jsonExtensions(d.extensions, bin);
      return out;
    }
    static signalUnknownCredential(options) {
      try {
        const d = dict(options, 'options');
        const rpId = str(required(d.rpId, 'rpId'));
        const credentialId = str(required(d.credentialId, 'credentialId'));
        return signalRequest(rpId, [['credentialId', credentialId]], { kind: 'unknown', credentialId });
      } catch (e) {
        return reject(e);
      }
    }
    static signalAllAcceptedCredentials(options) {
      try {
        const d = dict(options, 'options');
        const rpId = str(required(d.rpId, 'rpId'));
        const userId = str(required(d.userId, 'userId'));
        const accepted = sequence(required(d.allAcceptedCredentialIds, 'allAcceptedCredentialIds'), 'allAcceptedCredentialIds').map(str);
        return signalRequest(rpId, [['userId', userId], ...accepted.map((id) => ['credential id', id])], { kind: 'all', userId, accepted });
      } catch (e) {
        return reject(e);
      }
    }
    static signalCurrentUserDetails(options) {
      try {
        const d = dict(options, 'options');
        const rpId = str(required(d.rpId, 'rpId'));
        const userId = str(required(d.userId, 'userId'));
        const name = str(required(d.name, 'name'));
        const displayName = str(required(d.displayName, 'displayName'));
        return signalRequest(rpId, [['userId', userId]], { kind: 'details', userId, name, displayName });
      } catch (e) {
        return reject(e);
      }
    }
  }
  tag(PublicKeyCredential, 'PublicKeyCredential');

  // The Signal API: the arguments are base64url (TypeError otherwise), the RP ID must be the origin's; the
  // context's authenticators then drop or update their discoverable credentials for the user.
  const signalRequest = (rpId, encoded, request) => {
    const inactive = activeOrReject();
    if (inactive !== null) return reject(inactive);
    for (const [name, value] of encoded) {
      if (decode(value) === null) throw typeError(`The ${name} is not valid base64url.`);
    }
    const f = info();
    request.rpId = rpIdOf(rpId, f);
    native.signal(JSONC.stringify(request));
    return resolve(undefined);
  };
  const jsonDescriptors = (v, bin) => (v === undefined ? [] : sequence(v, 'credential descriptors').map((item) => {
    const e = dict(item, 'descriptor');
    const out = { type: str(required(e.type, 'type')), id: bin(required(e.id, 'id'), 'id') };
    if (e.transports !== undefined) out.transports = strings(e.transports, 'transports');
    return out;
  }));
  const jsonPrfValues = (v, bin) => {
    const e = dict(v, 'prf values');
    const out = { first: bin(required(e.first, 'first'), 'first') };
    if (e.second !== undefined) out.second = bin(e.second, 'second');
    return out;
  };
  // The JSON form of the extension inputs: the members that are BufferSource are base64url strings.
  const jsonExtensions = (v, bin) => {
    const d = dict(v, 'extensions');
    const out = {};
    for (const k of ['appid', 'appidExclude', 'credentialProtectionPolicy']) if (d[k] !== undefined) out[k] = str(d[k]);
    for (const k of ['hmacCreateSecret', 'enforceCredentialProtectionPolicy', 'minPinLength', 'credProps', 'getCredBlob']) if (d[k] !== undefined) out[k] = !!d[k];
    if (d.credBlob !== undefined) out.credBlob = bin(d.credBlob, 'credBlob');
    if (d.largeBlob !== undefined) {
      const lb = dict(d.largeBlob, 'largeBlob');
      out.largeBlob = {};
      if (lb.support !== undefined) out.largeBlob.support = str(lb.support);
      if (lb.read !== undefined) out.largeBlob.read = !!lb.read;
      if (lb.write !== undefined) out.largeBlob.write = bin(lb.write, 'largeBlob.write');
    }
    if (d.prf !== undefined) {
      const p = dict(d.prf, 'prf');
      out.prf = {};
      if (p.eval !== undefined) out.prf.eval = jsonPrfValues(p.eval, bin);
      if (p.evalByCredential !== undefined) {
        const table = dict(p.evalByCredential, 'evalByCredential');
        out.prf.evalByCredential = {};
        for (const key of O.keys(table)) out.prf.evalByCredential[key] = jsonPrfValues(table[key], bin);
      }
    }
    return out;
  };

  // ---- CredentialsContainer

  const containers = new WeakSet();
  const containerCheck = (self) => { if (!containers.has(self)) throw invocation(); };
  const MEDIATION = ['silent', 'optional', 'conditional', 'required'];
  // CredentialRequestOptions / CredentialCreationOptions: the members in the lexicographic order of
  // WebIDL. `kind`: the credential types the options ask for.
  const readOptions = (options, get) => {
    if (options !== undefined && options !== null && !isObj(options)) throw typeError("The provided value is not of type 'CredentialOptions'");
    const d = options === undefined || options === null ? {} : options;
    const out = { kinds: [], mediation: 'optional', signal: undefined, uiMode: undefined };
    if (d.federated !== undefined) {
      dict(d.federated, 'federated');
      out.federated = d.federated === null ? {} : d.federated;
      out.kinds.push('federated');
    }
    if (d.mediation !== undefined) out.mediation = enumOf(d.mediation, MEDIATION, 'CredentialMediationRequirement');
    if (get) {
      for (const unsupported of ['identity', 'otp', 'digital']) if (d[unsupported] !== undefined) out.kinds.push(unsupported);
      if (d.password !== undefined && d.password) out.kinds.push('password');
    } else if (d.password !== undefined) {
      out.password = passwordInit(d.password);
      out.kinds.push('password');
    }
    if (d.publicKey !== undefined) {
      out.publicKey = get ? requestOptions(d.publicKey) : creationOptions(d.publicKey);
      out.kinds.push('publicKey');
    }
    if (d.signal !== undefined) out.signal = signalOf(d.signal);
    if (get && d.uiMode !== undefined) out.uiMode = str(d.uiMode);
    return out;
  };
  // The checks every operation makes first: fully active, then an abort that already happened.
  const begin = (signal) => {
    const inactive = activeOrReject();
    if (inactive !== null) throw inactive;
    if (signal !== undefined && apply(signalAborted, signal, [])) throw apply(signalReason, signal, []);
  };
  const needTuple = () => {
    if (info().origin === null) throw dom('The origin of the document is opaque.', 'SecurityError');
  };

  class CredentialsContainer {
    constructor(...args) {
      if (args[0] !== token) throw illegal('CredentialsContainer');
      containers.add(this);
    }
    get(options = {}) {
      try {
        containerCheck(this);
        const o = readOptions(options, true);
        begin(o.signal);
        const kinds = o.kinds;
        const federatedPassword = kinds.length === 2 && kinds.includes('federated') && kinds.includes('password');
        if (kinds.length === 0 || (kinds.length > 1 && !federatedPassword) || kinds.some((k) => k === 'identity' || k === 'otp' || k === 'digital')) {
          throw dom('The options name no credential type this browser can request, or an unsupported combination.', 'NotSupportedError');
        }
        needTuple();
        if (kinds.includes('publicKey')) return getPublicKey(o.publicKey, o.signal, o.mediation, o.uiMode);
        // Passwords and federated identities: there is no password manager or identity provider to ask. A
        // request that is not silent needs the user's activation first (it would show the account chooser),
        // then finds nothing.
        if (o.mediation !== 'silent') {
          if (!info().activation) throw notAllowed('A user activation is required to request a password or federated credential.');
          native.consumeActivation();
        }
        return resolve(null);
      } catch (e) {
        return reject(e);
      }
    }
    store(credential) {
      try {
        containerCheck(this);
        if (arguments.length < 1) throw typeError("Failed to execute 'store' on 'CredentialsContainer': 1 argument required, but only 0 present.");
        if (!isObj(credential) || !credentialSlots.has(credential)) throw typeError("Failed to execute 'store' on 'CredentialsContainer': parameter 1 is not of type 'Credential'.");
        const inactive = activeOrReject();
        if (inactive !== null) throw inactive;
        needTuple();
        const type = credentialSlots.get(credential).type;
        if (type !== 'password' && type !== 'federated') throw dom(`Credentials of type '${type}' cannot be stored.`, 'NotSupportedError');
        // No password manager backs this browser: nothing to save the credential to.
        return resolve(undefined);
      } catch (e) {
        return reject(e);
      }
    }
    create(options = {}) {
      try {
        containerCheck(this);
        const o = readOptions(options, false);
        begin(o.signal);
        if (o.kinds.length !== 1) throw dom('The options must name exactly one credential type to create.', 'NotSupportedError');
        needTuple();
        const kind = o.kinds[0];
        if (kind === 'publicKey') return createPublicKey(o.publicKey, o.signal);
        if (kind === 'password') return resolve(new PasswordCredential(o.password));
        return resolve(new FederatedCredential(federatedInit(o.federated)));
      } catch (e) {
        return reject(e);
      }
    }
    preventSilentAccess() {
      try {
        containerCheck(this);
        const inactive = activeOrReject();
        if (inactive !== null) throw inactive;
        needTuple();
        // Silent access is only ever granted by a password manager, which this browser has none of.
        return resolve(undefined);
      } catch (e) {
        return reject(e);
      }
    }
  }
  tag(CredentialsContainer, 'CredentialsContainer');

  // WebIDL: operations and attributes are enumerable.
  for (const C of [Credential, PasswordCredential, FederatedCredential, AuthenticatorResponse, AuthenticatorAttestationResponse, AuthenticatorAssertionResponse, PublicKeyCredential, CredentialsContainer]) {
    for (const target of [C.prototype, C]) {
      for (const k of O.getOwnPropertyNames(target)) {
        if (k === 'constructor' || k === 'prototype' || k === 'length' || k === 'name') continue;
        const d = O.getOwnPropertyDescriptor(target, k);
        d.enumerable = true;
        O.defineProperty(target, k, d);
      }
    }
  }

  return {
    Credential,
    CredentialsContainer,
    PasswordCredential,
    FederatedCredential,
    PublicKeyCredential,
    AuthenticatorResponse,
    AuthenticatorAttestationResponse,
    AuthenticatorAssertionResponse,
    credentials: new CredentialsContainer(token),
  };
});
