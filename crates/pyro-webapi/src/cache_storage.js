// Cache Storage's script-level part (`caches`, `CacheStorage`, `Cache`), made
// on a realm's first use of one of them. The entries live natively
// (cache_storage.rs), shared by the context's documents and workers of one
// origin; `native`:
//   storage(op, name): 'get' -> handle | null, 'open' -> handle, 'has',
//     'delete' -> boolean, 'keys' -> names in creation order.
//   match(handle | null, url | null, method, headers, ignoreSearch,
//     ignoreMethod, ignoreVary, first) -> new Responses (null: every cache).
//   keys(handle, url | null, method, headers, ...options) -> [[url, method, [[n, v]]]].
//   read(response) -> promise of the body for a 'put' operation, or null.
//   batch(handle, operations) -> whether each delete removed something.
// Headers cross as flat [name, value, ...] lists.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const RequestC = Request;
  const ResponseC = Response;
  const URLC = URL;
  const fetchF = fetch;
  const all = PromiseC.all.bind(PromiseC);
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => O.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  // Runs `f` in a promise: a throw rejects it.
  const run = (f) => new PromiseC((resolve) => resolve(f()));
  const need = (count, wanted, iface, method) => {
    if (count < wanted) {
      throw new TypeErrorC(`Failed to execute '${method}' on '${iface}': ${wanted} argument${wanted > 1 ? 's' : ''} required, but only ${count} present.`);
    }
  };
  const flat = (headers) => {
    const out = [];
    for (const pair of headers) out.push(pair[0], pair[1]);
    return out;
  };
  const options = (o) => {
    if (o === undefined || o === null) return { ignoreSearch: false, ignoreMethod: false, ignoreVary: false, cacheName: undefined };
    if (typeof o !== 'object' && typeof o !== 'function') {
      throw new TypeErrorC("The provided value is not of type 'CacheQueryOptions'.");
    }
    return {
      ignoreSearch: !!o.ignoreSearch,
      ignoreMethod: !!o.ignoreMethod,
      ignoreVary: !!o.ignoreVary,
      cacheName: o.cacheName === undefined ? undefined : String(o.cacheName),
    };
  };
  // RequestInfo: a Request as it is, anything else through `new Request`.
  const toRequest = (r) => (r instanceof RequestC ? r : new RequestC(r));
  const httpScheme = (url) => {
    const scheme = new URLC(url).protocol;
    return scheme === 'http:' || scheme === 'https:';
  };
  const varyAll = (response) => {
    const vary = response.headers.get('vary');
    return vary !== null && vary.split(',').some((v) => v.trim() === '*');
  };

  // The matching responses (`first`: at most one) of cache `h` (null: all).
  const lookup = (h, request, o, first, backend = native) => {
    if (request === undefined) return backend.match(h, null, 'GET', [], false, false, false, first);
    const r = toRequest(request);
    if (r.method !== 'GET' && !o.ignoreMethod) return [];
    return backend.match(h, r.url, r.method, flat(r.headers), o.ignoreSearch, o.ignoreMethod, o.ignoreVary, first);
  };

  const putOp = (r, response, body) => [
    'put', r.url, r.method, flat(r.headers), response.type, response.url, response.status, response.statusText,
    flat(response.headers), response.redirected, body,
  ];
  // The body as the cache keeps it: the bytes the network delivered, shared.
  const readBody = (response) => PromiseC.resolve(native.read(response));

  const handles = new WeakMap();
  const handle = (cache) => {
    const h = handles.get(cache);
    if (h === undefined) throw new TypeErrorC('Illegal invocation');
    return h;
  };

  class Cache {
    constructor(t, h) {
      if (t !== token) throw illegal('Cache');
      handles.set(this, h);
    }
    match(request, opts) {
      return run(() => {
        const h = handle(this);
        need(arguments.length, 1, 'Cache', 'match');
        return lookup(h, request, options(opts), true)[0];
      });
    }
    matchAll(request, opts) {
      return run(() => lookup(handle(this), request, options(opts), false));
    }
    add(request) {
      return run(() => {
        handle(this);
        need(arguments.length, 1, 'Cache', 'add');
        return this.addAll([request]).then(() => undefined);
      });
    }
    addAll(requests) {
      return run(() => {
        const h = handle(this);
        need(arguments.length, 1, 'Cache', 'addAll');
        if (requests === null || typeof requests !== 'object' || typeof requests[Symbol.iterator] !== 'function') {
          throw new TypeErrorC("Failed to execute 'addAll' on 'Cache': The provided value cannot be converted to a sequence.");
        }
        const list = Array.from(requests, (input) => {
          const r = toRequest(input);
          if (!httpScheme(r.url)) throw new TypeErrorC(`Failed to execute 'addAll' on 'Cache': Request scheme '${new URLC(r.url).protocol.slice(0, -1)}' is unsupported`);
          if (r.method !== 'GET') throw new TypeErrorC(`Failed to execute 'addAll' on 'Cache': Request method '${r.method}' is unsupported`);
          return r;
        });
        return all(list.map((r) => fetchF(r).then((response) => {
          if (response.type === 'error' || !response.ok || response.status === 206) {
            throw new TypeErrorC(`Failed to execute 'addAll' on 'Cache': Request failed`);
          }
          if (varyAll(response)) throw new TypeErrorC("Failed to execute 'addAll' on 'Cache': Vary header contains *");
          return readBody(response).then((body) => putOp(r, response, body));
        }))).then((ops) => {
          native.batch(h, ops);
        });
      });
    }
    put(request, response) {
      return run(() => {
        const h = handle(this);
        need(arguments.length, 2, 'Cache', 'put');
        if (!(response instanceof ResponseC)) {
          throw new TypeErrorC("Failed to execute 'put' on 'Cache': parameter 2 is not of type 'Response'.");
        }
        if (request instanceof RequestC && request.method !== 'GET') {
          throw new TypeErrorC(`Failed to execute 'put' on 'Cache': Request method '${request.method}' is unsupported`);
        }
        const r = toRequest(request);
        if (!httpScheme(r.url)) {
          throw new TypeErrorC(`Failed to execute 'put' on 'Cache': Request scheme '${new URLC(r.url).protocol.slice(0, -1)}' is unsupported`);
        }
        if (response.status === 206) {
          throw new TypeErrorC("Failed to execute 'put' on 'Cache': Partial response (status code 206) is unsupported");
        }
        if (varyAll(response)) throw new TypeErrorC("Failed to execute 'put' on 'Cache': Vary header contains *");
        if (response.bodyUsed) throw new TypeErrorC("Failed to execute 'put' on 'Cache': Response body is already used");
        return readBody(response).then((body) => {
          native.batch(h, [putOp(r, response, body)]);
        });
      });
    }
    delete(request, opts) {
      return run(() => {
        const h = handle(this);
        need(arguments.length, 1, 'Cache', 'delete');
        const o = options(opts);
        const r = toRequest(request);
        if (r.method !== 'GET' && !o.ignoreMethod) return false;
        return native.batch(h, [['delete', r.url, r.method, flat(r.headers), o.ignoreSearch, o.ignoreMethod, o.ignoreVary]])[0];
      });
    }
    keys(request, opts) {
      return run(() => {
        const h = handle(this);
        const o = options(opts);
        let found;
        if (request === undefined) {
          found = native.keys(h, null, 'GET', [], false, false, false);
        } else {
          const r = toRequest(request);
          if (r.method !== 'GET' && !o.ignoreMethod) return [];
          found = native.keys(h, r.url, r.method, flat(r.headers), o.ignoreSearch, o.ignoreMethod, o.ignoreVary);
        }
        return found.map((k) => new RequestC(k[0], { method: k[1], headers: k[2] }));
      });
    }
  }
  tag(Cache, 'Cache');

  const stores = new WeakMap();
  const storeOf = (storage) => {
    const backend = stores.get(storage);
    if (!backend) throw new TypeErrorC('Illegal invocation');
    return backend;
  };
  class CacheStorage {
    constructor(t, ...backend) {
      if (t !== token) throw illegal('CacheStorage');
      stores.set(this, backend[0]);
    }
    match(request, opts) {
      return run(() => {
        const backend = storeOf(this);
        need(arguments.length, 1, 'CacheStorage', 'match');
        const o = options(opts);
        if (o.cacheName !== undefined) {
          const h = backend.storage('get', o.cacheName);
          return h === null ? undefined : lookup(h, request, o, true, backend)[0];
        }
        return lookup(null, request, o, true, backend)[0];
      });
    }
    has(name) {
      return run(() => {
        need(arguments.length, 1, 'CacheStorage', 'has');
        return storeOf(this).storage('has', String(name));
      });
    }
    open(name) {
      return run(() => {
        need(arguments.length, 1, 'CacheStorage', 'open');
        return new Cache(token, storeOf(this).storage('open', String(name)));
      });
    }
    delete(name) {
      return run(() => {
        need(arguments.length, 1, 'CacheStorage', 'delete');
        return storeOf(this).storage('delete', String(name));
      });
    }
    keys() {
      return run(() => storeOf(this).storage('keys', ''));
    }
  }
  tag(CacheStorage, 'CacheStorage');

  return { Cache, CacheStorage, caches: new CacheStorage(token, native), bucketCaches: backend => new CacheStorage(token, backend) };
})
