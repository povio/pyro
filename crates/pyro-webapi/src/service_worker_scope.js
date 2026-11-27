// The ServiceWorkerGlobalScope's script-level part, made once when a service
// worker's realm is created (service workers are rare, so this costs nothing
// for pages). `native` crosses to the browser thread or answers fetches:
//   query(kind, a, b) -> id: an asynchronous browser-thread request
//     ('skipWaiting', 'claim', 'matchAll' (includeUncontrolled, type), 'get'
//     (clientId), 'unregister', 'update'), answered by reply(id, ok, json).
//   postToClient(clientId, message, options) structured-serializes now.
//   respondHead(fetchId, status, statusText, headers, url, type), respondChunk(fetchId, bytes),
//   respondEnd(fetchId): the response to an intercepted request, event by event
//   (a worker that goes away between head and end truncates the body)
//   passthrough(fetchId), failFetch(fetchId, reason)
//   done(kind, ok): the install or activate event's lifetime promises settled.
// `info`: {scope, scriptURL, registrationId, versionId, updateViaCache}.
(function (native, info) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const defineProperty = O.defineProperty;
  const freeze = O.freeze;
  const parse = JSON.parse;
  const PromiseC = Promise;
  const all = PromiseC.all.bind(PromiseC);
  const resolved = (v) => PromiseC.resolve(v);
  const EventC = Event;
  const EventTargetC = EventTarget;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const ResponseC = Response;
  const RequestC = Request;
  const isResponse = (v) => v instanceof ResponseC;
  const arrayBuffer = ResponseC.prototype.arrayBuffer;
  const headersEntries = Headers.prototype.entries;
  const responseHeaders = O.getOwnPropertyDescriptor(ResponseC.prototype, 'headers').get;
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const global = (name, value) =>
    defineProperty(self, name, { value, writable: true, enumerable: false, configurable: true });

  // -- Browser-thread queries -------------------------------------------------
  const pending = new Map();
  const query = (kind, a, b) =>
    new PromiseC((resolve, reject) => {
      const id = native.query(kind, a, b);
      pending.set(id, { resolve, reject });
    });
  function reply(id, ok, json, name, message) {
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (ok) p.resolve(json === undefined || json === '' ? undefined : parse(json));
    else p.reject(name === 'TypeError' ? new TypeErrorC(message) : new DOMExceptionC(message, name));
  }

  // -- Events -------------------------------------------------------------------
  const extendData = new WeakMap();
  class ExtendableEvent extends EventC {
    constructor(type, init) {
      super(type, init);
      extendData.set(this, { promises: [], dispatching: false, pendingCount: 0 });
    }
    waitUntil(promise) {
      const d = extendData.get(this);
      if (!d) throw new TypeErrorC("Illegal invocation");
      // Events this scope dispatches are made here (script-constructed, so
      // `isTrusted` reads false); one a page script made cannot be extended.
      if (!d.internal) throw new DOMExceptionC("Failed to execute 'waitUntil' on 'ExtendableEvent': The event is not trusted.", 'InvalidStateError');
      if (!d.dispatching && d.pendingCount === 0) {
        throw new DOMExceptionC("Failed to execute 'waitUntil' on 'ExtendableEvent': The event handler is already finished and no extend lifetime promises are outstanding.", 'InvalidStateError');
      }
      d.pendingCount++;
      const p = resolved(promise);
      p.then(() => { d.pendingCount--; }, () => { d.pendingCount--; });
      d.promises.push(p);
    }
  }
  tag(ExtendableEvent, 'ExtendableEvent');

  const fetchData = new WeakMap();
  class FetchEvent extends ExtendableEvent {
    constructor(type, init) {
      if (arguments.length < 2) throw new TypeErrorC("Failed to construct 'FetchEvent': 2 arguments required, but only " + arguments.length + ' present.');
      if (init === null || typeof init !== 'object' || !(init.request instanceof RequestC)) {
        throw new TypeErrorC("Failed to construct 'FetchEvent': Failed to read the 'request' property from 'FetchEventInit': Required member is undefined.");
      }
      super(type, init);
      fetchData.set(this, {
        request: init.request,
        clientId: init.clientId === undefined ? '' : String(init.clientId),
        resultingClientId: init.resultingClientId === undefined ? '' : String(init.resultingClientId),
        handled: init.handled instanceof PromiseC ? init.handled : resolved(undefined),
        preloadResponse: resolved(undefined),
        response: null,
        responded: false,
      });
    }
    get request() { return fetchData.get(this).request; }
    get clientId() { return fetchData.get(this).clientId; }
    get resultingClientId() { return fetchData.get(this).resultingClientId; }
    get replacesClientId() { return ''; }
    get handled() { return fetchData.get(this).handled; }
    get preloadResponse() { return fetchData.get(this).preloadResponse; }
    respondWith(r) {
      const d = fetchData.get(this);
      const e = extendData.get(this);
      if (!d) throw new TypeErrorC('Illegal invocation');
      if (!e.dispatching) {
        throw new DOMExceptionC("Failed to execute 'respondWith' on 'FetchEvent': The event handler is already finished.", 'InvalidStateError');
      }
      if (d.responded) {
        throw new DOMExceptionC("Failed to execute 'respondWith' on 'FetchEvent': The event has already been responded to.", 'InvalidStateError');
      }
      d.responded = true;
      this.preventDefault();
      const p = resolved(r);
      d.response = p;
      this.waitUntil(p);
      this.stopImmediatePropagation();
    }
  }
  tag(FetchEvent, 'FetchEvent');

  const messageData = new WeakMap();
  class ExtendableMessageEvent extends ExtendableEvent {
    constructor(type, init) {
      super(type, init);
      const i = init || {};
      messageData.set(this, {
        data: i.data === undefined ? null : i.data,
        origin: i.origin === undefined ? '' : String(i.origin),
        lastEventId: i.lastEventId === undefined ? '' : String(i.lastEventId),
        source: i.source === undefined ? null : i.source,
        ports: freeze(i.ports ? Array.from(i.ports) : []),
      });
    }
    get data() { return messageData.get(this).data; }
    get origin() { return messageData.get(this).origin; }
    get lastEventId() { return messageData.get(this).lastEventId; }
    get source() { return messageData.get(this).source; }
    get ports() { return messageData.get(this).ports; }
  }
  tag(ExtendableMessageEvent, 'ExtendableMessageEvent');

  // -- Clients ------------------------------------------------------------------
  const clientData = new WeakMap();
  class Client {
    constructor(t, c) {
      if (t !== token) throw illegal('Client');
      clientData.set(this, c);
    }
    get id() { return clientData.get(this).id; }
    get url() { return clientData.get(this).url; }
    get type() { return clientData.get(this).type; }
    get frameType() { return clientData.get(this).frameType; }
    get lifecycleState() { return 'active'; }
    postMessage(message, options) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'postMessage' on 'Client': 1 argument required, but only 0 present.");
      }
      native.postToClient(clientData.get(this).id, message, options);
    }
  }
  tag(Client, 'Client');
  class WindowClient extends Client {
    get visibilityState() { return clientData.get(this).visibilityState; }
    get focused() { return clientData.get(this).focused; }
    get ancestorOrigins() { return freeze([]); }
    focus() {
      return PromiseC.reject(new DOMExceptionC("Failed to execute 'focus' on 'WindowClient': Not allowed to focus a window.", 'InvalidAccessError'));
    }
    navigate() {
      return PromiseC.reject(new TypeErrorC("Failed to execute 'navigate' on 'WindowClient': Navigation is not supported."));
    }
  }
  tag(WindowClient, 'WindowClient');
  const client = (c) => (c === null ? undefined : new WindowClient(token, c));

  class Clients {
    constructor(t) {
      if (t !== token) throw illegal('Clients');
    }
    get(id) {
      if (arguments.length === 0) {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'get' on 'Clients': 1 argument required, but only 0 present."));
      }
      return query('get', String(id)).then(client);
    }
    matchAll(options) {
      const o = options === undefined || options === null ? {} : options;
      const type = o.type === undefined ? 'window' : String(o.type);
      if (!['window', 'worker', 'sharedworker', 'all'].includes(type)) {
        return PromiseC.reject(new TypeErrorC(`Failed to execute 'matchAll' on 'Clients': The provided value '${type}' is not a valid enum value of type ClientType.`));
      }
      return query('matchAll', !!o.includeUncontrolled, type).then((list) => list.map(client));
    }
    openWindow() {
      return PromiseC.reject(new TypeErrorC("Failed to execute 'openWindow' on 'Clients': Opening windows is not supported."));
    }
    claim() {
      return query('claim').then(() => undefined);
    }
  }
  tag(Clients, 'Clients');
  const clients = new Clients(token);

  // -- The worker's own objects ---------------------------------------------------
  const workerData = new WeakMap();
  class ServiceWorker extends EventTargetC {
    constructor(t, d) {
      if (t !== token) throw illegal('ServiceWorker');
      super();
      defineProperty(this, Symbol.toStringTag, { value: 'ServiceWorker', configurable: true });
      workerData.set(this, d);
    }
    get scriptURL() { return workerData.get(this).scriptURL; }
    get state() { return workerData.get(this).state; }
    postMessage(message, options) {
      if (!workerData.has(this)) throw new TypeErrorC('Illegal invocation');
      if (arguments.length === 0) throw new TypeErrorC("ServiceWorker.postMessage requires a message.");
      native.postToSelf(message, options);
    }
  }
  tag(ServiceWorker, 'ServiceWorker');
  const me = new ServiceWorker(token, { scriptURL: info.scriptURL, state: 'parsed' });

  const managers = new WeakMap();
  class ServiceWorkerRegistration extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('ServiceWorkerRegistration');
      super();
      defineProperty(this, Symbol.toStringTag, { value: 'ServiceWorkerRegistration', configurable: true });
    }
    get cookies() {
      if (this !== registration) throw illegal('ServiceWorkerRegistration.cookies');
      if (!managers.has(this)) managers.set(this, native.cookieManager());
      return managers.get(this);
    }
    get scope() { return info.scope; }
    get updateViaCache() { return info.updateViaCache; }
    get installing() { return slots.installing; }
    get waiting() { return slots.waiting; }
    get active() { return slots.active; }
    get navigationPreload() { return navigationPreload; }
    update() { return query('update').then(() => registration); }
    unregister() { return query('unregister'); }
  }
  defineProperty(ServiceWorkerRegistration.prototype, 'cookies', { enumerable: true });
  tag(ServiceWorkerRegistration, 'ServiceWorkerRegistration');
  const registration = new ServiceWorkerRegistration(token);
  const slots = { installing: null, waiting: null, active: null };
  const navigationPreload = freeze({
    enable: () => resolved(undefined),
    disable: () => resolved(undefined),
    setHeaderValue: () => resolved(undefined),
    getState: () => resolved({ enabled: false, headerValue: 'true' }),
  });

  // This worker's state, as the browser thread moves it.
  function state(value) {
    workerData.get(me).state = value;
    slots.installing = value === 'installing' ? me : null;
    slots.waiting = value === 'installed' ? me : null;
    slots.active = value === 'activating' || value === 'activated' ? me : slots.active === me ? null : slots.active;
    if (value === 'redundant') slots.active = null;
  }

  // -- Dispatch -------------------------------------------------------------------
  const dispatch = (event, trusted = false, originKey = undefined) => {
    const d = extendData.get(event);
    d.internal = true;
    d.dispatching = true;
    try {
      if (trusted) native.dispatchTrustedEvent(event, originKey);
      else self.dispatchEvent(event);
    } finally {
      d.dispatching = false;
    }
    return d;
  };
  // Lifetime promises, including ones added while they settle.
  const settled = (d) => {
    const count = d.promises.length;
    return all(d.promises).then(() => (d.promises.length > count ? settled(d) : undefined));
  };

  function lifecycle(kind) {
    state(kind === 'install' ? 'installing' : 'activating');
    const d = dispatch(new ExtendableEvent(kind));
    settled(d).then(() => native.done(kind, true), () => native.done(kind, kind === 'activate'));
  }

  // -- The fetch pipeline ----------------------------------------------------------
  // `dispatchFetch(job)` is called by Rust (`service_worker_scope::fetch`) for each
  // request a controlled client (or a navigation) sends through this worker:
  //   job = {id, request, clientId, resultingClientId, isNavigation, preload}
  // `request` is a `Request`; `id` is the key of the native `FetchJob` (answered by
  // `native.respondHead/respondChunk/respondEnd`, `passthrough` or `failFetch`).
  // Three stages, each a hook for the lanes that build on this:
  //   route(job)       -> a source: 'network' | 'cache' | 'fetch-event' | 'race'
  //                       (sw-1: the static router's rules; sw-3 decides here whether the
  //                       navigation preload starts: it needs `job.preload`, `native.fetchRoute(id)`
  //                       is the hook for natives it adds)
  //   fire(job)        -> builds the FetchEvent, dispatches it, returns its fetchData record
  //                       (sw-2: FetchEvent semantics: handled, preloadResponse from job.preload,
  //                       event init, waitUntil lifetime of the event past the response)
  //   settle(job, d)   -> turns d.response into native calls (sw-2: stream the body with
  //                       respondChunk as the Response's reader yields instead of `arrayBuffer`,
  //                       and failFetch after the head means a truncated body: Rust's Drop of the job)
  // The browser-side half is `LoadRoute::dispatch` (service_worker.rs), which builds the
  // native `FetchJob`; sw-3 adds `preload` to `LoadRoute`/`FetchJob` there.
  const respondWhole = (id, response, body) => {
    const headers = [];
    for (const pair of headersEntries.call(responseHeaders.call(response))) headers.push(pair[0], pair[1]);
    native.respondHead(id, response.status, response.statusText, headers, response.url, response.type);
    native.respondChunk(id, body);
    native.respondEnd(id);
  };

  // Which source answers `job`. Behaviour of a worker without routes: its fetch handlers.
  function route(job) {
    return 'fetch-event';
  }

  // The `fetch` event for `job`, dispatched; null when it cannot be made (the request
  // then goes to the network).
  function fire(job) {
    let event;
    try {
      event = new FetchEvent('fetch', { request: job.request, clientId: job.clientId, resultingClientId: job.resultingClientId, cancelable: true });
    } catch (e) {
      return null;
    }
    dispatch(event);
    return fetchData.get(event);
  }

  // The `respondWith` promise's response, handed to the native side.
  function settle(job, d) {
    const id = job.id;
    native.observeFetch(id);
    d.response.then(
      (response) => {
        if (!isResponse(response)) throw new TypeErrorC('respondWith() was given a value that is not a Response');
        if (response.type === 'error') throw new TypeErrorC('respondWith() was given a network error');
        if (response.bodyUsed) throw new TypeErrorC('respondWith() was given a Response whose body was used');
        return arrayBuffer.call(response).then((body) => respondWhole(id, response, body));
      },
    ).then(undefined, (error) => native.failFetch(id, String(error && error.message || error)));
  }

  function dispatchFetch(job) {
    const source = route(job);
    if (source === 'network') {
      native.passthrough(job.id);
      return;
    }
    const d = fire(job);
    if (d === null || !d.responded) {
      native.passthrough(job.id);
      return;
    }
    settle(job, d);
  }

  function dispatchMessage(data, ports, source, origin, originKey) {
    const event = new ExtendableMessageEvent('message', {
      data,
      origin,
      ports: ports || [],
      source: source === null ? me : client(source),
    });
    dispatch(event, true, originKey);
  }

  function dispatchMessageError(source, origin, originKey) {
    const event = new ExtendableMessageEvent('messageerror', { origin, source: source === null ? me : client(source) });
    dispatch(event, true, originKey);
  }

  for (const [name, C] of [
    ['ExtendableEvent', ExtendableEvent], ['FetchEvent', FetchEvent],
    ['ExtendableMessageEvent', ExtendableMessageEvent], ['Client', Client], ['WindowClient', WindowClient],
    ['Clients', Clients], ['ServiceWorker', ServiceWorker], ['ServiceWorkerRegistration', ServiceWorkerRegistration],
  ]) {
    global(name, C);
  }

  return {
    clients,
    ExtendableEvent,
    dispatchCookieChange(event) {
      const d = dispatch(event, true);
      settled(d).catch(error => { self.reportError(error); });
    },
    registration,
    serviceWorker: me,
    skipWaiting: () => query('skipWaiting').then(() => undefined),
    state,
    install: () => lifecycle('install'),
    activate: () => lifecycle('activate'),
    dispatchFetch,
    dispatchMessage,
    dispatchMessageError,
    reply,
  };
})
