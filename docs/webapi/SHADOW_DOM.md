# Shadow DOM

`attachShadow({mode, clonable, serializable, delegatesFocus, slotAssignment})`
stores its options on the root, readable as `ShadowRoot.clonable`,
`serializable`, `delegatesFocus` and `slotAssignment`. A clonable root is cloned
with its host, also by a shallow clone; cloning a shadow root itself throws
NotSupportedError. `disabledFeatures: ["shadow"]` on a custom element class makes
`attachShadow` throw for that name, and the parser leave its template in place.

## Declarative roots

A `<template shadowrootmode=open|closed>` start tag attaches a root to the parent
element while parsing (html5ever's `attach_declarative_shadow` sink hook); the
template element never enters the tree, and its contents go into the root,
including when a script runs in the middle of it. The other `shadowroot*`
attributes set the root's options. An invalid host, a host that already has a
root, or an invalid mode leaves an ordinary template. A later `attachShadow` of
the same mode claims the root once, emptied, and a different mode throws.

Parsers honour the document's "allow declarative shadow roots" flag:

| Entry point | Declarative roots |
|---|---|
| Navigation, `document.write` on a document in a browsing context | yes |
| `setHTMLUnsafe` (Element, ShadowRoot), `Document.parseHTMLUnsafe` | yes |
| `document.write` on a `parseHTMLUnsafe` document, clones of such a document | yes |
| `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `createContextualFragment` | no |
| `DOMParser`, `createHTMLDocument` | no |

## Serialization

`getHTML({serializableShadowRoots, shadowRoots})` on Element and ShadowRoot writes
a root as a `<template shadowrootmode>` before the host's light children when the
root is serializable and `serializableShadowRoots` is true, or when it is listed
in `shadowRoots`. Attributes come in the order mode, delegatesfocus, serializable,
slotassignment (manual only), clonable. Without options it matches `innerHTML`.

## Slots

`assignedSlot` (Element, Text; null for closed roots), `assignedNodes({flatten})`
and `assignedElements({flatten})` follow named assignment, computed on each call.

## Focus, hit testing and events

`document.activeElement` is the focused element retargeted against the document
(the outermost shadow host for focus inside a shadow tree), and
`ShadowRoot.activeElement` the focused element retargeted against the root, or
null when focus is outside its tree. `ShadowRoot.styleSheets` lists the sheets
of the root's `<style>` and stylesheet `<link>` elements while it is connected.
`document.elementFromPoint`/`elementsFromPoint` retarget hits to hosts (each
once); CDP `DOM.getNodeForLocation` stays un-retargeted, as in Chrome.
`composedPath()` leaves out nodes of closed shadow trees the current target
cannot see. `slotchange` fires after the MutationObserver callbacks of the same
microtask checkpoint, and `ShadowRoot.onslotchange` is an event handler
attribute. Focus delegation and Tab order across shadow trees are described in
[focus navigation](FOCUS-NAVIGATION.md).

## Known gaps

- `slotAssignment: "manual"` and `shadowrootslotassignment=manual` are stored and
  serialized, but `HTMLSlotElement.assign()` is missing, so such slots stay empty.
- `shadowRoots` reads an array-like, not any iterable.
- html5ever matches `shadowrootmode` values case-sensitively, so `OPEN` leaves an
  ordinary template.
- `ElementInternals.shadowRoot` returns the host's root without the spec's
  "available to element internals" check.
- `disabledFeatures` is per Dom store, not per realm.
- The parser does not construct defined custom elements, so a parsed host stays
  un-upgraded until `define` or an explicit upgrade.
- Custom elements get `connectedCallback`, `disconnectedCallback` and
  `connectedMoveCallback` reactions (see [moveBefore](MOVE_BEFORE.md));
  `attributeChangedCallback` and `adoptedCallback` are not called.
- `slotchange` also fires for a slot whose fallback content changed while it
  has no assigned nodes.
- Parser insertions queue no MutationObserver records, so an observer cannot run
  `attachShadow` or move a template in the middle of a parse.
- Scoped custom element registries and `shadowrootadoptedstylesheets` are absent.
