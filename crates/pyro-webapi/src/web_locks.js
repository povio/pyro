// Web Locks API for one realm: LockManager (navigator.locks) and Lock. The lock manager's queues are
// native (web_locks.rs), shared by every same-origin document of the browser context:
//   state() -> 'ok' | 'inactive' | 'opaque'
//   request(name, exclusive, ifAvailable, steal, grant, stolen) -> id, 0 when ifAvailable and busy
//   release(id), abort(id) -> whether it was still waiting, watchAbort(signal, id) (on abort: abortRequest(id)), query() -> [held, pending] of [name, mode, clientId]
// `grant` and `stolen` are called with no arguments in a task when the lock is granted or stolen.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const resolve = (v) => PromiseC.resolve(v);
  const reject = (e) => PromiseC.reject(e);
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const AbortSignalC = AbortSignal;
  const StringC = String;
  const call = Function.prototype.call;
  const signalAborted = O.getOwnPropertyDescriptor(AbortSignalC.prototype, 'aborted').get;
  const signalReason = O.getOwnPropertyDescriptor(AbortSignalC.prototype, 'reason').get;
  // A request's abort steps, by request id: run by the signal's abort algorithm (native.watchAbort,
  // DOM "add an abort algorithm"), not an `abort` listener a page could stop or fake.
  const abortSteps = new Map();
  // Names cross to the native queues as their UTF-16 code units in hex, so that names that are not
  // valid UTF-16 (lone surrogates) stay distinct and come back unchanged.
  const encode = (name) => { let out = ''; for (let i = 0; i < name.length; i++) out += name.charCodeAt(i).toString(16).padStart(4, '0'); return out; };
  const decode = (hex) => { let out = ''; for (let i = 0; i < hex.length; i += 4) out += StringC.fromCharCode(parseInt(hex.slice(i, i + 4), 16)); return out; };
  const isSignal = (v) => { try { call.call(signalAborted, v); return true; } catch { return false; } };
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const fail = (what, message) => new TypeErrorC(`Failed to execute '${what}' on 'LockManager': ${message}`);
  const dom = (message, name) => new DOMExceptionC(message, name);
  const tag = (C, name) => O.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });

  const locks = new WeakMap();
  class Lock {
    constructor(t, name, mode) {
      if (t !== token) throw illegal('Lock');
      locks.set(this, { name, mode });
    }
    get name() { return lockOf(this, 'name').name; }
    get mode() { return lockOf(this, 'mode').mode; }
  }
  const lockOf = (self, member) => {
    const l = locks.get(self);
    if (!l) throw new TypeErrorC(`Failed to read the '${member}' property from 'Lock': Illegal invocation`);
    return l;
  };
  tag(Lock, 'Lock');

  const managers = new WeakSet();
  class LockManager {
    constructor(t) {
      if (t !== token) throw illegal('LockManager');
      managers.add(this);
    }
    request(name, options, callback) {
      try {
        if (!managers.has(this)) throw fail('request', 'Illegal invocation');
        if (arguments.length < 2) throw fail('request', `2 arguments required, but only ${arguments.length} present.`);
        // request(name, callback) or request(name, options, callback)
        if (arguments.length === 2 || (typeof options === 'function' && callback === undefined)) {
          callback = options;
          options = undefined;
        }
        name = StringC(name);
        if (options !== undefined && options !== null && typeof options !== 'object' && typeof options !== 'function') {
          throw fail('request', "The provided value is not of type 'LockOptions'.");
        }
        const o = options ?? {};
        let mode = o.mode;
        mode = mode === undefined ? 'exclusive' : StringC(mode);
        if (mode !== 'exclusive' && mode !== 'shared') {
          throw fail('request', `Failed to read the 'mode' property from 'LockOptions': The provided value '${mode}' is not a valid enum value of type LockMode.`);
        }
        const ifAvailable = !!o.ifAvailable;
        const steal = !!o.steal;
        const signal = o.signal;
        if (signal !== undefined && !isSignal(signal)) {
          throw fail('request', "Failed to read the 'signal' property from 'LockOptions': Failed to convert value to 'AbortSignal'.");
        }
        if (typeof callback !== 'function') {
          throw fail('request', "The callback provided as parameter 3 is not a function.");
        }
        const state = native.state();
        if (state === 'inactive') throw dom('The document is not fully active.', 'InvalidStateError');
        if (state === 'opaque') throw dom('Access to the Locks API is denied in this context.', 'SecurityError');
        if (name.startsWith('-')) throw dom("Names cannot start with '-'.", 'NotSupportedError');
        if (steal && ifAvailable) throw dom("The 'steal' and 'ifAvailable' options cannot be used together.", 'NotSupportedError');
        if (steal && mode !== 'exclusive') throw dom("The 'steal' option may only be used with 'exclusive' locks.", 'NotSupportedError');
        if (signal !== undefined && (steal || ifAvailable)) {
          throw dom(`The 'signal' and '${steal ? 'steal' : 'ifAvailable'}' options cannot be used together.`, 'NotSupportedError');
        }
        if (signal !== undefined && call.call(signalAborted, signal)) return reject(call.call(signalReason, signal));
        let settle, fail2;
        const promise = new PromiseC((res, rej) => { settle = res; fail2 = rej; });
        let id = 0;
        // The callback runs with the lock (or null); the lock is held until its promise settles,
        // which settles the request's promise.
        const run = (lock) => {
          let r;
          try { r = callback(lock); } catch (e) { r = reject(e); }
          resolve(r).then(
            (v) => { if (lock) native.release(id); settle(v); },
            (e) => { if (lock) native.release(id); fail2(e); },
          );
        };
        let granted = false;
        const grant = (grantedId) => {
          id = grantedId;
          granted = true;
          abortSteps.delete(id);
          run(new Lock(token, name, mode));
        };
        const stolen = () => fail2(dom("Lock broken by another request with the 'steal' option.", 'AbortError'));
        id = native.request(encode(name), mode === 'exclusive', ifAvailable, steal, grant, stolen);
        if (id === 0) {
          // ifAvailable and not grantable now: the callback runs with null, nothing is queued.
          resolve().then(() => run(null));
          return promise;
        }
        if (signal !== undefined && !granted) {
          abortSteps.set(id, () => {
            abortSteps.delete(id);
            if (native.abort(id)) fail2(call.call(signalReason, signal));
          });
          native.watchAbort(signal, id);
        }
        return promise;
      } catch (e) {
        return reject(e);
      }
    }
    query() {
      try {
        if (!managers.has(this)) throw new TypeErrorC("Failed to execute 'query' on 'LockManager': Illegal invocation");
        const state = native.state();
        if (state === 'inactive') throw dom('The document is not fully active.', 'InvalidStateError');
        if (state === 'opaque') throw dom('Access to the Locks API is denied in this context.', 'SecurityError');
        const [held, pending] = native.query();
        const info = ([name, mode, clientId]) => ({ name: decode(name), mode, clientId });
        return resolve({ held: held.map(info), pending: pending.map(info) });
      } catch (e) {
        return reject(e);
      }
    }
  }
  O.defineProperty(LockManager.prototype.request, 'length', { value: 2 });
  tag(LockManager, 'LockManager');
  for (const C of [Lock, LockManager]) {
    // Neither interface has an IDL constructor.
    O.defineProperty(C, 'length', { value: 0 });
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(C.prototype, k);
      d.enumerable = true;
      O.defineProperty(C.prototype, k, d);
    }
  }
  // Called by the signal's abort algorithm (web_locks.rs) with the request it guards.
  const abortRequest = (id) => { const steps = abortSteps.get(id); if (steps) steps(); };
  return { LockManager, Lock, locks: new LockManager(token), abortRequest };
});
