# Custom elements

`customElements.define/get/getName/whenDefined/upgrade`, construction with
`new`, `document.createElement`, the parser and clones, upgrades of existing
elements, the lifecycle callbacks, form-associated custom elements
(`ElementInternals`) and `CustomStateSet` / `:state()`.

## Definitions

`define` reads, once, the prototype's `connectedCallback`,
`disconnectedCallback`, `connectedMoveCallback`, `adoptedCallback` and
`attributeChangedCallback`; `observedAttributes` (a `sequence<DOMString>`,
read only when there is an `attributeChangedCallback`); `disabledFeatures`
(`"internals"`, `"shadow"`); and `formAssociated`, which adds
`formAssociatedCallback`, `formResetCallback`, `formDisabledCallback` and
`formStateRestoreCallback`. Changing the prototype or `observedAttributes`
afterwards changes nothing, as in browsers.

## Construction and upgrades

- The main document parser (network and `document.open` parsers) constructs an
  element whose name is defined when it reaches its start tag: the constructor
  runs with the element's earlier siblings already in the tree and none of
  its attributes or children, then the parser appends the token's attributes
  (mutation records and `attributeChangedCallback`), inserts it
  (`connectedCallback`) and goes on. `document.open/write/writeln/close` throw
  `InvalidStateError` meanwhile. A constructor that throws, or returns another
  element, is reported and leaves a "failed" element that is never upgraded.
- `customElements.define` upgrades the document's elements of that name in
  tree order; an element with a defined name is upgraded when it becomes
  connected (insertion, `innerHTML`), and `cloneNode`/`importNode` upgrade the
  clones they make, connected or not.
- An upgrade calls `attributeChangedCallback(name, null, value, namespace)` for
  each observed attribute the element has, then `connectedCallback` when it is
  connected, then the form-associated callbacks.

## Reactions

The DOM queues them as it changes and runs them when the DOM call returns
(`[CEReactions]`), in order, by the window that created the element:

- `connectedCallback`, `disconnectedCallback`, `connectedMoveCallback` (see
  [moveBefore](MOVE_BEFORE.md));
- `adoptedCallback(oldDocument, newDocument)` when the element moves to
  another document (`adoptNode`, insertion into another document);
- `attributeChangedCallback(name, oldValue, newValue, namespace)` for every
  set, change (also to the same value) or removal of an observed attribute,
  through any API (`setAttribute*`, `removeAttribute*`, `toggleAttribute`,
  `Attr.value`, `setAttributeNode`, `className`, `dataset`, reflected
  properties).

## Form-associated custom elements

`static formAssociated = true` makes the element a listed, labelable,
submittable and resettable form control:

- `ElementInternals.setFormValue(value)`: a string or `File` is submitted under
  the element's `name`; a `FormData` contributes its own entries. `null`
  removes the value. Values reach `new FormData(form)` and form submission.
- `form` (the form owner: ancestor form or `form` attribute), `labels`,
  membership of `form.elements`, `label.control`.
- Disabled by its own `disabled` attribute or a disabled `fieldset` ancestor
  (not in its first `legend`): left out of submission and validation, matches
  `:disabled`.
- `setValidity(flags, message, anchor)`, `validity`, `validationMessage`,
  `willValidate` (false when disabled, `readonly` or in a `datalist`),
  `checkValidity`/`reportValidity` (fire `invalid`), `form.checkValidity()`,
  `:valid`/`:invalid`.
- `formAssociatedCallback(form)` when the form owner changes (insertion,
  removal, `form`/`id` attribute changes, forms inserted or removed),
  `formDisabledCallback(disabled)` when the disabled state changes, and
  `formResetCallback()` on `form.reset()`.

`attachInternals()` throws `NotSupportedError` for elements that are not
custom (or not yet upgraded), when the definition disables internals, and on a
second call. The form members throw `NotSupportedError` on internals of an
element that is not form-associated. `shadowRoot`, `states`, `role` and the
`aria*` string attributes work on any custom element's internals; the ARIA
values are stored on the internals (default semantics), not reflected to
attributes. `shadowRoot` returns only a root attached while the host was
custom or its upgrade constructor ran ("available to element internals"), not
one it had before its upgrade.

## Custom states

`internals.states` is a `CustomStateSet` (setlike: `add`, `delete`, `has`,
`clear`, `size`, `values`/`keys`/`entries`/`forEach`/`@@iterator`, iterators
live like a `Set`'s). `:state(name)` matches in `querySelector`, `matches`,
`closest` and stylesheets, and a change
restyles the element.

## Cost when unused

No interface script runs at realm creation: `ElementInternals` and
`CustomStateSet` are host classes made on first use. An attribute change on
an element whose definition observes no attributes tests one element flag;
insertions and removals test one flag until some element is custom, and one
counter until a form-associated custom element exists; the parser checks
whether any name is defined at all before it looks one up.

## Known gaps

- Customized built-in elements (`is`) and scoped registries are not supported.
- `formStateRestoreCallback` is never called (there is no session history or
  autofill restore), and `setFormValue`'s `state` argument is ignored.
- The XML parser does not construct custom elements; they upgrade at `define`
  or when connected.
- Reactions the parser's insertions queue run with dynamic markup insertion
  throwing (`document.write` from a `connectedCallback` during parsing throws
  instead of writing at the insertion point).
- The parser's adoption agency and foster parenting move elements without
  disconnected/connected reactions.
- `CustomStateSet.prototype` gets its iteration members when the realm makes
  its first set.
- `ElementInternals` has no element-reference ARIA attributes
  (`ariaLabelledByElements`, ...).
- A disabled form-associated custom element matches `:disabled` in queries;
  the renderer's styles do not see that state yet.
