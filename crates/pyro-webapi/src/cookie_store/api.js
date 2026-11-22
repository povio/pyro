// The receiver slot is shared by same-origin realms. Borrowed methods use
// the receiver's native operations, including its creation URL and partition.
(function (native, slot, window) {
  'use strict';
  const token = Symbol();
  const error = TypeError;
  const toWellFormed = Function.call.bind(String.prototype.toWellFormed);
  const finite = Number.isFinite;
  const trunc = Math.trunc;
  const bigInt = BigInt;
  const asInt64 = BigInt.asIntN;
  const define = Object.defineProperty;
  const descriptor = Object.getOwnPropertyDescriptor;

  function usv(value) {
    return toWellFormed(`${value}`);
  }

  function asDictionary(value) {
    if (value != null && typeof value !== 'object' && typeof value !== 'function')
      throw new error('Expected a dictionary.');
    return value ?? {};
  }

  function state(receiver) {
    const result = slot.get(receiver);
    if (!result) throw new error('Illegal invocation');
    return result;
  }

  function dictionaryOverload(value) {
    return value == null || typeof value === 'object' || typeof value === 'function';
  }

  function date(value) {
    const number = +value;
    if (!finite(number)) throw new error('Invalid cookie expiry timestamp.');
    return number;
  }

  // Signed 64-bit Web IDL integer conversion, passed to native losslessly.
  function maxAge(value) {
    const number = +value;
    return (finite(number) ? asInt64(64, bigInt(trunc(number))) : 0n).toString();
  }

  function sameSite(value) {
    if (value === undefined) return 'strict';
    const mode = `${value}`;
    if (mode !== 'strict' && mode !== 'lax' && mode !== 'none') throw new error('Invalid SameSite.');
    return mode;
  }

  async function read(receiver, value, first) {
    const ops = state(receiver);
    let name = null, url = null;
    if (dictionaryOverload(value)) {
      const options = asDictionary(value);
      const n = options.name;
      if (n !== undefined) name = usv(n);
      const u = options.url;
      if (u !== undefined) url = usv(u);
    } else {
      name = usv(value);
    }
    const pairs = await ops.query(name, url, first);
    if (first) return pairs.length ? {name:pairs[0][0], value:pairs[0][1]} : null;
    const result = [];
    for (let i = 0; i < pairs.length; ++i)
      result[i] = {name:pairs[i][0], value:pairs[i][1]};
    return result;
  }

  class CookieStore extends EventTarget {
    constructor(...args) {
      if (args[0] !== token) throw new error('Illegal constructor');
      super();
      slot.set(this, native);
    }

    get(nameOrOptions = undefined) { return read(this, nameOrOptions, true); }
    getAll(nameOrOptions = undefined) { return read(this, nameOrOptions, false); }

    async set(nameOrOptions, ...rest) {
      const ops = state(this);
      if (rest.length) return ops.write(usv(nameOrOptions), usv(rest[0]), null, null, null, false, '/', 'strict');
      if (!arguments.length) throw new error('A cookie is required.');
      const o = asDictionary(nameOrOptions);
      // Web IDL dictionary getters run once, in lexicographic member order.
      const domainValue = o.domain, domain = domainValue == null ? null : usv(domainValue);
      const expiresValue = o.expires, expires = expiresValue == null ? null : date(expiresValue);
      const ageValue = o.maxAge, age = ageValue == null ? null : maxAge(ageValue);
      const nameValue = o.name;
      if (nameValue === undefined) throw new error('CookieInit requires name.');
      const name = usv(nameValue);
      const partitioned = !!o.partitioned;
      const pathValue = o.path, path = pathValue === undefined ? '/' : usv(pathValue);
      const mode = sameSite(o.sameSite);
      const value = o.value;
      if (value === undefined) throw new error('CookieInit requires value.');
      return ops.write(name, usv(value), domain, expires, age, partitioned, path, mode);
    }

    async delete(nameOrOptions) {
      const ops = state(this);
      if (!arguments.length) throw new error('A cookie name is required.');
      let domain = null, path = '/', partitioned = false, name;
      if (dictionaryOverload(nameOrOptions)) {
        const o = asDictionary(nameOrOptions);
        const d = o.domain;
        if (d != null) domain = usv(d);
        const n = o.name;
        if (n === undefined) throw new error('CookieStoreDeleteOptions requires name.');
        name = usv(n);
        partitioned = !!o.partitioned;
        const p = o.path;
        if (p !== undefined) path = usv(p);
      } else name = usv(nameOrOptions);
      return ops.erase(name, domain, partitioned, path);
    }

    get onchange() { return state(this).handler(this); }
    set onchange(value) { state(this).handler(this, value); }
  }

  if (!window) delete CookieStore.prototype.onchange;
  for (const k of Object.getOwnPropertyNames(CookieStore.prototype)) {
    if (k === 'constructor') continue;
    const d = descriptor(CookieStore.prototype, k);
    d.enumerable = true;
    define(CookieStore.prototype, k, d);
  }
  define(CookieStore.prototype, Symbol.toStringTag, {value:'CookieStore', configurable:true});
  return {CookieStore, cookieStore:new CookieStore(token)};
})
