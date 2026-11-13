# Focus navigation, focus appearance and pointer capture

## Focusable areas

`focus()`, a mouse press and Tab move focus only to a *focusable area*: an
element `Dom::is_focusable` accepts (connected, not disabled, a tabindex or a
naturally focusable element) that is not inert and is being rendered — not in a
`display: none` subtree and not `visibility: hidden`, as Chrome's focusable-style
check. `<area>` is exempt from the rendering check (it renders through its image
map). Anything else leaves focus where it is.

A shadow host whose root has `delegatesFocus` never takes focus itself:
`focus()` on it (and a press anywhere in it) focuses its *focus delegate* — the
first autofocus focusable area of its shadow tree, else the first focusable area
in tree order, looking into nested hosts that delegate focus too. Slotted light
children are not candidates. When focus is already inside the host nothing
moves; with no candidate nothing is focused.

A press focuses the nearest focusable (or focus-delegating) flat-tree ancestor of
the hit element: slottables go to their slot, shadow roots to their host.

## Sequential navigation (Tab, Shift+Tab)

`pyro_dom::focus_nav` follows HTML's flattened tabindex-ordered focus navigation
scopes, as Chrome's `FocusController` does. The document, every shadow host and
every slot own a scope listing their elements in tree order without entering
nested owners; elements with a positive tabindex come first (ascending, ties in
tree order), then everything else. Navigation:

- lands on focusable areas with a non-negative tabindex that are neither slots
  nor `delegatesFocus` hosts;
- enters a nested host's or slot's scope at the owner's position, unless the
  owner's tabindex is negative (its content is skipped, but navigation that
  starts inside it continues after the owner);
- after the last (first) element of a nested scope continues after (before) its
  owner; Shift+Tab reaches an owner after its scope's content.

Without a focused element Tab starts at the document's first element (Shift+Tab
at its last). At either end of the document focus stays put (there is no browser
UI to move to). Child frames are not entered. The order is computed on each key
press; nothing is kept between presses.

## `:focus-visible` and `:active`

The document keeps whether its focused element matches `:focus-visible`, decided
when focus moves, with Chrome's heuristics: after keyboard input (any keydown
without Ctrl/Meta/Alt, which also makes the current focus visible) and for
script focus before any pointer press, it matches; after a mouse press only an
element that takes typed text (text controls, editable content) matches, and
script focus keeps that pointer modality until the next key press.

`:active` matches the element under a primary-button press, and its
shadow-including ancestors, until the release; Space on a clickable control
makes it active until the keyup or until it blurs. Both states are held on the
document (`Dom::set_activated`, `Dom::set_focus_visible`), restyle like a hover
change, and reach the layout mirror (`Renderer::set_interaction_states`) for
`getComputedStyle` and screenshots.

## Pointer capture (mouse)

`setPointerCapture`, `releasePointerCapture` and `hasPointerCapture` model the
mouse pointer (`pointerId` 1) per Pointer Events: `setPointerCapture` sets the
pending capture target while a button is down (no-op otherwise), throws
`NotFoundError` for ids below 1 and `InvalidStateError` for a disconnected
element; `hasPointerCapture` reads the pending target, so it is true right
after the call. Before each mouse pointer event the pending capture is
processed: `lostpointercapture` at the old target (at the document when that
target left it), `gotpointercapture` at the new one. While captured, the pointer
events, their compatibility mouse events and boundary/hover state target the
capture element. After `pointerup` capture is released implicitly
(`lostpointercapture` before `mouseup`), and the `click` goes to the capture
target, as in Chrome.

Gaps: touch and pen pointers (`pointerId` 2 and up) keep their implicit capture
to the touchstart target; explicit `setPointerCapture`/`releasePointerCapture`
for them is ignored and `hasPointerCapture` returns false. A capture target
removed from the document is noticed at the next mouse pointer event, not at
removal.
