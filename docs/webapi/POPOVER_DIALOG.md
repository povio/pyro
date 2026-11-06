# Popovers, dialogs, invokers and details

What lightningpanda implements of the HTML popover API, `<dialog>`,
`command`/`commandfor` invokers and `<details>` toggling, and what it does
not. Code: `crates/lp-webapi/src/popover.rs` (algorithms and bindings),
`crates/lp-dom/src/top_layer.rs` (top layer, popover stacks, inertness),
`crates/lp-dom/src/details.rs` (details attribute steps). Tests:
`crates/lp-webapi/tests/popover_dialog_js.rs`, `crates/lp-dom/tests/top_layer.rs`.

## Events

- `ToggleEvent` (`oldState`, `newState`, `source`) and `CommandEvent`
  (`command`, `source`) are constructible; `source` must be an Element or
  null (TypeError otherwise). `oncommand` is a global event handler.
- Popovers fire `beforetoggle` synchronously (cancelable when showing; a
  canceled one keeps the popover closed) and `toggle` asynchronously. Toggle
  tasks are coalesced per element: a later change before the task ran keeps
  the first old state and takes the last new state (hide then show is one
  `toggle` open to open).
- Dialogs fire the same pair from `show()`, `showModal()` and closing, as
  Chrome does, then `close` as a task.
- `<details>` fires a coalesced `toggle` (`ToggleEvent`, no bubbling, not
  cancelable) for every change of the `open` attribute's presence, however it
  changed: `details.open`, `setAttribute`/`removeAttribute`, a summary click,
  the parser (`<details open>`), the exclusive accordion. lp-dom queues the
  change at mutation time and the page posts the task.

## Popovers

- `popover` states: auto (`""`, `auto`), hint, manual (any other value).
- Showing an auto popover hides the auto popovers that are not its
  ancestors (a flat-tree ancestor popover, or its invoker's popover) and
  every hint; a hint hides the hints that are not its ancestors and no auto.
  Hiding a popover hides the ones above it in its stack first.
- `showPopover({source})`, `togglePopover({force, source})`: the source is
  the invoker for nesting and the events' `source`.
- Focus: the popover's autofocus delegate is focused when shown; hiding an
  auto/hint popover that contains the focus returns it to the element focused
  before.
- Light dismiss: a trusted pointer press and release outside the auto/hint
  popovers (both on the same "topmost clicked popover", which counts a
  popover's invoker as inside it) hides the popovers above that one, or all.
- Escape (a close request) hides the newest auto/hint popover.
- Removing a popover from its document hides it without events.

## Dialogs

- `show()` / `showModal()` follow the spec's checks and exceptions,
  `showModal()` puts the dialog in the top layer and hides the auto popovers
  that do not contain it.
- Dialog focusing steps: the autofocus delegate, else the first focusable
  descendant that is rendered (tabindex >= 0), else the dialog itself.
  Closing returns focus to the element focused before when the focus was
  inside the dialog (or nowhere).
- `requestClose(returnValue)`: a cancelable `cancel`, then the dialog closes
  unless it was canceled, whatever `closedby` says.
- `closedBy` reflects `any` / `closerequest` / `none`; without a valid
  attribute it is `closerequest` for a modal dialog and `none` otherwise.
- Escape (close request): the newest open dialog whose closed-by state is not
  `none` gets a cancelable `cancel`, then closes. Dialogs opened by setting
  `open` directly have no close watcher, as in Chrome.
- Light dismiss for `closedby=any`: a press and release outside the topmost
  open dialog (on the backdrop of a modal one) request it to close.
- `<form method=dialog>` (or `formmethod=dialog`) closes the form's dialog
  with the submitter's `value` as `returnValue` and does not navigate.
- Removing `open` from a modal dialog takes it out of the top layer.

## Invokers

- `popovertarget` / `popovertargetaction` (`toggle`, `show`, `hide`) on
  `<button>` and `<input type=button|submit|reset|image>`, with
  `popoverTargetElement` / `popoverTargetAction`. A submit button with a form
  owner submits instead; a click inside a popover nested in its own invoker
  does nothing.
- `command` / `commandfor` on `<button>`, with `command` / `commandForElement`:
  a cancelable `CommandEvent` at the target, then the built-ins
  (`toggle-popover`, `show-popover`, `hide-popover`, `show-modal`, `close`,
  `request-close`; `close`/`request-close` pass the button's `value`).
  Custom commands (`--name`) only fire the event. An unknown command, or a
  built-in the target cannot run (popover commands on a non-popover, dialog
  commands on a non-dialog), fires nothing. A button with `commandfor` and no
  valid `type` is a plain button (`type` reads `button`) and does not submit.
- The element-reflecting setters store the element and set the content
  attribute to `""`; an empty attribute then means the stored element.

## Top layer and inertness

- The top layer (modal dialogs and showing popovers, in the order they
  entered it) is kept by lp-dom at mutation time. Hit testing
  (`elementFromPoint`, the pointer target of CDP input) puts top-layer boxes
  above everything else, by top-layer order, whatever their ancestors'
  z-index or stacking contexts.
- The topmost modal dialog blocks its document: everything outside it is
  inert (not hit-testable, not focusable by `focus()`, by a click or by Tab,
  and left out of the CDP accessibility tree). A point outside the dialog
  hits the dialog (its `::backdrop`).
- `:modal` and `:popover-open` match; the UA style of popovers
  (`position: fixed`, centered) applies.

## Not implemented

- Painting: screenshots (lp-render/Blitz) do not paint the top layer above
  everything, and `::backdrop` is not painted or laid out as a box.
- Layout: a top-layer element inside a transformed or `contain`ed ancestor
  uses that ancestor as its containing block, as a normal fixed box would.
- Close watcher abuse prevention and user-activation rules: Escape's
  `cancel` is always cancelable.
- `source` is not retargeted across shadow trees.
- Popover `hint` details beyond stack separation (`interestfor`, hover
  triggers), anchor positioning.
- `<details>` toggle for cloned elements (`cloneNode` copies `open` without
  a toggle).
