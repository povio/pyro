// The ServiceWorkerGlobalScope's script-level part, made once when a service
// worker's realm is created (service workers are rare, so this costs nothing
// for pages). `native` crosses to the browser thread or answers fetches:
//   query(kind, a, b) -> id: an asynchronous browser-thread request
//     ('skipWaiting', 'claim', 'matchAll' (includeUncontrolled, type), 'get'
//     (clientId), 'unregister', 'update', 'preload' (op, value: the registration's
//     NavigationPreloadManager)), answered by reply(id, ok, json).
//   postToClient(clientId, message, options) structured-serializes now.
//   respondHead(fetchId, status, statusText, headers, url, type), respondChunk(fetchId, bytes),
//   respondEnd(fetchId): the response to an intercepted request, event by event
//   (a worker that goes away between head and end truncates the body)
//   passthrough(fetchId), failFetch(fetchId, reason), startRace(fetchId) (the static router's
//   race source: the network request starts now, the first good response wins)
//   startPreload(fetchId): the navigation preload request of a job with `preload` (the header
//   value) starts; preloadDone(id, response) / preloadFailed(id, message) are called back
//   hasFetchHandler(): whether the global has a `fetch` listener (addRoutes needs one for some sources)
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
  const queueMicrotaskC = typeof queueMicrotask === 'function' ? queueMicrotask : (f) => { PromiseC.resolve().then(f); };
  const EventC = Event;
  const EventTargetC = EventTarget;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const ResponseC = Response;
  const RequestC = Request;
  const URLPatternC = typeof URLPattern === 'function' ? URLPattern : undefined;
  const isResponse = (v) => v instanceof ResponseC;
  const responseBody = O.getOwnPropertyDescriptor(ResponseC.prototype, 'body').get;
  const getReader = ReadableStream.prototype.getReader;
  const readerRead = ReadableStreamDefaultReader.prototype.read;
  const readerCancel = ReadableStreamDefaultReader.prototype.cancel;
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
      // "Upon fulfillment or rejection of promise, queue a microtask to decrement the pending
      // promises count": a waitUntil() made by a handler of the same settlement still sees it.
      const done = () => { queueMicrotaskC(() => { d.pendingCount--; }); };
      p.then(done, done);
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
      // `handled` is the page's promise when the init gives one; this scope's events own theirs
      // and settle it when the response is known (see `settleFetch`).
      let handled = init.handled;
      let handledSettlers = null;
      if (!(handled instanceof PromiseC)) {
        handled = new PromiseC((resolve, reject) => { handledSettlers = { resolve, reject }; });
        // Rejected for a network error, never an unhandled rejection of the worker.
        handled.then(undefined, () => {});
      }
      fetchData.set(this, {
        request: init.request,
        clientId: init.clientId === undefined ? '' : String(init.clientId),
        resultingClientId: init.resultingClientId === undefined ? '' : String(init.resultingClientId),
        handled,
        handledSettlers,
        preloadResponse: init.preloadResponse === undefined ? resolved(undefined) : resolved(init.preloadResponse),
        response: null,
        responded: false,
        event: this,
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

  // -- Static routing API (InstallEvent.addRoutes) ---------------------------------
  // https://w3c.github.io/ServiceWorker/#dom-installevent-addroutes . The rules live in this
  // worker's realm, so they belong to this version (a new version registers its own).
  const ROUTER_MAX_DEPTH = 10;
  const ROUTER_MAX_COUNT = 1024;
  const ROUTER_MODES = ['navigate', 'same-origin', 'no-cors', 'cors'];
  const ROUTER_DESTINATIONS = ['', 'audio', 'audioworklet', 'document', 'embed', 'font', 'frame', 'iframe', 'image', 'json', 'manifest', 'object', 'paintworklet', 'report', 'script', 'sharedworker', 'style', 'track', 'video', 'worker', 'xslt'];
  const ROUTER_STATUSES = ['running', 'not-running'];
  const ROUTER_METHODS = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'];
  const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const routerRules = [];
  let routerCount = 0;

  const routerEnum = (value, allowed, what) => {
    const v = String(value);
    if (!allowed.includes(v)) throw new TypeErrorC(`Failed to execute 'addRoutes' on 'InstallEvent': The provided value '${v}' is not a valid enum value of type ${what}.`);
    return v;
  };
  const routerMethod = (value) => {
    const v = String(value);
    for (let i = 0; i < v.length; i++) {
      if (v.charCodeAt(i) > 0xff) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': The provided requestMethod is not a ByteString.");
    }
    if (!TOKEN.test(v)) throw new TypeErrorC('Failed to execute \'addRoutes\' on \'InstallEvent\': requestMethod is not a valid HTTP method token.');
    const upper = v.toUpperCase();
    if (upper === 'CONNECT' || upper === 'TRACE' || upper === 'TRACK') throw new TypeErrorC(`Failed to execute 'addRoutes' on 'InstallEvent': '${v}' is a forbidden method.`);
    return ROUTER_METHODS.includes(upper) ? upper : v;
  };
  // A URLPattern a string or dictionary denotes is relative to the worker's script URL; one
  // that is constructed already is used as it is.
  const routerPattern = (value) => {
    if (URLPatternC === undefined) throw new TypeErrorC('URLPattern is not supported');
    if (value instanceof URLPatternC) return value;
    if (value !== null && typeof value === 'object') {
      const init = {};
      for (const key of ['baseURL', 'hash', 'hostname', 'password', 'pathname', 'port', 'protocol', 'search', 'username']) {
        if (value[key] !== undefined) init[key] = value[key];
      }
      if (init.baseURL === undefined) init.baseURL = info.scriptURL;
      return new URLPatternC(init);
    }
    return new URLPatternC(String(value), info.scriptURL);
  };

  // A condition as a function of (request, runningStatus). `budget.left` counts the
  // conditions the registry may still take.
  function routerCondition(raw, depth, budget) {
    if (raw === null || typeof raw !== 'object') throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': a condition must be a dictionary.");
    if (depth > ROUTER_MAX_DEPTH) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': the condition nests deeper than the limit.");
    if (--budget.left < 0) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': too many router conditions.");
    const has = (key) => raw[key] !== undefined;
    const tests = [];
    const request = ['urlPattern', 'requestMethod', 'requestMode', 'requestDestination', 'runningStatus'].filter(has);
    if (has('or') || has('not')) {
      if (request.length > 0 || (has('or') && has('not'))) {
        throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': 'or' and 'not' cannot be combined with other conditions.");
      }
      if (has('or')) {
        if (raw.or === null || typeof raw.or[Symbol.iterator] !== 'function') throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': 'or' must be a sequence.");
        const subs = Array.from(raw.or, (sub) => routerCondition(sub, depth + 1, budget));
        return (r, status) => subs.some((sub) => sub(r, status));
      }
      const sub = routerCondition(raw.not, depth + 1, budget);
      return (r, status) => !sub(r, status);
    }
    if (request.length === 0) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': a condition must have at least one member.");
    if (has('urlPattern')) {
      const pattern = routerPattern(raw.urlPattern);
      tests.push((r) => pattern.test(r.url));
    }
    if (has('requestMethod')) {
      const method = routerMethod(raw.requestMethod);
      tests.push((r) => r.method === method);
    }
    if (has('requestMode')) {
      const mode = routerEnum(raw.requestMode, ROUTER_MODES, 'RequestMode');
      tests.push((r) => r.mode === mode);
    }
    if (has('requestDestination')) {
      const destination = routerEnum(raw.requestDestination, ROUTER_DESTINATIONS, 'RequestDestination');
      tests.push((r) => r.destination === destination);
    }
    if (has('runningStatus')) {
      const wanted = routerEnum(raw.runningStatus, ROUTER_STATUSES, 'RunningStatus');
      tests.push((r, status) => status === wanted);
    }
    return (r, status) => tests.every((test) => test(r, status));
  }

  function routerSource(raw) {
    if (raw === undefined) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': a rule needs a source.");
    if (raw !== null && typeof raw === 'object') {
      return { type: 'cache', cacheName: raw.cacheName === undefined ? undefined : String(raw.cacheName) };
    }
    const v = String(raw);
    switch (v) {
      case 'network': return { type: 'network' };
      case 'cache': return { type: 'cache', cacheName: undefined };
      case 'fetch-event':
      case 'race-network-and-fetch-handler':
        // These need the worker's fetch handler.
        if (!native.hasFetchHandler()) throw new TypeErrorC(`Failed to execute 'addRoutes' on 'InstallEvent': the '${v}' source needs a fetch event handler.`);
        return { type: v === 'fetch-event' ? 'fetch-event' : 'race' };
      default:
        throw new TypeErrorC(`Failed to execute 'addRoutes' on 'InstallEvent': The provided value '${v}' is not a valid router source.`);
    }
  }

  class InstallEvent extends ExtendableEvent {
    addRoutes(rules) {
      try {
        if (!extendData.has(this) || !(this instanceof InstallEvent)) throw new TypeErrorC('Illegal invocation');
        if (arguments.length === 0) throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': 1 argument required, but only 0 present.");
        if (!extendData.get(this).internal || workerData.get(me).state !== 'installing') {
          throw new DOMExceptionC("Failed to execute 'addRoutes' on 'InstallEvent': Router rules can only be added while the service worker is installing.", 'InvalidStateError');
        }
        let list;
        if (rules !== null && typeof rules === 'object' && typeof rules[Symbol.iterator] === 'function') list = Array.from(rules);
        else if (rules === undefined || rules === null || typeof rules === 'object') list = [rules === null ? undefined : rules];
        else throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': The provided value is not a RouterRule or a sequence of them.");
        const budget = { left: ROUTER_MAX_COUNT - routerCount };
        const parsed = list.map((rule) => {
          if (rule === undefined || rule === null || typeof rule !== 'object' || rule.condition === undefined) {
            throw new TypeErrorC("Failed to execute 'addRoutes' on 'InstallEvent': a rule needs a condition and a source.");
          }
          const condition = routerCondition(rule.condition, 0, budget);
          return { matches: condition, source: routerSource(rule.source) };
        });
        // All or none of the rules register.
        for (const rule of parsed) routerRules.push(rule);
        routerCount = ROUTER_MAX_COUNT - budget.left;
        return resolved(undefined);
      } catch (error) {
        return PromiseC.reject(error);
      }
    }
  }
  tag(InstallEvent, 'InstallEvent');

  const messageData = new WeakMap();
  class ExtendableMessageEvent extends ExtendableEvent {
    constructor(type, init) {
      super(type, init);
      const i = init || {};
      // WebIDL: `(Client or ServiceWorker or MessagePort)? source = null`, `sequence<MessagePort> ports = []`.
      const source = i.source === undefined ? null : i.source;
      if (source !== null && !(clientData.has(source) || workerData.has(source) || source instanceof MessagePort)) {
        throw new TypeErrorC("Failed to construct 'ExtendableMessageEvent': Failed to read the 'source' property from 'ExtendableMessageEventInit': The provided value is neither a Client, a ServiceWorker nor a MessagePort.");
      }
      let ports = [];
      if (i.ports !== undefined) {
        if (i.ports === null || typeof i.ports !== 'object' || typeof i.ports[Symbol.iterator] !== 'function') {
          throw new TypeErrorC("Failed to construct 'ExtendableMessageEvent': Failed to read the 'ports' property from 'ExtendableMessageEventInit': The provided value cannot be converted to a sequence.");
        }
        ports = Array.from(i.ports);
        for (const port of ports) {
          if (!(port instanceof MessagePort)) {
            throw new TypeErrorC("Failed to construct 'ExtendableMessageEvent': Failed to read the 'ports' property from 'ExtendableMessageEventInit': Failed to convert a value to 'MessagePort'.");
          }
        }
      }
      messageData.set(this, {
        data: i.data === undefined ? null : i.data,
        origin: i.origin === undefined ? '' : String(i.origin),
        lastEventId: i.lastEventId === undefined ? '' : String(i.lastEventId),
        source,
        ports: freeze(ports),
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
  const client = (c) => (c === null ? undefined : c.type === 'window' ? new WindowClient(token, c) : new Client(token, c));

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
  // `registration.navigationPreload`: the registration's state lives on the browser thread
  // (queries), as the page's manager reaches it.
  const preloadOp = (manager, op, value) => {
    if (manager !== navigationPreload) return PromiseC.reject(new TypeErrorC('Illegal invocation'));
    return query('preload', op, value).then((state) => state);
  };
  class NavigationPreloadManager {
    constructor(t) {
      if (t !== token) throw illegal('NavigationPreloadManager');
    }
    enable() { return preloadOp(this, 'enable'); }
    disable() { return preloadOp(this, 'disable'); }
    setHeaderValue(value) {
      if (arguments.length === 0) {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'setHeaderValue' on 'NavigationPreloadManager': 1 argument required, but only 0 present."));
      }
      // ByteString, and a header value: no NUL, CR or LF.
      const v = String(value);
      for (let i = 0; i < v.length; i++) {
        const c = v.charCodeAt(i);
        if (c > 0xff || c === 0 || c === 10 || c === 13) {
          return PromiseC.reject(new TypeErrorC("Failed to execute 'setHeaderValue' on 'NavigationPreloadManager': The provided value is not a valid header value."));
        }
      }
      return preloadOp(this, 'setHeaderValue', v);
    }
    getState() { return preloadOp(this, 'getState'); }
  }
  tag(NavigationPreloadManager, 'NavigationPreloadManager');
  const navigationPreload = new NavigationPreloadManager(token);

  // This worker's state, as the browser thread moves it.
  function state(value) {
    workerData.get(me).state = value;
    slots.installing = value === 'installing' ? me : null;
    slots.waiting = value === 'installed' ? me : null;
    slots.active = value === 'activating' || value === 'activated' ? me : slots.active === me ? null : slots.active;
    if (value === 'redundant') slots.active = null;
  }

  // -- Dispatch -------------------------------------------------------------------
  // An event's dispatch flag stays set until the microtask checkpoint after its listeners ran
  // (respondWith() and waitUntil() are allowed from a microtask of the handler); Rust calls
  // `closeDispatch` once the call that dispatched returned, which drained the microtasks.
  let open = [];
  const dispatch = (event, trusted = false, originKey = undefined) => {
    const d = extendData.get(event);
    d.internal = true;
    d.dispatching = true;
    open.push(d);
    if (trusted) native.dispatchTrustedEvent(event, originKey);
    else self.dispatchEvent(event);
    return d;
  };
  function closeDispatch() {
    const list = open;
    open = [];
    for (const d of list) {
      d.dispatching = false;
      if (d.after !== undefined) {
        const after = d.after;
        d.after = undefined;
        after();
      }
    }
  }
  // Lifetime promises, including ones added while they settle.
  const settled = (d) => {
    const count = d.promises.length;
    return all(d.promises).then(() => (d.promises.length > count ? settled(d) : undefined));
  };

  function lifecycle(kind) {
    state(kind === 'install' ? 'installing' : 'activating');
    const d = dispatch(kind === 'install' ? new InstallEvent(kind) : new ExtendableEvent(kind));
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
  const sendHead = (id, response) => {
    const headers = [];
    let status = response.status;
    const internal = response.type === 'opaqueredirect' ? native.internalHead(response) : undefined;
    if (typeof internal === 'string') {
      // An opaque-redirect response hides its redirect from script, not from the navigation it
      // answers: that follows the `Location` it names.
      const lines = internal.split('\n');
      status = Number(lines[0]);
      for (let i = 1; i < lines.length; i++) {
        const colon = lines[i].indexOf(':');
        if (colon > 0) headers.push(lines[i].slice(0, colon), lines[i].slice(colon + 1));
      }
    } else {
      for (const pair of headersEntries.call(responseHeaders.call(response))) headers.push(pair[0], pair[1]);
    }
    native.respondHead(id, status, response.statusText, headers, response.url, response.type, native.responseCorp(response));
  };

  // Response bodies being streamed to the page, by fetch id: the page cancelling its side
  // (`cancelFetch`, from the browser thread) cancels the body's reader here.
  const streaming = new Map();
  function cancelFetch(id) {
    const entry = streaming.get(id);
    if (entry === undefined) return;
    streaming.delete(id);
    entry.cancelled = true;
    if (entry.reader === null) return;
    try { readerCancel.call(entry.reader).then(undefined, () => {}); } catch (e) { /* already released */ }
  }

  // The body, chunk by chunk as the Response's reader yields: head first, then each Uint8Array,
  // then the end. A chunk of another type errors the page's body (`failFetch` after the head is
  // a truncated body, not a network error before it). The reader is taken before the head goes,
  // so a locked body is the network error `respondWith` wants.
  function stream(id, response) {
    if (response.type === 'opaque') {
      // An opaque response has no `body` for script, but its bytes are the page's: head, the
      // bytes once they have arrived, end.
      const bytes = native.internalBody(response);
      if (bytes !== undefined) {
        const entry = { reader: null, cancelled: false };
        streaming.set(id, entry);
        bytes.then((buffer) => {
          if (entry.cancelled) return;
          streaming.delete(id);
          sendHead(id, response);
          if (buffer.byteLength > 0) native.respondChunk(id, new Uint8Array(buffer));
          native.respondEnd(id);
        }, (error) => {
          if (entry.cancelled) return;
          streaming.delete(id);
          native.failFetch(id, String(error && error.message || error));
        });
        return;
      }
    }
    const body = responseBody.call(response);
    const reader = body === null ? null : getReader.call(body);
    sendHead(id, response);
    if (reader === null) {
      native.respondEnd(id);
      return;
    }
    const entry = { reader, cancelled: false };
    streaming.set(id, entry);
    (async () => {
      for (;;) {
        const result = await readerRead.call(reader);
        if (entry.cancelled) return;
        if (result.done) break;
        const chunk = result.value;
        if (!(chunk instanceof Uint8Array)) throw new TypeErrorC('respondWith() was given a Response whose body has a chunk that is not a Uint8Array');
        if (chunk.byteLength > 0) native.respondChunk(id, chunk);
      }
      streaming.delete(id);
      native.respondEnd(id);
    })().then(undefined, (error) => {
      if (entry.cancelled) return;
      streaming.delete(id);
      try { readerCancel.call(reader, error).then(undefined, () => {}); } catch (e) { /* released */ }
      native.failFetch(id, String(error && error.message || error));
    });
  }

  // Which source answers `job`: the first static router rule (addRoutes) its request matches, in
  // registration order; without one, the fetch handlers. Always evaluated here, with or without a
  // fetch handler. This worker is running, since it is evaluating.
  function route(job) {
    for (const rule of routerRules) {
      if (rule.matches(job.request, 'running')) return rule.source;
    }
    return { type: 'fetch-event' };
  }

  // The 'cache' source: a match in the (named) cache answers; no entry or a failing lookup is the
  // network's, not the fetch handler's.
  function fromCache(job, source) {
    const options = source.cacheName === undefined ? {} : { cacheName: source.cacheName };
    self.caches.match(job.request, options).then((response) => {
      if (response === undefined) native.passthrough(job.id);
      else settle(job, { response: resolved(response) });
    }, () => native.passthrough(job.id));
  }

  // The `fetch` event for `job`, dispatched; null when it cannot be made (the request
  // then goes to the network). The record returned is the event's `fetchData`.
  function fire(job, preloadResponse) {
    let event;
    try {
      event = new FetchEvent('fetch', { request: job.request, clientId: job.clientId, resultingClientId: job.resultingClientId, preloadResponse, cancelable: true });
    } catch (e) {
      return null;
    }
    dispatch(event);
    return fetchData.get(event);
  }

  // The navigation preload (https://w3c.github.io/ServiceWorker/#navigation-preload): `job.preload`
  // is the header value when the registration has it enabled for this GET navigation, and then the
  // network request starts now, beside the handler (`native.startPreload`); `preloadResponse` is the
  // promise of its Response (a network error rejects it), undefined when there is no preload.
  const preloads = new Map();
  function startPreload(job) {
    if (typeof job.preload !== 'string' || !native.hasFetchHandler()) return resolved(undefined);
    const promise = new PromiseC((resolve, reject) => { preloads.set(job.id, { resolve, reject }); });
    if (!native.startPreload(job.id)) {
      preloads.delete(job.id);
      return resolved(undefined);
    }
    // A preload nobody asked for that failed is not the worker's unhandled rejection.
    promise.then(undefined, () => {});
    return promise;
  }
  function preloadDone(id, response) {
    const entry = preloads.get(id);
    if (entry === undefined) return;
    preloads.delete(id);
    entry.resolve(response);
  }
  function preloadFailed(id, message) {
    const entry = preloads.get(id);
    if (entry === undefined) return;
    preloads.delete(id);
    entry.reject(new DOMExceptionC(message, 'NetworkError'));
  }

  // `FetchEvent.handled`: resolved when the event's outcome is known and good, rejected for a
  // network error (a canceled event nobody answered, an invalid or rejected `respondWith` value).
  function handledSettled(d, error) {
    const settlers = d.handledSettlers;
    if (!settlers) return;
    d.handledSettlers = null;
    if (error === undefined) settlers.resolve(undefined);
    else settlers.reject(error);
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
        handledSettled(d);
        stream(id, response);
      },
    ).then(undefined, (error) => {
      handledSettled(d, error);
      native.failFetch(id, String(error && error.message || error));
    });
  }

  // What the handlers' outcome means once the microtask checkpoint after them is over (a
  // `respondWith()` call may come from a microtask): the response, a network error when the event
  // was canceled and nobody answered, otherwise the network's answer.
  function decide(job, d) {
    if (d.responded) {
      settle(job, d);
    } else if (d.event.defaultPrevented) {
      handledSettled(d, new TypeErrorC('The fetch event was canceled without a response'));
      native.failFetch(job.id, 'The fetch event was canceled without a response');
    } else {
      handledSettled(d);
      native.passthrough(job.id);
    }
  }

  function dispatchFetch(job) {
    const source = route(job);
    if (source.type === 'network') {
      native.passthrough(job.id);
      return;
    }
    if (source.type === 'cache') {
      fromCache(job, source);
      return;
    }
    // 'race': the network request starts before the handler runs (it may run long); the native
    // side answers with the first good response, and `passthrough` below means "the network's".
    if (source.type === 'race') native.startRace(job.id);
    const d = fire(job, startPreload(job));
    if (d === null) {
      native.passthrough(job.id);
      return;
    }
    extendData.get(d.event).after = () => decide(job, d);
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
    ['ExtendableEvent', ExtendableEvent], ['InstallEvent', InstallEvent], ['FetchEvent', FetchEvent],
    ['ExtendableMessageEvent', ExtendableMessageEvent], ['Client', Client], ['WindowClient', WindowClient],
    ['Clients', Clients], ['ServiceWorker', ServiceWorker], ['ServiceWorkerRegistration', ServiceWorkerRegistration],
    ['NavigationPreloadManager', NavigationPreloadManager],
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
    closeDispatch,
    cancelFetch,
    preloadDone,
    preloadFailed,
    dispatchMessage,
    dispatchMessageError,
    reply,
  };
})
