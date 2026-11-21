// Web Crypto for one realm, made on its first SubtleCrypto call other than
// digest() or its first read of `CryptoKey`: the WebIDL conversions, algorithm
// normalization and the per-algorithm steps of the Web Crypto API, over the
// byte primitives of crypto_subtle.rs (`N`). Operations run synchronously and
// settle the promise they return, so error order follows the spec's steps.
//
// A CryptoKey keeps its internal slots in a private field: type, extractable,
// the key algorithm (`alg`, plain data), usages and the key material as bytes
// (secret bytes; PKCS #8 or SPKI DER for EC and RSA keys; the 32 raw bytes
// for Ed25519 and X25519). `cloneKey` copies them for structured clone.
((N) => {
  'use strict';
  const token = Symbol('pyro');
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const Uint8ArrayC = Uint8Array;
  const ArrayBufferC = ArrayBuffer;
  const NumberC = Number;
  const apply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const ownDescriptor = Object.getOwnPropertyDescriptor;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const objectKeys = Object.keys;
  const isView = ArrayBuffer.isView;
  const abLength = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const abSlice = ArrayBuffer.prototype.slice;
  const typedProto = Object.getPrototypeOf(Uint8Array.prototype);
  const viewBuffer = Object.getOwnPropertyDescriptor(typedProto, 'buffer').get;
  const viewOffset = Object.getOwnPropertyDescriptor(typedProto, 'byteOffset').get;
  const viewLength = Object.getOwnPropertyDescriptor(typedProto, 'byteLength').get;
  const typedTag = Object.getOwnPropertyDescriptor(typedProto, Symbol.toStringTag).get;
  const dvBuffer = Object.getOwnPropertyDescriptor(DataView.prototype, 'buffer').get;
  const dvOffset = Object.getOwnPropertyDescriptor(DataView.prototype, 'byteOffset').get;
  const dvLength = Object.getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get;
  const u8Slice = Uint8Array.prototype.slice;
  const stringify = JSON.stringify;
  const parseJSON = JSON.parse;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const encode = TextEncoder.prototype.encode;
  const decode = TextDecoder.prototype.decode;
  const iteratorKey = Symbol.iterator;
  const strReplace = String.prototype.replace;
  const fromCharCode = String.fromCharCode;
  const mathTrunc = Math.trunc;

  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const dom = (name, message) => new DOMExceptionC(message, name);
  const notSupported = (message) => dom('NotSupportedError', message);
  const syntax = (message) => dom('SyntaxError', message);
  const data = (message) => dom('DataError', message);
  const access = (message) => dom('InvalidAccessError', message);
  const operation = (message) => dom('OperationError', message);
  const run = (f) => {
    try {
      return PromiseC.resolve(f());
    } catch (e) {
      return PromiseC.reject(e);
    }
  };
  const has = (list, value) => {
    for (let i = 0; i < list.length; i++) if (list[i] === value) return true;
    return false;
  };
  const asciiUpper = (s) => apply(strReplace, s, [/[a-z]/g, (c) => fromCharCode(c.charCodeAt(0) - 32)]);
  const view = (buffer) => new Uint8ArrayC(buffer);

  // ----- WebIDL conversions ------------------------------------------------

  const required = (args, count, method) => {
    if (args.length < count) {
      throw new TypeErrorC(`Failed to execute '${method}' on 'SubtleCrypto': ${count} arguments required, but only ${args.length} present.`);
    }
  };
  // A BufferSource, unconverted: [buffer, offset, length] or undefined. Not
  // [AllowShared]: a SharedArrayBuffer or a view of one is not one.
  const bufferSource = (v) => {
    if (!isObject(v)) return undefined;
    if (isView(v)) {
      const b = apply(typedTag, v, []) !== undefined
        ? [apply(viewBuffer, v, []), apply(viewOffset, v, []), apply(viewLength, v, [])]
        : [apply(dvBuffer, v, []), apply(dvOffset, v, []), apply(dvLength, v, [])];
      try {
        apply(abLength, b[0], []);
      } catch (_) {
        return undefined;
      }
      return b;
    }
    try {
      return [v, 0, apply(abLength, v, [])];
    } catch (_) {
      return undefined;
    }
  };
  const requireBuffer = (v, what) => {
    const b = bufferSource(v);
    if (b === undefined) throw new TypeErrorC(`${what} is not of type '(ArrayBuffer or ArrayBufferView)'`);
    return b;
  };
  // "Get a copy of the bytes held by the buffer source".
  const copy = (b) => {
    let length = 0;
    try {
      length = apply(abLength, b[0], []);
    } catch (_) {
      return new Uint8ArrayC(0);
    }
    if (b[1] + b[2] > length) return new Uint8ArrayC(0);
    return view(apply(abSlice, b[0], [b[1], b[1] + b[2]]));
  };
  const bytesOf = (v, what) => copy(requireBuffer(v, what));
  const toBoolean = (v) => !!v;
  const toString = (v) => `${v}`;
  const enforceRange = (v, max, what) => {
    let n = NumberC(v);
    if (n !== n || n === Infinity || n === -Infinity) throw new TypeErrorC(`${what} is not a finite number`);
    n = mathTrunc(n);
    if (n < 0 || n > max) throw new TypeErrorC(`${what} is outside the accepted range`);
    return n + 0;
  };
  const FORMATS = ['raw', 'raw-secret', 'raw-public', 'raw-private', 'raw-seed', 'spki', 'pkcs8', 'jwk'];
  const USAGES = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey'];
  // These are valid KeyUsage enum values even when the selected algorithm
  // cannot use them. Its own usage checks run after WebIDL conversion.
  const KEM_USAGES = ['encapsulateKey', 'encapsulateBits', 'decapsulateKey', 'decapsulateBits'];
  const keyFormat = (v) => {
    const format = toString(v);
    if (!has(FORMATS, format)) throw new TypeErrorC(`The provided value '${format}' is not a valid enum value of type KeyFormat.`);
    return format;
  };
  const keyUsages = (v) => {
    if (!isObject(v) || typeof v[iteratorKey] !== 'function') throw new TypeErrorC('The provided value cannot be converted to a sequence.');
    const out = [];
    for (const item of v) {
      const usage = toString(item);
      if (!has(USAGES, usage) && !has(KEM_USAGES, usage)) {
        throw new TypeErrorC(`The provided value '${usage}' is not a valid enum value of type KeyUsage.`);
      }
      out.push(usage);
    }
    return out;
  };
  const JWK_MEMBERS = ['alg', 'crv', 'd', 'dp', 'dq', 'e', 'ext', 'k', 'key_ops', 'kty', 'n', 'oth', 'p', 'q', 'qi', 'use', 'x', 'y'];
  const jsonWebKey = (v) => {
    const jwk = { __proto__: null };
    if (v === undefined || v === null) return jwk;
    if (!isObject(v)) throw new TypeErrorC('The provided value is not of type JsonWebKey');
    for (const member of JWK_MEMBERS) {
      const value = v[member];
      if (value === undefined) continue;
      if (member === 'ext') jwk.ext = toBoolean(value);
      else if (member === 'key_ops') jwk.key_ops = sequenceOfStrings(value);
      else if (member === 'oth') jwk.oth = value;
      else jwk[member] = toString(value);
    }
    return jwk;
  };
  const sequenceOfStrings = (v) => {
    if (!isObject(v) || typeof v[iteratorKey] !== 'function') throw new TypeErrorC('The provided value cannot be converted to a sequence.');
    const out = [];
    for (const item of v) out.push(toString(item));
    return out;
  };
  // (BufferSource or JsonWebKey)
  const keyDataUnion = (v) => {
    const b = bufferSource(v);
    if (b !== undefined) return { buffer: b };
    if (v !== undefined && v !== null && !isObject(v)) throw new TypeErrorC("The provided value is not of type '(ArrayBuffer or ArrayBufferView or JsonWebKey)'");
    return { jwk: jsonWebKey(v) };
  };

  // ----- Algorithm normalization -----------------------------------------

  const CHACHA = 'ChaCha20-Poly1305';
  const AES = ['AES-CTR', 'AES-CBC', 'AES-GCM', 'AES-KW'];
  const RSA = ['RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP'];
  const SHA = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
  const map = (names, type) => {
    const out = {};
    for (const name of names) out[name] = type;
    return out;
  };
  const cipher = { [CHACHA]: 'AeadParams', 'RSA-OAEP': 'RsaOaepParams', 'AES-CTR': 'AesCtrParams', 'AES-CBC': 'AesCbcParams', 'AES-GCM': 'AesGcmParams' };
  const signer = { 'RSASSA-PKCS1-v1_5': '', 'RSA-PSS': 'RsaPssParams', ECDSA: 'EcdsaParams', Ed25519: '', HMAC: '' };
  const REGISTRY = {
    encrypt: cipher,
    decrypt: cipher,
    sign: signer,
    verify: signer,
    digest: map(SHA, ''),
    generateKey: {
      ...map(RSA, 'RsaHashedKeyGenParams'), ECDSA: 'EcKeyGenParams', ECDH: 'EcKeyGenParams', Ed25519: '', X25519: '',
      [CHACHA]: '', ...map(AES, 'AesKeyGenParams'), HMAC: 'HmacKeyGenParams',
    },
    deriveBits: { ECDH: 'EcdhKeyDeriveParams', X25519: 'EcdhKeyDeriveParams', HKDF: 'HkdfParams', PBKDF2: 'Pbkdf2Params' },
    importKey: {
      ...map(RSA, 'RsaHashedImportParams'), ECDSA: 'EcKeyImportParams', ECDH: 'EcKeyImportParams', Ed25519: '', X25519: '',
      [CHACHA]: '', ...map(AES, ''), HMAC: 'HmacImportParams', HKDF: '', PBKDF2: '',
    },
    wrapKey: { 'AES-KW': '' },
    unwrapKey: { 'AES-KW': '' },
    'get key length': { [CHACHA]: '', ...map(AES, 'AesDerivedKeyParams'), HMAC: 'HmacImportParams', HKDF: '', PBKDF2: '' },
  };
  const BS = 1, BS_OPT = 2, HASH = 3, ULONG = 4, ULONG_OPT = 5, USHORT = 6, OCTET = 7, OCTET_OPT = 8, STRING = 9, BIGINT = 10, KEY = 11;
  // Members of each dictionary, inherited ones first, then in lexicographic order.
  const DICTIONARIES = {
    AeadParams: [['additionalData', BS_OPT], ['iv', BS], ['tagLength', OCTET_OPT]],
    AesCbcParams: [['iv', BS]],
    AesCtrParams: [['counter', BS], ['length', OCTET]],
    AesGcmParams: [['additionalData', BS_OPT], ['iv', BS], ['tagLength', OCTET_OPT]],
    AesKeyGenParams: [['length', USHORT]],
    AesDerivedKeyParams: [['length', USHORT]],
    HmacImportParams: [['hash', HASH], ['length', ULONG_OPT]],
    HmacKeyGenParams: [['hash', HASH], ['length', ULONG_OPT]],
    RsaHashedKeyGenParams: [['modulusLength', ULONG], ['publicExponent', BIGINT], ['hash', HASH]],
    RsaHashedImportParams: [['hash', HASH]],
    RsaPssParams: [['saltLength', ULONG]],
    RsaOaepParams: [['label', BS_OPT]],
    EcKeyGenParams: [['namedCurve', STRING]],
    EcKeyImportParams: [['namedCurve', STRING]],
    EcdsaParams: [['hash', HASH]],
    EcdhKeyDeriveParams: [['public', KEY]],
    HkdfParams: [['hash', HASH], ['info', BS], ['salt', BS]],
    Pbkdf2Params: [['hash', HASH], ['iterations', ULONG], ['salt', BS]],
  };
  const algorithmName = (alg) => {
    if (!isObject(alg)) throw new TypeErrorC('Algorithm: the provided value is not an object');
    const name = alg.name;
    if (name === undefined) throw new TypeErrorC("Algorithm: required member 'name' is missing");
    return toString(name);
  };
  function normalize(alg, op) {
    if (!isObject(alg)) return normalize({ name: toString(alg) }, op);
    const name = algorithmName(alg);
    const registered = REGISTRY[op];
    const upper = asciiUpper(name);
    let found;
    // Own keys only: page code may add enumerable members to Object.prototype.
    const names = objectKeys(registered);
    for (let i = 0; i < names.length; i++) {
      if (asciiUpper(names[i]) === upper) found = names[i];
    }
    if (found === undefined) throw notSupported(`Algorithm '${name}' is not supported for ${op}`);
    const type = registered[found];
    const out = { name: found };
    if (type === '') return out;
    algorithmName(alg);
    // HashAlgorithmIdentifier members are converted with the dictionary and
    // normalized after it (WebCrypto "normalize an algorithm"), so a missing
    // required member is a TypeError before an unknown hash is NotSupported.
    let hashes;
    for (const [member, kind] of DICTIONARIES[type]) {
      const value = alg[member];
      if (value === undefined) {
        if (kind === BS_OPT || kind === ULONG_OPT || kind === OCTET_OPT) continue;
        throw new TypeErrorC(`${type}: required member '${member}' is missing`);
      }
      const what = `${type}.${member}`;
      switch (kind) {
        case BS: case BS_OPT: out[member] = bytesOf(value, what); break;
        case HASH:
          out[member] = isObject(value) ? value : toString(value);
          (hashes || (hashes = [])).push(member);
          break;
        case ULONG: case ULONG_OPT: out[member] = enforceRange(value, 0xffffffff, what); break;
        case USHORT: out[member] = enforceRange(value, 0xffff, what); break;
        case OCTET: case OCTET_OPT: out[member] = enforceRange(value, 0xff, what); break;
        case STRING: out[member] = toString(value); break;
        case BIGINT:
          if (!isView(value) || apply(typedTag, value, []) !== 'Uint8Array') throw new TypeErrorC(`${what} is not of type 'Uint8Array'`);
          out[member] = apply(u8Slice, value, []);
          break;
        case KEY:
          if (slotsOf(value) === undefined) throw new TypeErrorC(`${what} is not of type 'CryptoKey'`);
          out[member] = value;
          break;
      }
    }
    if (hashes) for (let i = 0; i < hashes.length; i++) out[hashes[i]] = normalize(out[hashes[i]], 'digest');
    return out;
  }
  // wrapKey/unwrapKey: the algorithm's own operation, else encrypt/decrypt.
  const normalizeWrap = (alg, op, fallback) => {
    try {
      return normalize(alg, op);
    } catch (_) {
      return normalize(alg, fallback);
    }
  };

  // ----- CryptoKey ---------------------------------------------------------

  let slotsOf;
  class CryptoKey {
    #k;
    // A rest parameter keeps the interface object's length at 0.
    constructor(...a) {
      if (a[0] !== token) throw new TypeErrorC('Illegal constructor');
      this.#k = a[1];
    }
    get type() {
      return this.#k.type;
    }
    get extractable() {
      return this.#k.extractable;
    }
    get algorithm() {
      return this.#k.algorithm;
    }
    get usages() {
      return this.#k.usagesObject;
    }
    static {
      slotsOf = (v) => (isObject(v) && #k in v ? v.#k : undefined);
    }
  }
  for (const key of getOwnPropertyNames(CryptoKey.prototype)) {
    if (key === 'constructor') continue;
    const desc = Object.getOwnPropertyDescriptor(CryptoKey.prototype, key);
    desc.enumerable = true;
    defineProperty(CryptoKey.prototype, key, desc);
  }
  defineProperty(CryptoKey.prototype, Symbol.toStringTag, { value: 'CryptoKey', configurable: true });

  // The `algorithm` attribute's object, made once per key.
  const algorithmObject = (alg) => {
    const out = { name: alg.name };
    if (alg.length !== undefined) out.length = alg.length;
    if (alg.namedCurve !== undefined) out.namedCurve = alg.namedCurve;
    if (alg.modulusLength !== undefined) {
      out.modulusLength = alg.modulusLength;
      out.publicExponent = apply(u8Slice, alg.publicExponent, []);
    }
    if (alg.hash !== undefined) out.hash = { name: alg.hash };
    return out;
  };
  const canonicalUsages = (usages) => {
    const out = [];
    for (const usage of USAGES) if (has(usages, usage)) out.push(usage);
    return out;
  };
  const makeKey = (type, extractable, alg, usages, material) => {
    const ordered = canonicalUsages(usages);
    return new CryptoKey(token, {
      type, extractable, alg, usages: ordered, material,
      algorithm: algorithmObject(alg), usagesObject: [...ordered],
    });
  };
  const keyArg = (v, what) => {
    const k = slotsOf(v);
    if (k === undefined) throw new TypeErrorC(`${what} is not of type 'CryptoKey'`);
    return k;
  };
  const checkKey = (k, normalized, usage) => {
    if (normalized.name !== k.alg.name) throw access(`The key is not a ${normalized.name} key`);
    if (!has(k.usages, usage)) throw access(`The key does not allow '${usage}'`);
  };
  const checkUsages = (usages, allowed) => {
    for (const usage of usages) if (!has(allowed, usage)) throw syntax(`Cannot create a key with usage '${usage}'`);
  };

  // ----- Per-algorithm steps ---------------------------------------------

  const asymKind = (name) => {
    if (name === 'ECDSA' || name === 'ECDH') return 'EC';
    if (name === 'Ed25519' || name === 'X25519') return name;
    return 'RSA';
  };
  const SIGN_KEY = { public: ['verify'], private: ['sign'] };
  const CRYPT_KEY = { public: ['encrypt', 'wrapKey'], private: ['decrypt', 'unwrapKey'] };
  const DERIVE_KEY = { public: [], private: ['deriveKey', 'deriveBits'] };
  const pairUsages = (name) => {
    if (name === 'RSA-OAEP') return CRYPT_KEY;
    if (name === 'ECDH' || name === 'X25519') return DERIVE_KEY;
    return SIGN_KEY;
  };
  const SECRET_USAGES = {
    [CHACHA]: ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'],
    'AES-CTR': ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'],
    'AES-CBC': ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'],
    'AES-GCM': ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'],
    'AES-KW': ['wrapKey', 'unwrapKey'],
    HMAC: ['sign', 'verify'],
    HKDF: ['deriveKey', 'deriveBits'],
    PBKDF2: ['deriveKey', 'deriveBits'],
  };
  const CURVES = ['P-256', 'P-384', 'P-521'];
  const hmacBlockBits = (hash) => (hash === 'SHA-384' || hash === 'SHA-512' ? 1024 : 512);
  const JWK_HASH = { 'SHA-1': '1', 'SHA-256': '256', 'SHA-384': '384', 'SHA-512': '512' };
  const rsaJwkAlg = (name, hash) => {
    if (name === 'RSA-OAEP') return hash === 'SHA-1' ? 'RSA-OAEP' : `RSA-OAEP-${JWK_HASH[hash]}`;
    return (name === 'RSA-PSS' ? 'PS' : 'RS') + JWK_HASH[hash];
  };
  const ES_ALG = { 'P-256': 'ES256', 'P-384': 'ES384', 'P-521': 'ES512' };

  // The checks every JWK import makes after its key-type specific ones.
  const jwkCommon = (jwk, usages, extractable, use) => {
    if (usages.length > 0 && jwk.use !== undefined && jwk.use !== use) throw data(`The JWK 'use' member must be '${use}'`);
    if (jwk.key_ops !== undefined) {
      const ops = jwk.key_ops;
      for (let i = 0; i < ops.length; i++) {
        for (let j = 0; j < i; j++) if (ops[j] === ops[i]) throw data("The JWK 'key_ops' member has duplicates");
      }
      for (const usage of usages) if (!has(ops, usage)) throw data(`The JWK 'key_ops' member does not allow '${usage}'`);
    }
    if (jwk.ext === false && extractable) throw data("The JWK 'ext' member is false but the key is extractable");
  };
  const b64urlDecode = (text) => {
    if (!/^[A-Za-z0-9_-]*$/.test(text)) throw data('The JWK member is not valid base64url');
    let s = apply(strReplace, apply(strReplace, text, [/-/g, '+']), [/_/g, '/']);
    while (s.length % 4) s += '=';
    let bin;
    try {
      bin = atobFn(s);
    } catch (_) {
      throw data('The JWK member is not valid base64url');
    }
    const out = new Uint8ArrayC(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  };
  const b64urlEncode = (bytes) => {
    let bin = '';
    for (let i = 0; i < bytes.length; i++) bin += fromCharCode(bytes[i]);
    return apply(strReplace, apply(strReplace, apply(strReplace, btoaFn(bin), [/\+/g, '-']), [/\//g, '_']), [/=+$/, '']);
  };
  const atobFn = atob;
  const btoaFn = btoa;

  function importSecret(format, keyData, normalized, extractable, usages) {
    const name = normalized.name;
    if ((name === 'HKDF' || name === 'PBKDF2') && format !== 'raw') throw notSupported(`${name} keys can only be imported as 'raw'`);
    checkUsages(usages, SECRET_USAGES[name]);
    if (name === 'HKDF' || name === 'PBKDF2') {
      if (extractable) throw syntax(`${name} keys cannot be extractable`);
      return makeKey('secret', false, { name }, usages, copy(keyData.buffer));
    }
    let bytes;
    if (format === (name === CHACHA ? 'raw-secret' : 'raw')) {
      bytes = copy(keyData.buffer);
    } else if (format === 'jwk') {
      const jwk = keyData.jwk;
      if (jwk.kty !== 'oct') throw data("The JWK 'kty' member must be 'oct'");
      if (jwk.k === undefined) throw data("The JWK 'k' member is missing");
      bytes = b64urlDecode(jwk.k);
      if (name === CHACHA) {
        if (bytes.length !== 32) throw data('ChaCha20-Poly1305 keys must be 256 bits');
        if (jwk.alg !== undefined && jwk.alg !== 'C20P') throw data("The JWK 'alg' member must be 'C20P'");
      } else if (name === 'HMAC') {
        if (jwk.alg !== undefined && jwk.alg !== `HS${JWK_HASH[normalized.hash.name]}`) throw data("The JWK 'alg' member does not match the hash");
      } else {
        const bits = bytes.length * 8;
        if (bits !== 128 && bits !== 192 && bits !== 256) throw data('AES key data must be 128, 192 or 256 bits');
        if (jwk.alg !== undefined && jwk.alg !== `A${bits}${name.slice(4)}`) throw data("The JWK 'alg' member does not match the key");
      }
      jwkCommon(jwk, usages, extractable, name === 'HMAC' ? 'sig' : 'enc');
    } else {
      throw notSupported(`${name} keys cannot be imported as '${format}'`);
    }
    if (name === CHACHA) {
      if (bytes.length !== 32) throw data('ChaCha20-Poly1305 keys must be 256 bits');
      return makeKey('secret', extractable, {name}, usages, bytes);
    }
    if (name === 'HMAC') {
      const bits = bytes.length * 8;
      if (bits === 0) throw data('HMAC key data must not be empty');
      let length = bits;
      if (normalized.length !== undefined) {
        if (normalized.length > bits || normalized.length <= bits - 8) throw data('The HMAC length does not match the key data');
        length = normalized.length;
      }
      return makeKey('secret', extractable, { name, length, hash: normalized.hash.name }, usages, bytes);
    }
    const bits = bytes.length * 8;
    if (bits !== 128 && bits !== 192 && bits !== 256) throw data('AES key data must be 128, 192 or 256 bits');
    return makeKey('secret', extractable, { name, length: bits }, usages, bytes);
  }

  // The JWK members importAsym takes, in its order, after the key-type checks.
  function asymJwk(jwk, normalized, extractable, usages) {
    const name = normalized.name;
    const kind = asymKind(name);
    const allowed = pairUsages(name);
    checkUsages(usages, jwk.d !== undefined ? allowed.private : allowed.public);
    if (kind === 'EC') {
      if (jwk.kty !== 'EC') throw data("The JWK 'kty' member must be 'EC'");
      if (jwk.crv !== normalized.namedCurve) throw data("The JWK 'crv' member does not match the namedCurve");
      if (name === 'ECDSA' && jwk.alg !== undefined && jwk.alg !== ES_ALG[normalized.namedCurve]) throw data("The JWK 'alg' member does not match the curve");
      jwkCommon(jwk, usages, extractable, name === 'ECDSA' ? 'sig' : 'enc');
      return [jwk.d, jwk.x, jwk.y];
    }
    if (kind === 'RSA') {
      if (jwk.kty !== 'RSA') throw data("The JWK 'kty' member must be 'RSA'");
      if (jwk.alg !== undefined && jwk.alg !== rsaJwkAlg(name, normalized.hash.name)) throw data("The JWK 'alg' member does not match the hash");
      jwkCommon(jwk, usages, extractable, name === 'RSA-OAEP' ? 'enc' : 'sig');
      if (jwk.oth !== undefined) throw notSupported('RSA keys with more than two primes are not supported');
      return [jwk.n, jwk.e, jwk.d, jwk.p, jwk.q, jwk.dp, jwk.dq, jwk.qi];
    }
    if (jwk.kty !== 'OKP') throw data("The JWK 'kty' member must be 'OKP'");
    if (jwk.crv !== name) throw data(`The JWK 'crv' member must be '${name}'`);
    if (name === 'Ed25519' && jwk.alg !== undefined && jwk.alg !== 'Ed25519' && jwk.alg !== 'EdDSA') throw data("The JWK 'alg' member must be 'Ed25519' or 'EdDSA'");
    jwkCommon(jwk, usages, extractable, name === 'Ed25519' ? 'sig' : 'enc');
    return [jwk.d, jwk.x];
  }

  function importAsym(format, keyData, normalized, extractable, usages) {
    const name = normalized.name;
    const kind = asymKind(name);
    const allowed = pairUsages(name);
    if (kind === 'EC' && !has(CURVES, normalized.namedCurve)) throw notSupported(`Unsupported namedCurve '${normalized.namedCurve}'`);
    let input;
    if (format === 'jwk') {
      input = asymJwk(keyData.jwk, normalized, extractable, usages);
    } else {
      if (format !== 'raw' && format !== 'pkcs8' && format !== 'spki') throw notSupported(`${name} keys cannot be imported as '${format}'`);
      if (format === 'raw' && kind === 'RSA') throw notSupported('RSA keys cannot be imported as raw');
      checkUsages(usages, format === 'pkcs8' ? allowed.private : allowed.public);
      input = copy(keyData.buffer);
    }
    const result = N.importAsym(kind, normalized.namedCurve || '', format, input);
    const type = result[0];
    const alg = { name };
    if (kind === 'EC') alg.namedCurve = normalized.namedCurve;
    if (kind === 'RSA') {
      alg.modulusLength = result[2];
      alg.publicExponent = view(result[3]);
      alg.hash = normalized.hash.name;
    }
    return makeKey(type, extractable, alg, usages, view(result[1]));
  }

  // Modern Web Crypto names raw formats by key type. Classic symmetric
  // and asymmetric algorithms retain raw as an alias for their existing format.
  function resolveKeyFormat(format, name) {
    if (format === 'raw-secret' && name !== CHACHA && SECRET_USAGES[name]) return 'raw';
    if (format === 'raw-public' && !SECRET_USAGES[name]) return 'raw';
    return format;
  }
  function importKeyStep(format, keyData, normalized, extractable, usages) {
    const name = normalized.name;
    if (format === 'jwk' ? keyData.jwk === undefined : keyData.buffer === undefined) {
      throw new TypeErrorC(format === 'jwk' ? 'Key data must be a JsonWebKey for the jwk format' : 'Key data must be a BufferSource for this format');
    }
    format = resolveKeyFormat(format, name);
    const key = SECRET_USAGES[name] ? importSecret(format, keyData, normalized, extractable, usages) : importAsym(format, keyData, normalized, extractable, usages);
    const k = slotsOf(key);
    if ((k.type === 'secret' || k.type === 'private') && k.usages.length === 0) throw syntax('Usages cannot be empty when creating a key.');
    return key;
  }

  function exportKeyStep(format, k) {
    const name = k.alg.name;
    if (name === 'HKDF' || name === 'PBKDF2') throw notSupported(`${name} keys cannot be exported`);
    if (!k.extractable) throw access('The key is not extractable');
    format = resolveKeyFormat(format, name);
    if (k.type === 'secret') {
      if (format === (name === CHACHA ? 'raw-secret' : 'raw')) return copyOut(k.material);
      if (format !== 'jwk') throw notSupported(`${name} keys cannot be exported as '${format}'`);
      let alg;
      if (name === CHACHA) alg = 'C20P';
      else if (name === 'HMAC') alg = `HS${JWK_HASH[k.alg.hash]}`;
      else alg = `A${k.material.length * 8}${name.slice(4)}`;
      return { alg, ext: k.extractable, k: b64urlEncode(k.material), key_ops: [...k.usages], kty: 'oct' };
    }
    const kind = asymKind(name);
    if (format !== 'raw' && format !== 'pkcs8' && format !== 'spki' && format !== 'jwk') throw notSupported(`${name} keys cannot be exported as '${format}'`);
    if (format === 'raw' && kind === 'RSA') throw notSupported('RSA keys cannot be exported as raw');
    if ((format === 'spki' || format === 'raw') && k.type !== 'public') throw access(`Only public keys can be exported as '${format}'`);
    if (format === 'pkcs8' && k.type !== 'private') throw access("Only private keys can be exported as 'pkcs8'");
    const out = N.exportAsym(kind, k.alg.namedCurve || '', k.type, format, k.material);
    if (format !== 'jwk') return out;
    const ops = [...k.usages];
    if (kind === 'EC') {
      const jwk = { crv: k.alg.namedCurve };
      if (k.type === 'private') jwk.d = out[0];
      return assign(jwk, { ext: k.extractable, key_ops: ops, kty: 'EC', x: out[k.type === 'private' ? 1 : 0], y: out[k.type === 'private' ? 2 : 1] });
    }
    if (kind === 'RSA') {
      const jwk = { alg: rsaJwkAlg(name, k.alg.hash) };
      if (k.type === 'private') {
        return assign(jwk, { d: out[2], dp: out[5], dq: out[6], e: out[1], ext: k.extractable, key_ops: ops, kty: 'RSA', n: out[0], p: out[3], q: out[4], qi: out[7] });
      }
      return assign(jwk, { e: out[1], ext: k.extractable, key_ops: ops, kty: 'RSA', n: out[0] });
    }
    const jwk = name === 'Ed25519' ? { alg: 'Ed25519', crv: name } : { crv: name };
    if (k.type === 'private') jwk.d = out[0];
    return assign(jwk, { ext: k.extractable, key_ops: ops, kty: 'OKP', x: out[k.type === 'private' ? 1 : 0] });
  }
  const assign = (target, source) => {
    for (const key in source) target[key] = source[key];
    return target;
  };
  const copyOut = (bytes) => apply(u8Slice, bytes, []).buffer;

  function generateKeyStep(normalized, extractable, usages) {
    const name = normalized.name;
    if (SECRET_USAGES[name]) {
      checkUsages(usages, SECRET_USAGES[name]);
      let alg, bytes;
      if (name === CHACHA) {
        alg = {name};
        bytes = view(N.random(32));
      } else if (name === 'HMAC') {
        let length = normalized.length;
        if (length === undefined) length = hmacBlockBits(normalized.hash.name);
        else if (length === 0) throw operation('The HMAC key length must not be zero');
        alg = { name, length, hash: normalized.hash.name };
        bytes = view(N.random((length + 7) >> 3));
      } else {
        const length = normalized.length;
        if (length !== 128 && length !== 192 && length !== 256) throw operation('AES key length must be 128, 192 or 256 bits');
        alg = { name, length };
        bytes = view(N.random(length >> 3));
      }
      if (usages.length === 0) throw syntax('Usages cannot be empty when creating a key.');
      return makeKey('secret', extractable, alg, usages, bytes);
    }
    const kind = asymKind(name);
    const allowed = pairUsages(name);
    checkUsages(usages, [...allowed.public, ...allowed.private]);
    const alg = { name };
    if (kind === 'EC') {
      if (!has(CURVES, normalized.namedCurve)) throw notSupported(`Unsupported namedCurve '${normalized.namedCurve}'`);
      alg.namedCurve = normalized.namedCurve;
    }
    if (kind === 'RSA') {
      alg.modulusLength = normalized.modulusLength;
      alg.publicExponent = normalized.publicExponent;
      alg.hash = normalized.hash.name;
    }
    const pair = N.generateAsym(kind, alg.namedCurve || '', normalized.modulusLength || 0, normalized.publicExponent);
    const privateUsages = usages.filter((u) => has(allowed.private, u));
    if (privateUsages.length === 0) throw syntax('Usages cannot be empty when creating a key.');
    return {
      privateKey: makeKey('private', extractable, alg, privateUsages, view(pair[0])),
      publicKey: makeKey('public', true, alg, usages.filter((u) => has(allowed.public, u)), view(pair[1])),
    };
  }

  const TAG_LENGTHS = [32, 64, 96, 104, 112, 120, 128];
  function cryptStep(encrypt, normalized, k, bytes) {
    const name = normalized.name;
    switch (name) {
      case CHACHA:
        if (normalized.iv.length !== 12) throw operation('ChaCha20-Poly1305 requires a 96-bit IV');
        if (normalized.tagLength !== undefined && normalized.tagLength !== 128) throw operation('ChaCha20-Poly1305 requires a 128-bit tag');
        return N.chacha20Poly1305(encrypt, k.material, normalized.iv, bytes, normalized.additionalData || new Uint8ArrayC(0));
      case 'AES-CBC':
        if (normalized.iv.length !== 16) throw operation('The AES-CBC iv must be 16 bytes');
        return N.aesCbc(encrypt, k.material, normalized.iv, bytes);
      case 'AES-CTR':
        if (normalized.counter.length !== 16) throw operation('The AES-CTR counter must be 16 bytes');
        if (normalized.length === 0 || normalized.length > 128) throw operation('The AES-CTR length must be between 1 and 128');
        return N.aesCtr(k.material, normalized.counter, normalized.length, bytes);
      case 'AES-GCM': {
        const tagLength = normalized.tagLength === undefined ? 128 : normalized.tagLength;
        if (!has(TAG_LENGTHS, tagLength)) throw operation(`${tagLength} is not a valid AES-GCM tag length`);
        return N.aesGcm(encrypt, k.material, normalized.iv, bytes, normalized.additionalData || new Uint8ArrayC(0), tagLength >> 3);
      }
      case 'RSA-OAEP':
        if (k.type !== (encrypt ? 'public' : 'private')) throw access(`RSA-OAEP ${encrypt ? 'encryption' : 'decryption'} needs a ${encrypt ? 'public' : 'private'} key`);
        return N.rsaOaep(encrypt, k.alg.hash, normalized.label || new Uint8ArrayC(0), k.material, bytes);
    }
    throw notSupported(`${name} cannot ${encrypt ? 'encrypt' : 'decrypt'}`);
  }

  const signKind = (name) => (name === 'RSASSA-PKCS1-v1_5' ? 'PKCS1' : name === 'RSA-PSS' ? 'PSS' : asymKind(name));
  function signStep(normalized, k, bytes) {
    const name = normalized.name;
    if (name === 'HMAC') return N.hmac(k.alg.hash, k.material, bytes);
    if (k.type !== 'private') throw access('Signing needs a private key');
    const hash = name === 'ECDSA' ? normalized.hash.name : k.alg.hash;
    return N.signAsym(signKind(name), k.alg.namedCurve || '', hash, normalized.saltLength || 0, k.material, bytes);
  }
  function verifyStep(normalized, k, signature, bytes) {
    const name = normalized.name;
    if (name === 'HMAC') return N.hmac(k.alg.hash, k.material, bytes, signature);
    if (k.type !== 'public') throw access('Verifying needs a public key');
    const hash = name === 'ECDSA' ? normalized.hash.name : k.alg.hash;
    return N.verifyAsym(signKind(name), k.alg.namedCurve || '', hash, normalized.saltLength || 0, k.material, signature, bytes);
  }

  const truncateBits = (buffer, length) => {
    const bytes = view(buffer);
    if (length === null) return buffer;
    if (length > bytes.length * 8) throw operation('The length is larger than the derived secret');
    const out = apply(u8Slice, bytes, [0, (length + 7) >> 3]);
    if (length % 8) out[out.length - 1] &= 0xff << (8 - (length % 8));
    return out.buffer;
  };
  function deriveBitsStep(normalized, k, length) {
    const name = normalized.name;
    if (name === 'HKDF' || name === 'PBKDF2') {
      if (length === null || length % 8 !== 0) throw operation('The length must be a multiple of 8');
      if (name === 'HKDF') return N.hkdf(normalized.hash.name, k.material, normalized.salt, normalized.info, length >> 3);
      if (normalized.iterations === 0) throw operation('The PBKDF2 iterations must not be zero');
      return N.pbkdf2(normalized.hash.name, k.material, normalized.salt, normalized.iterations, length >> 3);
    }
    const peer = slotsOf(normalized.public);
    if (peer.type !== 'public') throw access('The public member must be a public key');
    if (peer.alg.name !== k.alg.name) throw access('The public key is not of the same algorithm');
    if (peer.alg.namedCurve !== k.alg.namedCurve) throw access('The public key is not on the same curve');
    if (k.type !== 'private') throw access('The base key must be a private key');
    return truncateBits(N.deriveAsym(asymKind(name), k.alg.namedCurve || '', k.material, peer.material), length);
  }
  function keyLength(normalized) {
    const name = normalized.name;
    if (name === CHACHA) return 256;
    if (name === 'HKDF' || name === 'PBKDF2') return null;
    if (name === 'HMAC') {
      if (normalized.length === undefined) return hmacBlockBits(normalized.hash.name);
      if (normalized.length === 0) throw new TypeErrorC('The HMAC length must not be zero');
      return normalized.length;
    }
    if (normalized.length !== 128 && normalized.length !== 192 && normalized.length !== 256) throw operation('AES key length must be 128, 192 or 256 bits');
    return normalized.length;
  }
  const optionalLength = (v) => (v === undefined || v === null ? null : enforceRange(v, 0xffffffff, 'length'));

  // ----- SubtleCrypto methods ----------------------------------------------

  const methods = {
    encrypt(args) {
      required(args, 3, 'encrypt');
      const [algorithm, keyValue, dataValue] = args;
      const k = keyArg(keyValue, 'parameter 2');
      const source = requireBuffer(dataValue, 'parameter 3');
      const normalized = normalize(algorithm, 'encrypt');
      const bytes = copy(source);
      checkKey(k, normalized, 'encrypt');
      return cryptStep(true, normalized, k, bytes);
    },
    decrypt(args) {
      required(args, 3, 'decrypt');
      const [algorithm, keyValue, dataValue] = args;
      const k = keyArg(keyValue, 'parameter 2');
      const source = requireBuffer(dataValue, 'parameter 3');
      const normalized = normalize(algorithm, 'decrypt');
      const bytes = copy(source);
      checkKey(k, normalized, 'decrypt');
      return cryptStep(false, normalized, k, bytes);
    },
    sign(args) {
      required(args, 3, 'sign');
      const [algorithm, keyValue, dataValue] = args;
      const k = keyArg(keyValue, 'parameter 2');
      const source = requireBuffer(dataValue, 'parameter 3');
      const normalized = normalize(algorithm, 'sign');
      const bytes = copy(source);
      checkKey(k, normalized, 'sign');
      return signStep(normalized, k, bytes);
    },
    verify(args) {
      required(args, 4, 'verify');
      const [algorithm, keyValue, signatureValue, dataValue] = args;
      const k = keyArg(keyValue, 'parameter 2');
      const signatureSource = requireBuffer(signatureValue, 'parameter 3');
      const source = requireBuffer(dataValue, 'parameter 4');
      const normalized = normalize(algorithm, 'verify');
      const signature = copy(signatureSource);
      const bytes = copy(source);
      checkKey(k, normalized, 'verify');
      return verifyStep(normalized, k, signature, bytes);
    },
    generateKey(args) {
      required(args, 3, 'generateKey');
      const [algorithm, extractableValue, usagesValue] = args;
      const extractable = toBoolean(extractableValue);
      const usages = keyUsages(usagesValue);
      return generateKeyStep(normalize(algorithm, 'generateKey'), extractable, usages);
    },
    deriveKey(args) {
      required(args, 5, 'deriveKey');
      const [algorithm, baseKey, derivedKeyType, extractableValue, usagesValue] = args;
      const k = keyArg(baseKey, 'parameter 2');
      const extractable = toBoolean(extractableValue);
      const usages = keyUsages(usagesValue);
      const normalized = normalize(algorithm, 'deriveBits');
      const importAlgorithm = normalize(derivedKeyType, 'importKey');
      const lengthAlgorithm = normalize(derivedKeyType, 'get key length');
      checkKey(k, normalized, 'deriveKey');
      const length = keyLength(lengthAlgorithm);
      const secret = deriveBitsStep(normalized, k, length);
      return importKeyStep('raw-secret', { buffer: [secret, 0, apply(abLength, secret, [])] }, importAlgorithm, extractable, usages);
    },
    deriveBits(args) {
      required(args, 2, 'deriveBits');
      const [algorithm, baseKey, lengthValue] = args;
      const k = keyArg(baseKey, 'parameter 2');
      const length = optionalLength(lengthValue);
      const normalized = normalize(algorithm, 'deriveBits');
      checkKey(k, normalized, 'deriveBits');
      return deriveBitsStep(normalized, k, length);
    },
    importKey(args) {
      required(args, 5, 'importKey');
      const [formatValue, keyDataValue, algorithm, extractableValue, usagesValue] = args;
      const format = keyFormat(formatValue);
      const keyData = keyDataUnion(keyDataValue);
      const extractable = toBoolean(extractableValue);
      const usages = keyUsages(usagesValue);
      const normalized = normalize(algorithm, 'importKey');
      return importKeyStep(format, keyData, normalized, extractable, usages);
    },
    exportKey(args) {
      required(args, 2, 'exportKey');
      const format = keyFormat(args[0]);
      return exportKeyStep(format, keyArg(args[1], 'parameter 2'));
    },
    wrapKey(args) {
      required(args, 4, 'wrapKey');
      const [formatValue, keyValue, wrappingKeyValue, algorithm] = args;
      const format = keyFormat(formatValue);
      const k = keyArg(keyValue, 'parameter 2');
      const wrapping = keyArg(wrappingKeyValue, 'parameter 3');
      const normalized = normalizeWrap(algorithm, 'wrapKey', 'encrypt');
      checkKey(wrapping, normalized, 'wrapKey');
      const exported = exportKeyStep(format, k);
      const bytes = format === 'jwk' ? apply(encode, encoder, [stringify(exported)]) : view(exported);
      if (normalized.name === 'AES-KW') return N.aesKw(true, wrapping.material, bytes);
      return cryptStep(true, normalized, wrapping, bytes);
    },
    unwrapKey(args) {
      required(args, 7, 'unwrapKey');
      const [formatValue, wrappedValue, unwrappingKeyValue, algorithm, keyAlgorithm, extractableValue, usagesValue] = args;
      const format = keyFormat(formatValue);
      const wrappedSource = requireBuffer(wrappedValue, 'parameter 2');
      const unwrapping = keyArg(unwrappingKeyValue, 'parameter 3');
      const extractable = toBoolean(extractableValue);
      const usages = keyUsages(usagesValue);
      const normalized = normalizeWrap(algorithm, 'unwrapKey', 'decrypt');
      const importAlgorithm = normalize(keyAlgorithm, 'importKey');
      const wrapped = copy(wrappedSource);
      checkKey(unwrapping, normalized, 'unwrapKey');
      const bytes = normalized.name === 'AES-KW' ? N.aesKw(false, unwrapping.material, wrapped) : cryptStep(false, normalized, unwrapping, wrapped);
      let keyData;
      if (format === 'jwk') {
        let parsed;
        try {
          parsed = parseJSON(apply(decode, decoder, [bytes]));
        } catch (_) {
          throw data('The unwrapped key is not a JSON Web Key');
        }
        if (!isObject(parsed) || parsed.kty === undefined) throw data('The unwrapped key is not a JSON Web Key');
        keyData = { jwk: jsonWebKey(parsed) };
      } else {
        keyData = { buffer: [bytes, 0, apply(abLength, bytes, [])] };
      }
      return importKeyStep(format, keyData, importAlgorithm, extractable, usages);
    },
  };

  return {
    CryptoKey,
    // SubtleCrypto.<name>(...args) as a promise; argument errors reject it.
    call: (name, args) => run(() => methods[name](args)),
    // Structured clone: a copy of a CryptoKey of this realm, else undefined.
    cloneKey(value) {
      const k = slotsOf(value);
      if (k === undefined) return undefined;
      const alg = { ...k.alg };
      if (alg.publicExponent !== undefined) alg.publicExponent = apply(u8Slice, alg.publicExponent, []);
      return makeKey(k.type, k.extractable, alg, k.usages, apply(u8Slice, k.material, []));
    },
    // The cross-VM form of a CryptoKey of this realm (plain data), else undefined.
    serializeKey(value) {
      const k = slotsOf(value);
      if (k === undefined) return undefined;
      const alg = { name: k.alg.name };
      for (const field of ['length', 'namedCurve', 'modulusLength', 'hash']) {
        if (k.alg[field] !== undefined) alg[field] = k.alg[field];
      }
      if (k.alg.publicExponent !== undefined) alg.publicExponent = apply(u8Slice, k.alg.publicExponent, []);
      return [k.type, k.extractable, alg, [...k.usages], apply(u8Slice, k.material, [])];
    },
    // A CryptoKey of this realm from `serializeKey`'s parts (made by
    // another VM's copy of this script, so only their shape is checked).
    deserializeKey(parts) {
      const bad = () => new DOMExceptionC('Malformed CryptoKey', 'DataCloneError');
      if (!isObject(parts) || parts.length !== 5) throw bad();
      const type = parts[0], extractable = parts[1], source = parts[2], usages = parts[3], material = parts[4];
      if (type !== 'secret' && type !== 'public' && type !== 'private') throw bad();
      if (typeof extractable !== 'boolean' || !isObject(source)) throw bad();
      const named = ownDescriptor(source, 'name');
      if (!named || typeof named.value !== 'string') throw bad();
      if (!isObject(usages) || !(material instanceof Uint8ArrayC)) throw bad();
      const list = [];
      for (let i = 0; i < usages.length; i++) {
        if (!has(USAGES, usages[i])) throw bad();
        list.push(usages[i]);
      }
      // Own data properties only: nothing the receiving realm's prototypes add.
      const field = (key) => {
        const d = ownDescriptor(source, key);
        return d && 'value' in d ? d.value : undefined;
      };
      const alg = { name: named.value };
      if (typeof field('length') === 'number') alg.length = field('length');
      if (typeof field('namedCurve') === 'string') alg.namedCurve = field('namedCurve');
      if (typeof field('modulusLength') === 'number') alg.modulusLength = field('modulusLength');
      if (typeof field('hash') === 'string') alg.hash = field('hash');
      if (field('publicExponent') instanceof Uint8ArrayC) alg.publicExponent = field('publicExponent');
      return makeKey(type, extractable, alg, list, material);
    },
  };
})
