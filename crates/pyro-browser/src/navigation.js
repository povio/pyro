// The Navigation API (`window.navigation`) for one realm, made on its first
// use. `native` reaches the frame's session history (nav.rs):
//   watch(hook): hook('navigate', type, url, hashChange, cancelable,
//     canIntercept, destinationKey, formData, sourceElement) fires
//     `navigate` for a navigation the
//     page started some other way (link, location, history, traversal) and
//     returns 'proceed', 'cancel' or 'intercept'; hook('commit', type) follows
//     once it committed in this document.
//   entries() -> [currentIndex, [key, id, url, sameDocument, stateJSON]...]
//   resolve(url) -> absolute URL (SyntaxError otherwise)
//   navigate(url, replace, stateJSON) -> whether it stayed in the document
//   commit(url, type, stateJSON): an intercepted navigation's commit
//     (JS drives the commit of a navigation with a `precommitHandler`: its `navigate` hook
//     answers 'cancel' so that nothing happens natively, and this commits when the handlers
//     fulfilled; a traversal is queued again with traverse(key) and its hook answers 'intercept')
//   hook('stop') follows window.stop().
//   reload(); traverse(key) -> 'missing' | 'current' | 'queued';
//   updateState(stateJSON); reportException(error);
//   activation() -> null | [type, entryKey, entryId, entryUrl, null | [key, id, url, stateJSON]].
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
  const HTMLElementC = HTMLElement;
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
  // HTML "process scroll behavior". A traversal's restore was suppressed when it was
  // intercepted, so it is done here; a push, replace or reload (Chrome: also the reload, whatever
  // its saved scroll position) scrolls to the destination URL's fragment, or to the top of the
  // document when there is none or no element is indicated.
  const processScroll = (d) => {
    d.interception = 'scrolled';
    if (d.navigationType === 'traverse') native.restoreScroll();
    else native.scrollToFragment(d.destination.url);
  };
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
        precommit: [],
        trusted: false,
        dispatching: false,
        // HTML NavigateEvent "interception state" and "scroll behavior".
        interception: 'none',
        scrollBehavior: null,
        focusReset: null,
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
      const focusReset = options === undefined || options === null ? undefined : options.focusReset;
      let focusResetValue;
      if (focusReset !== undefined) {
        focusResetValue = String(focusReset);
        if (focusResetValue !== 'after-transition' && focusResetValue !== 'manual') {
          throw new TypeErrorC("Failed to execute 'intercept' on 'NavigateEvent': Failed to read the 'focusReset' property from 'NavigationInterceptOptions': The provided value '" + focusResetValue + "' is not a valid enum value of type NavigationFocusReset.");
        }
      }
      const precommitHandler = options === undefined || options === null ? undefined : options.precommitHandler;
      if (precommitHandler !== undefined && typeof precommitHandler !== 'function') {
        throw new TypeErrorC("Failed to execute 'intercept' on 'NavigateEvent': Failed to read the 'precommitHandler' property from 'NavigationInterceptOptions': The provided value is not a function.");
      }
      // HTML "intercept": a precommit handler needs a navigation that can be canceled.
      if (precommitHandler !== undefined && !this.cancelable) {
        throw invalidState("Failed to execute 'intercept' on 'NavigateEvent': intercept() with a precommitHandler may not be called on a non-cancelable event.");
      }
      const scroll = options === undefined || options === null ? undefined : options.scroll;
      if (scroll !== undefined) {
        const value = String(scroll);
        if (value !== 'after-transition' && value !== 'manual') {
          throw new TypeErrorC("Failed to execute 'intercept' on 'NavigateEvent': Failed to read the 'scroll' property from 'NavigationInterceptOptions': The provided value '" + value + "' is not a valid enum value of type NavigationScrollBehavior.");
        }
        d.scrollBehavior = value;
      }
      // The last `intercept()` that gives a `focusReset` decides.
      if (focusResetValue !== undefined) d.focusReset = focusResetValue;
      d.handlers.push(handler === undefined ? () => undefined : handler);
      if (precommitHandler !== undefined) d.precommit.push(precommitHandler);
      d.interception = 'intercepted';
    }
    scroll() {
      const d = eventData.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      if (!native.active()) {
        throw invalidState("Failed to execute 'scroll' on 'NavigateEvent': The document is not fully active.");
      }
      // HTML "ensure the event is ok to process scroll behavior": a synthetic event is never ok.
      if (!d.trusted) {
        throw new DOMExceptionC("Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called on a trusted event.", 'SecurityError');
      }
      if (d.handlers.length === 0) {
        throw invalidState("Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called on an intercepted navigation.");
      }
      // ... only once, after commit and before the navigation finished.
      if (d.interception !== 'committed') {
        throw invalidState("Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called after commit, and only once.");
      }
      processScroll(d);
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
  // HTML "potentially reset the focus" of an intercepted navigation, before `navigatesuccess` and
  // `navigateerror`: unless `focusReset: "manual"`, or the page moved the focus itself since the
  // navigation began, the focused element is blurred (focus goes back to the document). The
  // document's autofocus candidates are not processed.
  // "Focus changed during ongoing navigation" is set by any focus update, so a page that moved the
  // focus away and back still counts as having moved it: `focusEpoch` counts the focus changes.
  let focusEpoch = 0;
  const countFocus = () => { focusEpoch++; };
  addListener.call(window, 'focusin', countFocus, true);
  addListener.call(window, 'focusout', countFocus, true);
  const resetFocus = (o) => {
    if (o.handlers.length === 0 || o.event.focusReset === 'manual' || !native.active()) return;
    const active = document.activeElement;
    if (focusEpoch !== o.focusEpoch || active !== o.focused || active === null || active === document.body || active === document.documentElement) return;
    try {
      HTMLElementC.prototype.blur.call(active);
    } catch (e) {
      // Not an HTMLElement (an SVG element): left focused.
    }
  };
  // The ErrorEvent's message, as the console shows an uncaught throw ("TypeError: a message").
  const errorMessage = (error) => {
    try {
      if (error !== null && typeof error === 'object' && typeof error.name === 'string' && error.message !== undefined) {
        return error.name + ': ' + String(error.message);
      }
      return String(error);
    } catch (e) {
      return '';
    }
  };
  const fail = (o, error, resetsFocus) => {
    if (ongoing === o) ongoing = null;
    o.pending = false;
    o.event.interception = 'finished';
    if (resetsFocus === true) resetFocus(o);
    if (!o.controller.signal.aborted) o.controller.abort(error);
    // `filename` is the document's URL when the navigation began; Chrome reports the throw site
    // as the position, here any position (1:1) stands for it.
    const ev = new ErrorEventC('navigateerror', { error, message: errorMessage(error), filename: o.href, lineno: 1, colno: 1 });
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

  // HTML NavigationPrecommitController: handed to the `precommitHandler`s of an intercepted
  // navigation, which run before the navigation commits (the URL, the entries and
  // `location` are still the old ones).
  const controllerData = new WeakMap();
  const precommitData = (c, method) => {
    const o = controllerData.get(c);
    if (o === undefined) throw new TypeErrorC('Illegal invocation');
    if (!native.active()) {
      throw invalidState(`Failed to execute '${method}' on 'NavigationPrecommitController': The document is not fully active.`);
    }
    if (!o.pending || ongoing !== o) {
      throw invalidState(`Failed to execute '${method}' on 'NavigationPrecommitController': The navigation has already committed or was aborted.`);
    }
    return o;
  };
  class NavigationPrecommitController {
    constructor(t, o) {
      if (t !== token) throw illegal('NavigationPrecommitController');
      controllerData.set(this, o);
    }
    redirect(url, options) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'redirect' on 'NavigationPrecommitController': 1 argument required, but only 0 present.");
      }
      options = options ?? {};
      const history = options.history === undefined ? 'auto' : String(options.history);
      if (!['auto', 'push', 'replace'].includes(history)) {
        throw new TypeErrorC(`Failed to execute 'redirect' on 'NavigationPrecommitController': Failed to read the 'history' property from 'NavigationPrecommitControllerRedirectOptions': The provided value '${history}' is not a valid enum value of type NavigationHistoryBehavior.`);
      }
      const o = precommitData(this, 'redirect');
      // Only a push or a replace can be redirected (a reload or a traversal keeps its entry).
      if (o.type === 'reload' || o.type === 'traverse') {
        throw invalidState("Failed to execute 'redirect' on 'NavigationPrecommitController': A reload or a traversal cannot be redirected.");
      }
      const target = native.resolve(String(url));
      // HTML: the URL must "can have its URL rewritten" (same components, and the path for a
      // non-http(s) scheme), not merely share an origin (opaque origins differ).
      if (!native.canRewrite(target)) {
        throw new DOMExceptionC("Failed to execute 'redirect' on 'NavigationPrecommitController': Cannot redirect to '" + target + "' from a document with origin '" + location.origin + "'.", 'SecurityError');
      }
      // HTML serializes the state only for a navigation that has an API method tracker.
      const hasState = options.state !== undefined && o.result !== null;
      const state = hasState ? serialize(options.state) : undefined;
      if (history !== 'auto') o.type = history;
      o.url = target;
      o.event.navigationType = o.type;
      o.transitionData.navigationType = o.type;
      const destination = destinationData.get(o.event.destination);
      destination.url = target;
      if (options.info !== undefined) o.event.info = options.info;
      if (hasState) {
        o.commitState = state;
        destination.state = state;
      }
    }
    addHandler(handler) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'addHandler' on 'NavigationPrecommitController': 1 argument required, but only 0 present.");
      }
      if (typeof handler !== 'function') {
        throw new TypeErrorC("Failed to execute 'addHandler' on 'NavigationPrecommitController': The callback provided as parameter 1 is not a function.");
      }
      const o = precommitData(this, 'addHandler');
      // The handler joins the commit handlers, after those `intercept()` gave.
      o.handlers.push(handler);
    }
  }
  tag(NavigationPrecommitController, 'NavigationPrecommitController');

  // The traversal whose commit is queued again after its precommit handlers (see `commitDeferred`).
  let resume = null;
  // HTML "intercept" commit step of a navigation with precommit handlers: they fulfilled, so the
  // navigation commits now (the entry changes, `currententrychange`, the handlers run).
  const commitDeferred = (o) => {
    if (ongoing !== o) return;
    if (!native.active()) {
      fail(o, abortError('The navigation was aborted because the document is no longer active.'));
      return;
    }
    o.pending = false;
    if (o.type === 'traverse') {
      // The traversal is queued again; its `navigate` hook answers 'intercept' (see native.watch)
      // and the commit comes through the 'commit' hook.
      resume = { o, key: o.key };
      if (native.traverse(o.key) !== 'queued') {
        resume = null;
        fail(o, invalidState('The traversal could not be committed.'));
      }
      return;
    }
    native.commit(o.url, o.type, o.commitState);
    onCommit(o.type);
  };
  // Runs the `precommitHandler`s in order, then commits when all fulfilled; the first rejection
  // fails the navigation (it never committed: `committed` and `finished` reject with the reason).
  const runPrecommit = (o) => {
    const controller = new NavigationPrecommitController(token, o);
    let remaining = o.precommit.length;
    let failed = false;
    for (const handler of o.precommit) {
      let run;
      try {
        run = PromiseC.resolve(handler(controller));
      } catch (e) {
        run = PromiseC.reject(e);
      }
      run.then(
        () => {
          if (--remaining === 0 && !failed) commitDeferred(o);
        },
        (error) => {
          if (failed) return;
          failed = true;
          if (ongoing === o) fail(o, error);
        },
      );
    }
  };

  const fireNavigate = (type, url, hashChange, cancelable, canIntercept, key, result, info, state, formData, sourceElement) => {
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
      info, cancelable, formData, sourceElement,
    });
    const d = eventData.get(event);
    d.trusted = true;
    const o = {
      type, from, result, controller, handlers: [], transition: null, transitionFinished: null, state, event: d, href: location.href, focused: document.activeElement, focusEpoch,
      // A navigation with precommit handlers: not committed while `pending`; `url`, `type` and `commitState` can be redirected.
      precommit: [], pending: false, url, key, commitState: state, transitionData: null,
    };
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
      o.transitionData = { navigationType: type, from, finished: o.transitionFinished.promise };
      o.transition = new NavigationTransition(token, o.transitionData);
      transition = o.transition;
      if (d.precommit.length > 0) {
        o.precommit = d.precommit;
        o.pending = true;
        return 'defer';
      }
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
    if (o.handlers.length > 0) {
      o.event.interception = 'committed';
      o.event.scrollAtCommit = native.viewportScroll();
    }
    // HTML "react to" each handler promise directly: `navigatesuccess` fires one microtask after the
    // last handler settled, so it precedes the reactions the page attached to `committed` right after
    // the method returned. (`Promise.all` would add two ticks and put them first.)
    const succeed = () => {
        if (ongoing !== o) return;
        // HTML "potentially process scroll behavior": after the handlers, unless "manual" or
        // `scroll()` already did.
        // A traversal's restore is skipped when the page scrolled during the handlers.
        if (o.handlers.length > 0 && o.event.interception === 'committed' && o.event.scrollBehavior !== 'manual'
            && native.active()) {
          if (o.event.navigationType === 'traverse' && native.viewportScroll() !== o.event.scrollAtCommit) {
            o.event.interception = 'scrolled';
          } else {
            processScroll(o.event);
          }
        }
        resetFocus(o);
        o.event.interception = 'finished';
        ongoing = null;
        // HTML: `navigatesuccess` fires with `navigation.transition` still
        // set; then the method's and the transition's `finished` resolve and
        // the transition is cleared.
        dispatch.call(navigation, new EventC('navigatesuccess'));
        if (o.result !== null) o.result.finished.resolve(entry);
        if (o.transitionFinished !== null) o.transitionFinished.resolve();
        if (o.transition !== null && transition === o.transition) transition = null;
    };
    if (o.handlers.length === 0) {
      PromiseC.resolve().then(succeed);
      return;
    }
    let remaining = o.handlers.length;
    let failed = false;
    for (const handler of o.handlers) {
      let run;
      try {
        run = PromiseC.resolve(handler());
      } catch (e) {
        run = PromiseC.reject(e);
      }
      run.then(
        () => {
          if (--remaining === 0 && !failed) succeed();
        },
        (error) => {
          if (failed) return;
          failed = true;
          if (ongoing === o) fail(o, error, true);
        },
      );
    }
  };

  native.watch((kind, a, b, c, d, e, f, formData, sourceElement) => {
    if (kind === 'navigate') {
      // The queued-again traversal of a navigation whose precommit handlers fulfilled: it commits.
      if (resume !== null && a === 'traverse' && f === resume.key) {
        const r = resume;
        resume = null;
        if (ongoing === r.o) return 'intercept';
      }
      const decision = fireNavigate(a, b, c, d, e, f, null, undefined, undefined, formData, sourceElement);
      // Nothing happens natively until the precommit handlers fulfilled: the navigation is
      // canceled there and committed from `commitDeferred`.
      if (decision === 'defer') {
        runPrecommit(ongoing);
        return 'cancel';
      }
      return decision;
    }
    if (kind === 'commit') onCommit(a);
    // HTML "stop loading": a navigation still before its commit is aborted.
    if (kind === 'stop' && ongoing !== null && ongoing.pending) fail(ongoing, abortError('The navigation was aborted by window.stop().'));
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
    if (decision === 'defer') {
      runPrecommit(ongoing);
      return apiResult(result);
    }
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

  // HTML `navigation.activation`: how this document's navigation came about, fixed when the
  // document committed (same-document navigations leave it). `entry` and `from` are the objects of
  // the entries of `entries()` while those are in it, and keep their last object once they left
  // it (index -1).
  const activationInfo = native.activation();
  let activationObject = null;
  const activationEntries = { entry: null, from: null };
  const activationEntry = (slot, row) => {
    snapshot();
    const live = entryObjects.get(row.id);
    if (live !== undefined) {
      activationEntries[slot] = live;
      return live;
    }
    if (activationEntries[slot] === null) activationEntries[slot] = new NavigationHistoryEntry(token, row);
    return activationEntries[slot];
  };
  const makeActivation = () => {
    const [type, entryKey, entryId, entryUrl, fromRow] = activationInfo;
    return new NavigationActivation(token, {
      get entry() {
        return activationEntry('entry', { key: entryKey, id: entryId, url: entryUrl, same: true, state: undefined });
      },
      get from() {
        return fromRow === null ? null : activationEntry('from', { key: fromRow[0], id: fromRow[1], url: fromRow[2], same: false, state: fromRow[3] });
      },
      type,
    });
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
    get activation() {
      return activationInfo === null ? null : (activationObject ??= makeActivation());
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

  // HTML NavigationActivation: a cross-document navigation's entry, the entry it leaves
  // (`from`, the current one) and its type, as `pageswap` reports them.
  const activationData = new WeakMap();
  class NavigationActivation {
    constructor(t, data) {
      if (t !== token) throw illegal('NavigationActivation');
      activationData.set(this, data);
    }
    get entry() {
      return activationData.get(this).entry;
    }
    get from() {
      return activationData.get(this).from;
    }
    get navigationType() {
      return activationData.get(this).type;
    }
  }
  tag(NavigationActivation, 'NavigationActivation');
  // The activation of a cross-document navigation of this document to `url`: a reload's entry
  // is the current one, any other's a new entry of the document to come.
  const activation = (type, url) => {
    const from = current();
    const entry = type === 'reload' && from !== null
      ? from
      : new NavigationHistoryEntry(token, { key: '', id: '', url, same: false, state: undefined });
    return new NavigationActivation(token, { entry, from, type });
  };

  const navigation = new Navigation(token);
  lastEntry = current();
  return {
    navigation, Navigation, NavigateEvent, NavigationHistoryEntry, NavigationDestination, NavigationTransition,
    NavigationCurrentEntryChangeEvent, NavigationActivation, NavigationPrecommitController, activation,
  };
})
