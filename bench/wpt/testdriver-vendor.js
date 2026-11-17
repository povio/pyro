// testdriver-vendor.js for pyro (README "testdriver"): WPT's testdriver.js calls window.test_driver_internal, and
// this file implements it on top of the runner, which drives the browser over CDP as wptrunner drives Chrome over
// WebDriver. Each call goes to the runner through the binding run.mjs exposes on every context
// (window.__pyroTestdriver, a function returning a promise of the runner's JSON answer). Elements cannot cross the
// binding: they go as their center in this frame's viewport, and the runner adds the offsets of the frames around it.
// Implemented: click, send_keys, action_sequence (pointer, key, wheel and none sources), and with them bless, and
// set_permission, get_computed_role and get_computed_label. The rest keeps testdriver.js's own "not implemented" rejections.
(function () {
  'use strict';
  const internal = window.test_driver_internal;
  if (!internal || typeof window.__pyroTestdriver !== 'function') return;
  internal.in_automation = true;

  const call = async (cmd) => {
    const r = JSON.parse(await window.__pyroTestdriver(JSON.stringify(cmd)));
    if (r && r.error) throw new Error(r.error);
    return r;
  };
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
    // WebDriver's Element Send Keys focuses the element (it does not click it), then types.
    center(el);
    if (document.activeElement !== el) el.focus();
    return call({ cmd: 'send_keys', keys: String(keys) });
  };

  // The permission for this document's origin (WebDriver's Set Permission), in the page's browser context.
  internal.set_permission = (params) => call({ cmd: 'set_permission', descriptor: params.descriptor, state: params.state, origin: location.origin });

  // Computed role and name (WebDriver's Get Computed Role / Label): the runner reads them with Playwright's
  // accessibility engine, which finds the element by a marker attribute held only for the call.
  let marks = 0;
  async function aria(el) {
    if (!(el instanceof Element)) throw new Error('testdriver: not an element');
    const id = String(++marks);
    el.setAttribute('data-pyro-testdriver', id);
    try { return await call({ cmd: 'aria', id }); } finally { el.removeAttribute('data-pyro-testdriver'); }
  }
  internal.get_computed_role = async (el) => (await aria(el)).role;
  internal.get_computed_label = async (el) => (await aria(el)).name;

  internal.action_sequence = (sources) => {
    // Element origins become this frame's viewport coordinates of the element's center.
    const out = sources.map((s) => ({
      ...s,
      actions: s.actions.map((a) => (a.origin && typeof a.origin === 'object' ? { ...a, origin: 'element', at: center(a.origin) } : a)),
    }));
    return call({ cmd: 'actions', sources: out });
  };
})();
