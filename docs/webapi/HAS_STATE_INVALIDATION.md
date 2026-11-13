# `:has()` and element/form state

Computed styles come from the page's persistent Blitz document, styled by
Stylo. A `:has()` anchor sits above or before the element whose state
changed, where Stylo's snapshot invalidation (descendants and later siblings)
does not look, so the embedder has to run Stylo's relative selector
invalidation itself, as Gecko does from its restyle manager. Blitz does this
for every element state change and attribute change
(`vendor/blitz-dom/src/has_invalidation.rs`).

## What restyles

A change restyles the anchors whose `:has()` argument depends on it, that a
`:has()` search reached, and whose own compound (`.card` in
`.card:has(input:invalid)`) matches; each restyles only as far as its rule
needs: the anchor itself, plus the descendants (`.a:has(x) .b`) or siblings
(`.a:has(x) ~ .b`) the rule names. Stylo's
`RelativeSelectorInvalidator` decides this from the relative selector
invalidation map, the snapshot of the changed element and the selector flags
matching left.

States covered, from script, CDP input or Playwright:

| State | Changed by |
| --- | --- |
| `:checked` | `input.checked`, clicks, radio groups, `<option>` selectedness (`select.value`, `selectedIndex`, `option.selected`) |
| `:focus`, `:focus-within`, `:focus-visible` | `focus()`/`blur()`, clicks, Tab |
| `:hover`, `:active` | mouse moves and presses |
| `:disabled`, `:enabled`, `:read-only`, `:read-write`, `:required`, `:optional` | attributes and their IDL reflections |
| `:valid`, `:invalid`, `:in-range`, `:out-of-range`, `:placeholder-shown`, `:indeterminate`, `:default` | value, checkedness and attribute changes (forms and fieldsets follow their controls) |
| `:user-valid`, `:user-invalid` | see below |
| `:open`, `:modal`, `:popover-open` | `details.open`, `dialog.show()`/`showModal()`/`close()`, `showPopover()`/`hidePopover()` |
| `:state()` | `ElementInternals.states` |

The form states pyro-dom computes (validity, placeholder shown, ranges, user
validity, open/modal/popover) reach Blitz as tokens of the render tree's
`urn:x-pyro:state` attribute; Blitz turns them into the element's
`ElementState` bits when the attribute is written, so Stylo's own state
invalidation and style sharing see them like `:checked` or `:hover`.

## Cost

- No `:has()` in the page: matching never flags an element as searched, so a
  state or attribute change costs one selector-flags test on the element.
- `:has()` present but not depending on the change (`body:has(dialog[open])`
  and a checkbox toggles): one flag test against the union of the states the
  relative selector maps depend on, or one hash lookup per changed attribute
  name, id or class. Nothing restyles beyond what the element itself needs.
- Before this, any attribute change of an element a `:has()` search had
  visited restyled every anchor that could see it with its whole subtree: a
  page with `body:has(...)` restyled the document on every keystroke.

`crates/pyro-layout/tests/has_state.rs` checks both: each state change restyles
its anchor (incrementally and against a fresh document), and a page whose
`body:has(.modal-open)` depends on neither `:checked` nor class `x` styles
exactly as many elements for those changes as a page without `:has()`.

## User validity

`:user-valid` / `:user-invalid` (HTML "user validity") match a candidate for
constraint validation whose user validity is set and that is valid / invalid.
It is set when

- the user commits a change (`change` from a click on a checkbox or radio, a
  keyboard-stepped or picked value, a file chosen through CDP, a time value
  filled through CDP),
- the user leaves a text control after editing it (even when the value ends
  up as it was), and
- a submission with validation (`requestSubmit()`, a submit button, implicit
  submission; not `form.submit()` or a `novalidate` form) validates the form:
  all its `input`, `select` and `textarea` elements;

and cleared by the form's reset. Script value changes do not set it.

## Still missing

- DOM insertions and removals keep the older, coarser path: every anchor
  that can see the position restyles its subtree.
- A custom state (`:state()`) that turns off inside a non-rightmost compound
  of a `:has()` argument (`:has(:state(x) .y)`) is not noticed: Servo's
  element snapshots carry no custom states. The common form
  (`:has(:state(x))`, `:has(.y:state(x))`) is.
- `:indeterminate` of radio groups without a checked radio; `:autofill`,
  `:fullscreen`, `:visited`, `:target` inside `:has()` (Blitz never sets
  them).
- The relative selector invalidation recurses per DOM level for descendant
  invalidations without a stack limit, as Stylo's own snapshot invalidation in
  Blitz already does.
