# Node.moveBefore and the state it preserves

`parent.moveBefore(node, child)` is DOM's "move": the node changes place without
being removed and inserted, so state a removal resets is kept.

## Validity

The node and the new parent must share a shadow-including root. That is the only
connectedness rule: both may be disconnected (also across a shadow root of one
disconnected tree), while connected to disconnected and document to document
throw HierarchyRequestError. The node must be an Element or CharacterData node
and must not contain the parent; `child` must be a child of the parent
(NotFoundError). A Document parent keeps its one-element and doctype rules.

## What a move keeps

- Focus: the focused element stays `document.activeElement`, `:focus-within`
  follows its new ancestors, and no focus event fires. A later task runs the
  focus fixup rule for a moved focus: when the element landed in an `inert`
  subtree or one that is not rendered (`hidden`, `display: none`), it gets `blur`
  and `focusout` and the body is active. Chromium does this in the next
  rendering update. A text control keeps its selection and `selectionDirection`.
- Iframes: no browsing context is discarded or created, the document stays, no
  second `load` fires, and `window.frames` keeps its order.
- Modal dialogs (`:modal`) and showing popovers (`:popover-open`) stay open.
- The `nonce` content attribute (see below).
- Scripts are not prepared again.

## What a move does

- Two `childList` records, in this order: the removal from the old parent, the
  insertion into the new one. No transient observers are added.
- Live ranges and NodeIterators adjust as for a removal and an insertion.
- Slots: assign slottables for the old and new trees. A slot whose fallback
  content changed is signaled, and a slot moved out of or into a shadow tree is
  signaled when its assigned nodes changed.
- Custom elements of the moved subtree (shadow trees included), when it is
  connected: one `connectedMoveCallback` reaction each, in shadow-including
  tree order. A definition without that callback gets `disconnectedCallback`
  then `connectedCallback`, with the element connected in both.
- Style and link stylesheets take their new place in the cascade order.
- Images: an `<img>` or a `<source>` moving in or out of a `<picture>` selects
  the image source again and loads it when the URL changed.
- An option moved out of its select keeps its selectedness.

## Features this rests on

- Custom element reactions: `connectedCallback` and `disconnectedCallback` run
  for insertions and removals, when the DOM call that caused them returns. The
  callbacks are the prototype's at `define` time. Not implemented:
  `attributeChangedCallback`, `adoptedCallback`, upgrades by the parser.
- `dialog.showModal()` sets the modal flag (`:modal`); `close()` and removal
  clear it. It throws InvalidStateError for a disconnected dialog, one open as
  non-modal, or one showing as a popover. No top layer, backdrop or inertness
  of the rest of the document.
- `showPopover()`, `hidePopover()`, `togglePopover()` keep the popover
  visibility state (`:popover-open`, and the UA rule that hides a closed
  popover). Removal hides the popover. Not implemented: `beforetoggle` and
  `toggle` events, light dismiss, the auto popover stack, invokers.
- `selectionDirection` and the third argument of `setSelectionRange()`.
- `window.name`: the name of the frame (its iframe's `name` attribute) or popup.
- `<picture>`: an `<img>` child loads the first preceding `<source>` whose
  `media` matches and whose `type` is an image type; `currentSrc` reports it.
  The `<img>`'s own `srcset` and `sizes` are not read.
- Nonce hiding: a document delivered with a `Content-Security-Policy` header
  empties the `nonce` content attribute of an element that becomes connected by
  an insertion from script; `element.nonce` still returns the value. The policy
  itself is not enforced, and elements the parser inserts keep the attribute.
- A default-selected option (selected because none was) that leaves its select
  stays selected.

## Not implemented

CSS transitions and animations do not exist in the engine, so a move has none
to continue.
