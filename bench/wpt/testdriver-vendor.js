// testdriver-vendor.js for pyro (README "testdriver"): WPT's testdriver.js calls window.test_driver_internal, and
// this file implements it on top of the runner, which drives the browser over CDP as wptrunner drives Chrome over
// WebDriver. Each call goes to the runner through the binding run.mjs exposes on every context
// (window.__pyroTestdriver, a function returning a promise of the runner's JSON answer). Elements cannot cross the
// binding: they go as their center in this frame's viewport, and the runner adds the offsets of the frames around it.
// Implemented: click, send_keys, action_sequence (pointer, key, wheel and none sources), and with them bless, and
// set_permission, get_computed_role, get_computed_label (from Pyro's accessibility tree) and get_accessibility_properties_for_element / _for_accessibility_node, delete_all_cookies; get_named_cookie and get_all_cookies;
// bidi.log.entry_added, bidi.permissions.set_permission and the bidi.emulation overrides the browser really has
// (geolocation, locale, touch, prefers-color-scheme/reduced-motion), and WebDriver's virtual authenticators. The rest keeps testdriver.js's own "not
// implemented" rejections. A command group is added with one `def` line below (td/index.mjs says how).
(function () {
  'use strict';
  const internal = window.test_driver_internal;
  if (!internal || typeof window.__pyroTestdriver !== 'function') return;
  internal.in_automation = true;

  const call = async (cmd) => {
    const r = JSON.parse(await window.__pyroTestdriver(JSON.stringify(cmd)));
    if (r && r.error) throw new Error(r.error);
    return r.result;
  };

  // Generic commands (td/index.mjs). `def('a.b.c', { elements, check })` makes `internal.a.b.c(...args)` send
  // `{ cmd: 'a.b.c', args }` to the runner's handler of that name. `elements`: indexes of element arguments, sent as
  // `{ element: id }` (the element carries a marker attribute for the call). A window anywhere in the arguments (a
  // `context` parameter) is sent as `{ window: id }`, the window carrying a mark the runner reads. `check(...args)`
  // runs synchronously first (WebDriver's invalid-argument errors throw before anything is sent).
  let marks = 0;
  const isWindow = (v) => { try { return !!v && typeof v === 'object' && v.window === v; } catch { return false; } };
  async function rpc(name, args, { elements = [], check } = {}) {
    const undo = [];
    const encode = (v, i) => {
      if (i !== undefined && elements.includes(i)) {
        if (!(v && v.nodeType === 1)) throw new Error('testdriver: not an element');
        const id = String(++marks);
        v.setAttribute('data-pyro-testdriver', id);
        undo.push(() => v.removeAttribute('data-pyro-testdriver'));
        return { element: id };
      }
      if (isWindow(v)) {
        const id = String(++marks);
        try { v.__pyroTdWindow = id; undo.push(() => { try { delete v.__pyroTdWindow; } catch {} }); } catch { throw new Error('testdriver: cannot address a cross-origin window'); }
        return { window: id };
      }
      if (Array.isArray(v)) return v.map((x) => encode(x));
      if (v && typeof v === 'object' && Object.getPrototypeOf(v) === Object.prototype) return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, encode(x)]));
      return v;
    };
    try { return await call({ cmd: name, args: args.map((v, i) => encode(v, i)) }); } finally { undo.forEach((f) => f()); }
  }
  function def(name, opts = {}) {
    const path = name.split('.');
    const fn = path.pop();
    let o = internal;
    for (const k of path) o = o[k] ??= {};
    o[fn] = (...args) => {
      if (opts.check) opts.check(...args);
      return rpc(name, args, opts);
    };
  }

  // Events from the runner (`window.__pyroTdEvent(name, data)`, td/index.mjs `ctx.emit`) to the listeners of `onEvent`.
  const listeners = new Map();
  const onEvent = (name, fn) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(fn);
    return () => listeners.get(name).delete(fn);
  };
  window.__pyroTdEvent = (name, data) => { for (const fn of [...(listeners.get(name) ?? [])]) { try { fn(data); } catch (e) { setTimeout(() => { throw e; }); } } };
  // WebDriver's in-view center point of the element's first client rect, after scrolling it into view.
  function center(el) {
    if (!(el instanceof Element) && !(el && el.nodeType === 1)) throw new Error('testdriver: not an element');
    if (!el.isConnected) throw new Error('testdriver: element is not attached');
    el.scrollIntoView({ block: 'end', inline: 'nearest' });
    const rect = el.getClientRects()[0] ?? el.getBoundingClientRect();
    const left = Math.max(0, rect.left), right = Math.min(window.innerWidth, rect.right);
    const top = Math.max(0, rect.top), bottom = Math.min(window.innerHeight, rect.bottom);
    return { x: Math.floor((left + right) / 2), y: Math.floor((top + bottom) / 2) };
  }

  internal.click = (el) => call({ cmd: 'click', at: center(el) });

  internal.send_keys = (el, keys) => {
    // WebDriver's Element Send Keys focuses the element (it does not click it), then types. It scrolls nothing
    // itself: scrolling `<body>` into view (as `center` does) moved the viewport to the end of a tall page before
    // the key was sent, so a keyboard scroll test saw the document scroll first (css-overflow/keyboard-scroll.html).
    // (An element of another realm or without a box is fine: only its focus matters.)
    if (!(el && el.nodeType === 1)) throw new Error('testdriver: not an element');
    if (document.activeElement !== el) el.focus();
    return call({ cmd: 'send_keys', keys: String(keys) });
  };

  // The permission for this document's origin (WebDriver's Set Permission), in the page's browser context.
  internal.set_permission = (params) => call({ cmd: 'set_permission', descriptor: params.descriptor, state: params.state, origin: location.origin });

  // Computed role and name (WebDriver's Get Computed Role / Label) and the accessibility properties of an element or
  // node, from Pyro's accessibility tree (td/aria.mjs).
  def('get_computed_role', { elements: [0] });
  def('get_computed_label', { elements: [0] });
  def('get_accessibility_properties_for_element', { elements: [0] });
  def('get_accessibility_properties_for_accessibility_node');

  // WebDriver's Delete All Cookies: the browser context's cookies.
  internal.delete_all_cookies = () => call({ cmd: 'delete_all_cookies' });

  // WebDriver's Get All Cookies / Get Named Cookie for the current document's URL (td/cookies.mjs). testdriver.js turns a
  // missing cookie (null) into its "no such cookie" error.
  def('get_all_cookies');
  def('get_named_cookie');
  def('generate_test_report');
  def('create_virtual_pressure_source');
  def('update_virtual_pressure_source');
  def('remove_virtual_pressure_source');
  // WebDriver's Virtual Authenticators (td/webauthn.mjs).
  for (const name of ['add_virtual_authenticator', 'remove_virtual_authenticator', 'add_credential', 'get_credentials', 'remove_credential', 'remove_all_credentials', 'set_user_verified', 'set_credential_properties']) def(name);
  def('create_virtual_sensor'); def('update_virtual_sensor'); def('remove_virtual_sensor'); def('get_virtual_sensor_information');

  // WebDriver BiDi (td/bidi.mjs). `subscribe` returns the function that removes the subscription.
  const bidiError = (m) => new Error(`invalid argument: ${m}`);
  // An entry reaches every listener once however many subscriptions matched it (the runner sends it once per subscription).
  const seen = new Set(), logListeners = new Set();
  onEvent('log.entry_added', (e) => {
    if (seen.has(e.id)) return;
    seen.add(e.id);
    if (seen.size > 4096) seen.delete(seen.values().next().value);
    for (const fn of [...logListeners]) { try { fn(e.entry); } catch (err) { setTimeout(() => { throw err; }); } }
  });
  internal.bidi ??= {};
  internal.bidi.log ??= {};
  internal.bidi.log.entry_added = {
    async subscribe(params = {}) {
      const { id } = await rpc('bidi.log.entry_added.subscribe', [params ?? {}]);
      return () => rpc('bidi.log.entry_added.unsubscribe', [id]);
    },
    on(callback) { return logListeners.add(callback), () => logListeners.delete(callback); },
  };
  def('bidi.permissions.set_permission', { check: (p) => { if (!p || !p.descriptor || typeof p.descriptor.name !== 'string' || !['granted', 'denied', 'prompt'].includes(p.state)) throw bidiError('descriptor and state are required'); } });
  def('bidi.emulation.set_geolocation_override', { check: (p = {}) => {
    const c = p.coordinates;
    if (c && p.error) throw bidiError('coordinates and error are mutually exclusive');
    if (p.error && p.error.type !== 'positionUnavailable') throw bidiError('unknown geolocation error type');
    if (c && (!Number.isFinite(c.latitude) || Math.abs(c.latitude) > 90 || !Number.isFinite(c.longitude) || Math.abs(c.longitude) > 180 || !(c.accuracy === undefined || c.accuracy >= 0))) throw bidiError('invalid coordinates');
  } });
  def('bidi.emulation.set_locale_override', { check: (p = {}) => { if (p.locale != null && typeof p.locale !== 'string') throw bidiError('locale must be a string'); } });
  def('bidi.emulation.set_touch_override', { check: (p = {}) => { if (p.maxTouchPoints != null && !(Number.isInteger(p.maxTouchPoints) && p.maxTouchPoints >= 1)) throw bidiError('maxTouchPoints must be an integer of at least 1'); } });
  def('bidi.emulation.set_media_features_override', { check: (p = {}) => { if (p.features != null && (typeof p.features !== 'object' || Array.isArray(p.features))) throw bidiError('features must be an object'); } });

  internal.action_sequence = (sources) => {
    // Element origins become this frame's viewport coordinates of the element's center.
    const out = sources.map((s) => ({
      ...s,
      actions: s.actions.map((a) => (a.origin && typeof a.origin === 'object' ? { ...a, origin: 'element', at: center(a.origin) } : a)),
    }));
    return call({ cmd: 'actions', sources: out });
  };
})();
