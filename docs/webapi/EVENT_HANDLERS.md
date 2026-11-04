# Event handler attributes

`on*` IDL attributes from GlobalEventHandlers are defined on `HTMLElement`,
`SVGElement`, `Document` (plus `onreadystatechange`/`onvisibilitychange`) and
`Window` (plus WindowEventHandlers such as `onpopstate`, `onhashchange`,
`onmessage`, `onpageshow`). Touch handlers are only on the Window of documents under touch emulation
(`'ontouchstart' in window`), not on elements or Document, and stay until the next
navigation after emulation is turned off.

- Setting a function installs it in an ordinary listener slot, so it runs in
  registration order with `addEventListener` listeners; replacing it keeps the
  position; non-functions set `null`.
- A handler returning `false` cancels the event, except `mouseover` and
  `error` (HTML "process the event handler return value").
- `setAttribute('onx', body)` on an HTML or SVG element compiles
  `function (event) { body }` in the calling realm and installs it;
  `removeAttribute('onx')` clears it. This also satisfies ReactDOM's
  `isEventSupported` probes (`'oninput' in document`).
- Window handler state lives on the per-realm window holder because page
  realms use the default JavaScriptCore global.

Not implemented: compiling inline handler attributes created by the HTML
parser or `innerHTML`, the element/form/document scope chain of compiled
bodies, the 5-argument `window.onerror` form, `onbeforeunload` return value
semantics, and reflecting IDL assignments back into content attributes.
