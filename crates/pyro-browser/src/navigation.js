// The Navigation API (`window.navigation`) for one realm, made on its first
// use. `native` reaches the frame's session history (nav.rs):
//   watch(hook): hook('navigate', type, url, hashChange, cancelable,
//     canIntercept, destinationKey) fires `navigate` for a navigation the
//     page started some other way (link, location, history, traversal) and
//     returns 'proceed', 'cancel' or 'intercept'; hook('commit', type) follows
//     once it committed in this document.
//   entries() -> [currentIndex, [key, id, url, sameDocument, stateJSON]...]
//   resolve(url) -> absolute URL (SyntaxError otherwise)
//   navigate(url, replace, stateJSON) -> whether it stayed in the document
//   commit(url, type, stateJSON): an intercepted navigation's commit
//   reload(); traverse(key) -> 'missing' | 'current' | 'queued';
//   updateState(stateJSON); reportException(error).
// Navigation API state is kept as JSON, as history.state is. Events are made
// by this script, so their isTrusted is false.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const defineProperty = O.defineProperty;
  const EventTargetC = EventTarget;
  const EventC = Event;
  const ErrorEventC = ErrorEvent;
  const AbortControllerC = AbortController;
  const DOMExceptionC = DOMException;
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const parse = JSON.parse;
  const URLC = URL;
  const ArrayFromKeys = (map) => Array.from(map.keys());
  const stringify = JSON.stringify;
  const addListener = EventTargetC.prototype.addEventListener;
  const dispatch = EventTargetC.prototype.dispatchEvent;
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ownTag = (o, name) => defineProperty(o, Symbol.toStringTag, { value: name, configurable: true });
  const abortError = (message) => new DOMExceptionC(message, 'AbortError');
  const invalidState = (message) => new DOMExceptionC(message, 'InvalidStateError');

  // Platform objects (a DOM node) cannot be the state: HTML serializes it
  // with StructuredSerializeForStorage, which throws DataCloneError for them.
  const structuredCloneF = typeof structuredClone === 'function' ? structuredClone : null;
  const serialize = (value) => {
    if (value === undefined) return undefined;
    // Cloned once (getters run once), then kept as JSON.
    const plain = structuredCloneF !== null ? structuredCloneF(value) : value;
    let json;
    try {
      json = stringify(plain);
    } catch (e) {
      json = undefined;
    }
    if (json === undefined) throw new DOMExceptionC('The state could not be cloned.', 'DataCloneError');
    return json;
  };
  const deserialize = (json) => (json === undefined ? undefined : parse(json));

  // Event handler attributes (`onnavigate`, ...) over one listener each.
  const handlerAttributes = (C, types, store) => {
    for (const type of types) {
      defineProperty(C.prototype, 'on' + type, {
        get() {
          return store(this).handlers[type] ?? null;
        },
        set(value) {
          const d = store(this);
          const had = d.handlers[type] !== undefined;
          d.handlers[type] = typeof value === 'function' || (value !== null && typeof value === 'object') ? value : null;
          if (!had) {
            addListener.call(this, type, (e) => {
              const h = d.handlers[type];
              if (typeof h === 'function') return h.call(this, e);
            });
          }
        },
        enumerable: true,
        configurable: true,
      });
    }
  };

  // ----- entries -----------------------------------------------------------
  const entryData = new WeakMap();
  const entryObjects = new Map();
  class NavigationHistoryEntry extends EventTargetC {
    constructor(t, row) {
      if (t !== token) throw illegal('NavigationHistoryEntry');
      super();
      ownTag(this, 'NavigationHistoryEntry');
      entryData.set(this, { row, handlers: {} });
    }
    // HTML: an entry of a document that is not fully active reads as
    // "", "", null, -1 and false.
    get key() {
      return native.active() ? entryData.get(this).row.key : '';
    }
    get id() {
      return native.active() ? entryData.get(this).row.id : '';
    }
    get url() {
      return native.active() ? entryData.get(this).row.url : null;
    }
    get index() {
      return native.active() ? snapshot().list.indexOf(this) : -1;
    }
    get sameDocument() {
      return native.active() && entryData.get(this).row.same;
    }
    getState() {
      return deserialize(entryData.get(this).row.state);
    }
  }
  handlerAttributes(NavigationHistoryEntry, ['dispose'], (o) => entryData.get(o));
  tag(NavigationHistoryEntry, 'NavigationHistoryEntry');

  const snapshot = () => {
    const raw = native.entries();
    const rows = [];
    for (let i = 1; i < raw.length; i++) {
      const r = raw[i];
      rows.push({ key: r[0], id: r[1], url: r[2], same: r[3], state: r[4] });
    }
    const ids = new Set(rows.map((r) => r.id));
    for (const id of ArrayFromKeys(entryObjects)) if (!ids.has(id)) entryObjects.delete(id);
    const list = rows.map((row) => {
      let o = entryObjects.get(row.id);
      if (o === undefined) {
        o = new NavigationHistoryEntry(token, row);
        entryObjects.set(row.id, o);
      } else {
        entryData.get(o).row = row;
      }
      return o;
    });
    return { index: raw[0], list };
  };
  const current = () => {
    const { index, list } = snapshot();
    return index >= 0 ? list[index] : null;
  };

  // ----- destination, events, transition -----------------------------------
  const destinationData = new WeakMap();
  class NavigationDestination {
    constructor(t, d) {
      if (t !== token) throw illegal('NavigationDestination');
      destinationData.set(this, d);
    }
    get url() {
      return destinationData.get(this).url;
    }
    // A destination entry of a document no longer fully active reads as
    // "", "" and -1 (HTML).
    get key() {
      return native.active() ? destinationData.get(this).key : '';
    }
    get id() {
      return native.active() ? destinationData.get(this).id : '';
    }
    get index() {
      return native.active() ? destinationData.get(this).index : -1;
    }
    get sameDocument() {
      return destinationData.get(this).sameDocument;
    }
    getState() {
      return deserialize(destinationData.get(this).state);
    }
  }
  tag(NavigationDestination, 'NavigationDestination');

  const eventData = new WeakMap();
  const NAVIGATION_TYPES = ['push', 'replace', 'reload', 'traverse'];
  class NavigateEvent extends EventC {
    constructor(type, init) {
      if (arguments.length < 2) {
        throw new TypeErrorC(`Failed to construct 'NavigateEvent': 2 arguments required, but only ${arguments.length} present.`);
      }
      if (init === null || typeof init !== 'object') {
        throw new TypeErrorC("Failed to construct 'NavigateEvent': The provided value is not of type 'NavigateEventInit'.");
      }
      if (!(init.destination instanceof NavigationDestination)) {
        throw new TypeErrorC("Failed to construct 'NavigateEvent': Failed to read the 'destination' property from 'NavigateEventInit': Required member is undefined.");
      }
      if (!(init.signal instanceof AbortSignal)) {
        throw new TypeErrorC("Failed to construct 'NavigateEvent': Failed to read the 'signal' property from 'NavigateEventInit': Required member is undefined.");
      }
      super(type, init);
      const navigationType = init.navigationType === undefined ? 'push' : String(init.navigationType);
      if (!NAVIGATION_TYPES.includes(navigationType)) {
        throw new TypeErrorC(`Failed to construct 'NavigateEvent': The provided value '${navigationType}' is not a valid enum value of type NavigationType.`);
      }
      eventData.set(this, {
        navigationType,
        destination: init.destination,
        canIntercept: !!init.canIntercept,
        userInitiated: !!init.userInitiated,
        hashChange: !!init.hashChange,
        signal: init.signal,
        formData: init.formData ?? null,
        downloadRequest: init.downloadRequest ?? null,
        info: init.info,
        hasUAVisualTransition: !!init.hasUAVisualTransition,
        sourceElement: init.sourceElement ?? null,
        handlers: [],
        dispatching: false,
      });
    }
    intercept(options) {
      const d = eventData.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      if (!native.active()) {
        throw invalidState("Failed to execute 'intercept' on 'NavigateEvent': The document is not fully active.");
      }
      if (!d.dispatching) {
        throw invalidState("Failed to execute 'intercept' on 'NavigateEvent': intercept() may only be called while the navigate event is being dispatched.");
      }
      if (!d.canIntercept) {
        throw new DOMExceptionC("Failed to execute 'intercept' on 'NavigateEvent': A navigation with URL '" + d.destination.url + "' cannot be intercepted by in a window with origin '" + location.origin + "'.", 'SecurityError');
      }
      if (this.defaultPrevented) {
        throw invalidState("Failed to execute 'intercept' on 'NavigateEvent': intercept() may not be called on a canceled event.");
      }
      const handler = options === undefined || options === null ? undefined : options.handler;
      if (handler !== undefined && typeof handler !== 'function') {
        throw new TypeErrorC("Failed to execute 'intercept' on 'NavigateEvent': Failed to read the 'handler' property from 'NavigationInterceptOptions': The provided value is not a function.");
      }
      d.handlers.push(handler === undefined ? () => undefined : handler);
    }
    scroll() {
      const d = eventData.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      if (d.handlers.length === 0) {
        throw invalidState("Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called on an intercepted navigation.");
      }
    }
  }
  for (const key of ['navigationType', 'destination', 'canIntercept', 'userInitiated', 'hashChange', 'signal', 'formData', 'downloadRequest', 'info', 'hasUAVisualTransition', 'sourceElement']) {
    defineProperty(NavigateEvent.prototype, key, {
      get() {
        const d = eventData.get(this);
        if (d === undefined) throw new TypeErrorC('Illegal invocation');
        return d[key];
      },
      enumerable: true,
      configurable: true,
    });
  }
  tag(NavigateEvent, 'NavigateEvent');

  const changeData = new WeakMap();
  class NavigationCurrentEntryChangeEvent extends EventC {
    constructor(type, init) {
      if (arguments.length < 2) {
        throw new TypeErrorC(`Failed to construct 'NavigationCurrentEntryChangeEvent': 2 arguments required, but only ${arguments.length} present.`);
      }
      if (init === null || typeof init !== 'object' || !(init.from instanceof NavigationHistoryEntry)) {
        throw new TypeErrorC("Failed to construct 'NavigationCurrentEntryChangeEvent': Failed to read the 'from' property from 'NavigationCurrentEntryChangeEventInit': Required member is undefined.");
      }
      super(type, init);
      changeData.set(this, { navigationType: init.navigationType ?? null, from: init.from });
    }
    get navigationType() {
      return changeData.get(this).navigationType;
    }
    get from() {
      return changeData.get(this).from;
    }
  }
  tag(NavigationCurrentEntryChangeEvent, 'NavigationCurrentEntryChangeEvent');

  const transitionData = new WeakMap();
  class NavigationTransition {
    constructor(t, d) {
      if (t !== token) throw illegal('NavigationTransition');
      transitionData.set(this, d);
    }
    get navigationType() {
      return transitionData.get(this).navigationType;
    }
    get from() {
      return transitionData.get(this).from;
    }
    get finished() {
      return transitionData.get(this).finished;
    }
  }
  tag(NavigationTransition, 'NavigationTransition');

  // ----- the ongoing navigation --------------------------------------------
  // {committed, finished} of navigation.navigate() and friends; rejections
  // are marked handled, as the API's own promises are.
  const deferred = () => {
    let resolve, reject;
    const promise = new PromiseC((res, rej) => {
      resolve = res;
      reject = rej;
    });
    promise.catch(() => {});
    return { promise, resolve, reject };
  };
  const newResult = () => ({ committed: deferred(), finished: deferred() });
  const apiResult = (r) => ({ committed: r.committed.promise, finished: r.finished.promise });
  const rejectedResult = (error) => {
    const r = newResult();
    r.committed.reject(error);
    r.finished.reject(error);
    return apiResult(r);
  };

  let ongoing = null;
  let transition = null;
  let lastEntry = null;
  const pendingTraversals = [];

  // HTML "abort the ongoing navigation" / "reject the finished promise":
  // `navigateerror` fires while `navigation.transition` is still this one,
  // then the transition's `finished` rejects and it is cleared.
  const fail = (o, error) => {
    if (ongoing === o) ongoing = null;
    if (!o.controller.signal.aborted) o.controller.abort(error);
    const ev = new ErrorEventC('navigateerror', { error, message: error && error.message !== undefined ? String(error.message) : String(error) });
    dispatch.call(navigation, ev);
    if (o.result !== null) {
      o.result.committed.reject(error);
      o.result.finished.reject(error);
    }
    if (o.transition !== null) {
      o.transitionFinished.reject(error);
      if (transition === o.transition) transition = null;
    }
  };

  const fireNavigate = (type, url, hashChange, cancelable, canIntercept, key, result, info, state) => {
    if (ongoing !== null) fail(ongoing, abortError('The navigation was aborted by a new navigation.'));
    const from = current();
    let destination;
    if (type === 'traverse') {
      const { list } = snapshot();
      const index = list.findIndex((e) => e.key === key);
      const target = index >= 0 ? list[index] : null;
      destination = new NavigationDestination(token, {
        url, key: target ? target.key : '', id: target ? target.id : '', index,
        sameDocument: target ? target.sameDocument : false, state: target ? entryData.get(target).row.state : undefined,
      });
      const pending = pendingTraversals.findIndex((p) => p.key === key);
      if (pending >= 0) {
        const p = pendingTraversals.splice(pending, 1)[0];
        result = p.result;
        info = p.info;
      }
    } else {
      destination = new NavigationDestination(token, { url, key: '', id: '', index: -1, sameDocument: hashChange, state });
    }
    const controller = new AbortControllerC();
    const event = new NavigateEvent('navigate', {
      navigationType: type, destination, canIntercept, userInitiated: false, hashChange, signal: controller.signal,
      info, cancelable,
    });
    const d = eventData.get(event);
    const o = { type, from, result, controller, handlers: [], transition: null, transitionFinished: null, state };
    ongoing = o;
    d.dispatching = true;
    try {
      dispatch.call(navigation, event);
    } finally {
      d.dispatching = false;
    }
    if (ongoing !== o) return 'proceed';
    if (event.defaultPrevented && cancelable) {
      fail(o, abortError('Navigation was aborted'));
      return 'cancel';
    }
    if (d.handlers.length > 0) {
      o.handlers = d.handlers;
      o.transitionFinished = deferred();
      o.transition = new NavigationTransition(token, { navigationType: type, from, finished: o.transitionFinished.promise });
      transition = o.transition;
      return 'intercept';
    }
    return 'proceed';
  };

  const onCommit = (type) => {
    const o = ongoing;
    const entry = current();
    const from = o !== null && o.from !== null ? o.from : lastEntry;
    lastEntry = entry;
    if (type !== 'reload' && from !== null) {
      dispatch.call(navigation, new NavigationCurrentEntryChangeEvent('currententrychange', { navigationType: type, from }));
    }
    if (o === null) return;
    if (o.result !== null) o.result.committed.resolve(entry);
    const runs = o.handlers.map((handler) => {
      try {
        return PromiseC.resolve(handler());
      } catch (e) {
        return PromiseC.reject(e);
      }
    });
    PromiseC.all(runs).then(
      () => {
        if (ongoing !== o) return;
        ongoing = null;
        // HTML: `navigatesuccess` fires with `navigation.transition` still
        // set; then the method's and the transition's `finished` resolve and
        // the transition is cleared.
        dispatch.call(navigation, new EventC('navigatesuccess'));
        if (o.result !== null) o.result.finished.resolve(entry);
        if (o.transitionFinished !== null) o.transitionFinished.resolve();
        if (o.transition !== null && transition === o.transition) transition = null;
      },
      (error) => {
        if (ongoing === o) fail(o, error);
      },
    );
  };

  native.watch((kind, a, b, c, d, e, f) => {
    if (kind === 'navigate') return fireNavigate(a, b, c, d, e, f, null, undefined, undefined);
    if (kind === 'commit') onCommit(a);
    return undefined;
  });

  // Navigations this script starts: the event first, then the commit.
  const start = (type, url, info, state) => {
    const result = newResult();
    const target = new URLC(url);
    const here = new URLC(location.href);
    const hashChange = type !== 'reload' && target.hash !== '' && target.hash !== here.hash
      && target.href.split('#')[0] === here.href.split('#')[0];
    const canIntercept = target.origin === here.origin && target.protocol === here.protocol;
    const decision = fireNavigate(type, url, hashChange, true, canIntercept, '', result, info, state);
    if (decision === 'cancel') return apiResult(result);
    if (decision === 'intercept') {
      native.commit(url, type, state);
      onCommit(type);
    } else if (type === 'reload') {
      native.reload();
    } else if (native.navigate(url, type === 'replace', state)) {
      onCommit(type);
    }
    return apiResult(result);
  };

  const traverse = (key, info) => {
    if (!native.active()) return rejectedResult(invalidState('The document is not fully active.'));
    const answer = native.traverse(key);
    if (answer === 'missing') {
      return rejectedResult(invalidState("Failed to execute 'traverseTo' on 'Navigation': Invalid key"));
    }
    const result = newResult();
    if (answer === 'current') {
      const entry = current();
      result.committed.resolve(entry);
      result.finished.resolve(entry);
    } else {
      pendingTraversals.push({ key, result, info });
    }
    return apiResult(result);
  };

  const navigationData = { handlers: {} };
  class Navigation extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('Navigation');
      super();
      ownTag(this, 'Navigation');
    }
    entries() {
      return native.active() ? snapshot().list : [];
    }
    get currentEntry() {
      return native.active() ? current() : null;
    }
    get canGoBack() {
      return native.active() && snapshot().index > 0;
    }
    get canGoForward() {
      if (!native.active()) return false;
      const { index, list } = snapshot();
      return index >= 0 && index < list.length - 1;
    }
    get transition() {
      return transition;
    }
    updateCurrentEntry(options) {
      if (options === null || typeof options !== 'object' || !('state' in options)) {
        throw new TypeErrorC("Failed to execute 'updateCurrentEntry' on 'Navigation': Failed to read the 'state' property from 'NavigationUpdateCurrentEntryOptions': Required member is undefined.");
      }
      native.updateState(serialize(options.state));
      const from = lastEntry ?? current();
      lastEntry = current();
      dispatch.call(this, new NavigationCurrentEntryChangeEvent('currententrychange', { navigationType: null, from }));
    }
    navigate(url, options) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'navigate' on 'Navigation': 1 argument required, but only 0 present.");
      }
      options = options ?? {};
      const history = options.history === undefined ? 'auto' : String(options.history);
      if (!['auto', 'push', 'replace'].includes(history)) {
        throw new TypeErrorC(`Failed to execute 'navigate' on 'Navigation': Failed to read the 'history' property from 'NavigationNavigateOptions': The provided value '${history}' is not a valid enum value of type NavigationHistoryBehavior.`);
      }
      let target, state;
      try {
        target = native.resolve(String(url));
        state = serialize(options.state);
      } catch (e) {
        return rejectedResult(e);
      }
      if (target.startsWith('javascript:')) {
        return rejectedResult(new DOMExceptionC('A navigation to a javascript: URL cannot be done with navigation.navigate().', 'NotSupportedError'));
      }
      if (!native.active()) return rejectedResult(invalidState("Failed to execute 'navigate' on 'Navigation': The document is not fully active."));
      const replace = history === 'replace' || (history === 'auto' && target === location.href);
      return start(replace ? 'replace' : 'push', target, options.info, state);
    }
    reload(options) {
      options = options ?? {};
      let state;
      try {
        state = 'state' in options ? serialize(options.state) : entryData.get(current())?.row.state;
      } catch (e) {
        return rejectedResult(e);
      }
      if (!native.active()) return rejectedResult(invalidState("Failed to execute 'reload' on 'Navigation': The document is not fully active."));
      return start('reload', location.href, options.info, state);
    }
    traverseTo(key, options) {
      return traverse(String(key), (options ?? {}).info);
    }
    back(options) {
      if (!native.active()) return rejectedResult(invalidState("Failed to execute 'back' on 'Navigation': The document is not fully active."));
      const { index, list } = snapshot();
      if (index <= 0) return rejectedResult(invalidState("Failed to execute 'back' on 'Navigation': Cannot go back."));
      return traverse(list[index - 1].key, (options ?? {}).info);
    }
    forward(options) {
      if (!native.active()) return rejectedResult(invalidState("Failed to execute 'forward' on 'Navigation': The document is not fully active."));
      const { index, list } = snapshot();
      if (index < 0 || index >= list.length - 1) {
        return rejectedResult(invalidState("Failed to execute 'forward' on 'Navigation': Cannot go forward."));
      }
      return traverse(list[index + 1].key, (options ?? {}).info);
    }
  }
  handlerAttributes(Navigation, ['navigate', 'navigatesuccess', 'navigateerror', 'currententrychange'], () => navigationData);
  tag(Navigation, 'Navigation');

  const navigation = new Navigation(token);
  lastEntry = current();
  return {
    navigation, Navigation, NavigateEvent, NavigationHistoryEntry, NavigationDestination, NavigationTransition,
    NavigationCurrentEntryChangeEvent,
  };
})
