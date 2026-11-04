# Event handler attributes

`on*` IDL attributes from GlobalEventHandlers are defined on `HTMLElement`,
`SVGElement`, `Document` (plus `onreadystatechange`/`onvisibilitychange`) and
`Window` (plus WindowEventHandlers such as `onpopstate`, `onhashchange`,
`onmessage`, `onpageshow`). Touch handlers are not exposed (desktop UA).

- Setting a function installs it in an ordinary listener slot, so it runs in
  registration order with `addEventListener` listeners; replacing it keeps the
  position; non-functions set `null`.
- A handler returning `false` cancels the event, except `mouseover` and
  `error` (HTML "process the event handler return value").
- `setAttribute('onx', body)` on an HTML or SVG element compiles
  `function (event) { body }` in the calling realm, with the document, form
  owner and element on its scope chain, and installs it;
  `removeAttribute('onx')` clears it. This also satisfies ReactDOM's
  `isEventSupported` probes (`'oninput' in document`).
- `on*` attributes from the HTML parser, `innerHTML` and cloning are compiled in
  the main world before the next event dispatch, `addEventListener` or handler
  access. `<body>`/`<frameset>` attributes in the Window-reflecting set
  (`onload`, `onhashchange`, ...) install on the Window.
- Window handler state lives on the per-realm window holder because page
  realms use the default JavaScriptCore global.

Not implemented: handlers of nodes adopted out of a `<template>` without
cloning, the `evt` parameter name on SVG elements, the source text of a compiled
handler (`toString()` shows the scope-chain wrapper), the 5-argument
`window.onerror` form, `onbeforeunload` return value semantics, and reflecting
IDL assignments back into content attributes.
