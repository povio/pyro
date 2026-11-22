// navigator.serviceWorker, ServiceWorkerRegistration and ServiceWorker for
// one realm, made on its first use of one of them (service_worker.rs). The
// browser thread owns the registrations; this realm keeps one object per
// registration and per worker version, so identity holds across calls.
//   native.register(id, scriptURL, scope, type, updateViaCache)
//   native.blocked(): the context blocks registration (serviceWorkers: 'block')
//   native.getRegistration(id, clientURL), getRegistrations(id)
//   native.unregister(id, registrationId), update(id, registrationId)
//     each settles request `id` later through settle(id, ok, json, name, message)
//   native.postMessage(versionId, message, options) serializes now.
// The browser thread calls, each in its own task:
//   registration(json, updateFound): a registration's new state
//   worker(json): a version no registration holds any more became redundant
//   controller(json, fire): the new controller (controllerchange if fire)
//   message(versionJson, data, ports, origin), messageError(versionJson, origin)
// `initial`: {controller, ready, clientId, url} (JSON strings or null).
(function (native, initial) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const defineProperty = O.defineProperty;
  const parse = JSON.parse;
  const PromiseC = Promise;
  const EventTargetC = EventTarget;
  const EventC = Event;
  const MessageEventC = MessageEvent;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const addListener = EventTargetC.prototype.addEventListener;
  const dispatchEvent = EventTargetC.prototype.dispatchEvent;
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ownTag = (o, name) => defineProperty(o, Symbol.toStringTag, { value: name, configurable: true });

  // `on<type>` attributes: the stored handler runs from one listener.
  const handlers = new WeakMap();
  const handler = (C, type) => {
    defineProperty(C.prototype, 'on' + type, {
      get() {
        const map = handlers.get(this);
        return (map && map[type]) || null;
      },
      set(value) {
        let map = handlers.get(this);
        if (!map) handlers.set(this, (map = {}));
        const had = type in map;
        map[type] = typeof value === 'function' || (value !== null && typeof value === 'object') ? value : null;
        if (!had) {
          addListener.call(this, type, (event) => {
            const f = map[type];
            if (typeof f === 'function') {
              const result = f.call(this, event);
              if (result === false) event.preventDefault();
            }
          });
        }
        if (C === ServiceWorkerContainer && type === 'message' && map[type] !== null) startMessages();
      },
      enumerable: true,
      configurable: true,
    });
  };

  // -- Requests to the browser thread ---------------------------------------
  const pending = new Map();
  let nextRequest = 1;
  const request = (method, ...args) =>
    new PromiseC((resolve, reject) => {
      const id = nextRequest++;
      pending.set(id, { resolve, reject });
      try {
        native[method](id, ...args);
      } catch (e) {
        pending.delete(id);
        throw e;
      }
    });
  function settle(id, ok, json, name, message) {
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (!ok) {
      p.reject(name === 'TypeError' ? new TypeErrorC(message) : new DOMExceptionC(message, name));
      return;
    }
    const value = parse(json);
    if ('reg' in value) p.resolve(value.reg === null ? undefined : registrationOf(value.reg, false));
    else if ('regs' in value) p.resolve(value.regs.map((r) => registrationOf(r, false)));
    else p.resolve(value.value);
  }

  // -- ServiceWorker ----------------------------------------------------------
  const workers = new Map();
  const workerData = new WeakMap();
  class ServiceWorker extends EventTargetC {
    constructor(t, v) {
      if (t !== token) throw illegal('ServiceWorker');
      super();
      ownTag(this, 'ServiceWorker');
      workerData.set(this, { id: v.id, scriptURL: v.scriptURL, state: v.state });
    }
    get scriptURL() {
      return workerData.get(this).scriptURL;
    }
    get state() {
      return workerData.get(this).state;
    }
    postMessage(message, options) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'postMessage' on 'ServiceWorker': 1 argument required, but only 0 present.");
      }
      const d = workerData.get(this);
      if (d.state === 'redundant') {
        throw new DOMExceptionC("Failed to execute 'postMessage' on 'ServiceWorker': The worker is redundant.", 'InvalidStateError');
      }
      native.postMessage(d.id, message, options);
    }
  }
  tag(ServiceWorker, 'ServiceWorker');
  handler(ServiceWorker, 'statechange');
  handler(ServiceWorker, 'error');
  // The realm's object for version `v`; its state follows `v` (statechange).
  const workerOf = (v) => {
    if (v === null || v === undefined) return null;
    let worker = workers.get(v.id);
    if (!worker) {
      worker = new ServiceWorker(token, v);
      workers.set(v.id, worker);
      return worker;
    }
    const d = workerData.get(worker);
    if (d.state !== v.state) {
      d.state = v.state;
      dispatchEvent.call(worker, new EventC('statechange'));
    }
    if (v.state === 'redundant') workers.delete(v.id);
    return worker;
  };

  // -- ServiceWorkerRegistration -------------------------------------------------
  const registrations = new Map();
  const registrationData = new WeakMap();
  class NavigationPreloadManager {
    constructor(t) {
      if (t !== token) throw illegal('NavigationPreloadManager');
    }
    enable() {
      return PromiseC.resolve(undefined);
    }
    disable() {
      return PromiseC.resolve(undefined);
    }
    setHeaderValue() {
      return PromiseC.resolve(undefined);
    }
    getState() {
      return PromiseC.resolve({ enabled: false, headerValue: 'true' });
    }
  }
  tag(NavigationPreloadManager, 'NavigationPreloadManager');

  class ServiceWorkerRegistration extends EventTargetC {
    constructor(t, r) {
      if (t !== token) throw illegal('ServiceWorkerRegistration');
      super();
      ownTag(this, 'ServiceWorkerRegistration');
      // Keep both the factory and its result in the receiver's realm. The
      // same-origin private slot also recognizes receivers from another Window.
      let cookies;
      native.registrationCookies.set(this, () => cookies ??= native.cookieManager(r.id, r.scope));
      registrationData.set(this, {
        id: r.id, seq: -1, scope: r.scope, updateViaCache: r.updateViaCache,
        installing: null, waiting: null, active: null, preload: new NavigationPreloadManager(token),
      });
    }
    get scope() {
      return registrationData.get(this).scope;
    }
    get updateViaCache() {
      return registrationData.get(this).updateViaCache;
    }
    get installing() {
      return registrationData.get(this).installing;
    }
    get waiting() {
      return registrationData.get(this).waiting;
    }
    get active() {
      return registrationData.get(this).active;
    }
    get cookies() {
      const get = native.registrationCookies.get(this);
      if (!get) throw illegal('ServiceWorkerRegistration.cookies');
      return get();
    }
    get navigationPreload() {
      return registrationData.get(this).preload;
    }
    update() {
      return request('update', registrationData.get(this).id);
    }
    unregister() {
      return request('unregister', registrationData.get(this).id);
    }
    showNotification() {
      return PromiseC.reject(new TypeErrorC("Failed to execute 'showNotification' on 'ServiceWorkerRegistration': No notification permission has been granted for this origin."));
    }
    getNotifications() {
      return PromiseC.resolve([]);
    }
  }
  defineProperty(ServiceWorkerRegistration.prototype, 'cookies', { enumerable: true });
  tag(ServiceWorkerRegistration, 'ServiceWorkerRegistration');
  handler(ServiceWorkerRegistration, 'updatefound');

  // Applies snapshot `r` (newer ones only) and returns the realm's object.
  function registrationOf(r, updateFound) {
    let registration = registrations.get(r.id);
    if (!registration) {
      registration = new ServiceWorkerRegistration(token, r);
      registrations.set(r.id, registration);
    }
    const d = registrationData.get(registration);
    if (r.seq > d.seq) {
      d.seq = r.seq;
      const before = d.installing;
      d.installing = workerOf(r.installing);
      d.waiting = workerOf(r.waiting);
      d.active = workerOf(r.active);
      if (updateFound && d.installing !== null && d.installing !== before) {
        dispatchEvent.call(registration, new EventC('updatefound'));
      }
      if (r.matches && d.active !== null) resolveReady(registration);
    }
    return registration;
  }

  // -- ServiceWorkerContainer ----------------------------------------------------
  let controller = null;
  let readyResolve;
  const ready = new PromiseC((resolve) => {
    readyResolve = resolve;
  });
  let readyDone = false;
  const resolveReady = (registration) => {
    if (readyDone) return;
    readyDone = true;
    readyResolve(registration);
  };
  let messagesEnabled = false;
  const queued = [];
  function startMessages() {
    if (messagesEnabled) return;
    messagesEnabled = true;
    const list = queued.splice(0);
    for (const event of list) dispatchEvent.call(container, event);
  }

  class ServiceWorkerContainer extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('ServiceWorkerContainer');
      super();
      ownTag(this, 'ServiceWorkerContainer');
    }
    get controller() {
      return controller;
    }
    get ready() {
      return ready;
    }
    register(scriptURL, options) {
      // What Playwright's `serviceWorkers: 'block'` init script makes
      // `register` in Chrome: no checks, a warning, undefined.
      if (native.blocked()) {
        return (async () => {
          console.warn('Service Worker registration blocked by Playwright');
        })();
      }
      if (arguments.length === 0) {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'register' on 'ServiceWorkerContainer': 1 argument required, but only 0 present."));
      }
      const o = options === undefined || options === null ? {} : options;
      if (typeof o !== 'object' && typeof o !== 'function') {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'register' on 'ServiceWorkerContainer': The provided value is not of type 'RegistrationOptions'."));
      }
      try {
        const scope = o.scope === undefined ? undefined : String(o.scope);
        const type = o.type === undefined ? 'classic' : String(o.type);
        if (type !== 'classic' && type !== 'module') {
          throw new TypeErrorC(`Failed to execute 'register' on 'ServiceWorkerContainer': The provided value '${type}' is not a valid enum value of type WorkerType.`);
        }
        const updateViaCache = o.updateViaCache === undefined ? 'imports' : String(o.updateViaCache);
        if (!['imports', 'all', 'none'].includes(updateViaCache)) {
          throw new TypeErrorC(`Failed to execute 'register' on 'ServiceWorkerContainer': The provided value '${updateViaCache}' is not a valid enum value of type ServiceWorkerUpdateViaCache.`);
        }
        return request('register', String(scriptURL), scope, type, updateViaCache);
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    getRegistration(clientURL) {
      try {
        return request('getRegistration', clientURL === undefined ? undefined : String(clientURL));
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    getRegistrations() {
      return request('getRegistrations');
    }
    startMessages() {
      startMessages();
    }
  }
  tag(ServiceWorkerContainer, 'ServiceWorkerContainer');
  handler(ServiceWorkerContainer, 'controllerchange');
  handler(ServiceWorkerContainer, 'message');
  handler(ServiceWorkerContainer, 'messageerror');
  const container = new ServiceWorkerContainer(token);

  if (initial.controller !== null) controller = workerOf(parse(initial.controller));
  if (initial.ready !== null) registrationOf(parse(initial.ready), false);
  // The client message queue starts by itself once the document has loaded.
  if (typeof document === 'object' && document !== null) {
    if (document.readyState === 'loading') {
      addListener.call(document, 'DOMContentLoaded', startMessages, { once: true });
    } else {
      messagesEnabled = true;
    }
  }

  const deliver = (event) => {
    if (messagesEnabled) dispatchEvent.call(container, event);
    else queued.push(event);
  };

  return {
    container,
    ServiceWorker,
    ServiceWorkerContainer,
    ServiceWorkerRegistration,
    NavigationPreloadManager,
    settle,
    registration(json, updateFound) {
      registrationOf(parse(json), updateFound);
    },
    worker(json) {
      workerOf(parse(json));
    },
    controller(json, fire) {
      controller = json === null ? null : workerOf(parse(json));
      if (fire) dispatchEvent.call(container, new EventC('controllerchange'));
    },
    message(json, data, ports, origin) {
      const event = new MessageEventC('message', { data, origin, ports: ports || [] });
      defineProperty(event, 'source', { value: workerOf(parse(json)), configurable: true });
      deliver(event);
    },
    messageError(json, origin) {
      const event = new MessageEventC('messageerror', { origin });
      defineProperty(event, 'source', { value: workerOf(parse(json)), configurable: true });
      deliver(event);
    },
  };
})
