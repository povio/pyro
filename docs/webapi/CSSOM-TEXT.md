# Rendered text and CSSOM queries

What `innerText`, `CSS.*`, `getComputedStyle(el, pseudoElt)`, `dataset` and the document collections do, and where they stop.

## innerText

`HTMLElement.innerText` (`crates/pyro-dom/src/inner_text.rs`) takes each child's text display from its tag (the upstream table) unless its computed `display` differs from that tag's HTML default. The difference covers author CSS and Stylo's blockification of flex and grid items, floats and absolutely positioned boxes. In that case the computed box decides (`ComputedStyle::box_kind`, derived with the rest of the computed style):

- block-level boxes (`block`, `flow-root`, `flex`, `grid`, `list-item`, `table`, `table-caption`) are surrounded by a required line break;
- `table-cell` boxes are tab-separated and `table-row` boxes newline-separated, CSS tables included;
- `display: contents` and table row groups add nothing of their own; their children still count;
- inline-level boxes add nothing (atomic inlines keep the spaces around them).

`<p>` always takes two required line breaks. Replaced elements never contribute their contents, and a block-level one is a line break. `<template>`, `<noscript>` and columns stay unrendered whatever their display. A closed `<details>` renders only its first `<summary>`. An `<option>` or `<optgroup>` outside a `<select>` is a block. Inside one, the options' text is kept as before.

Not done: `display: block` on `<script>` / `<style>` (the layout engine keeps no style for them), `visibility: collapse` on table rows, and the language-sensitive `text-transform` mappings (es-zet, Turkish i).

## CSS namespace

`window.CSS` is made on its first read and then replaced by a writable, configurable, non-enumerable data property. Realms that never read it pay nothing.

- `CSS.supports(property, value)` parses `value` with Stylo as that property's value in an author style rule. Property names match ASCII case-insensitively and custom properties match as written. `!important`, trailing tokens and surrounding whitespace in the name all fail. Properties that Stylo's servo build does not parse fail too (for example `-webkit-line-clamp`, `text-wrap` and `content-visibility`), except the three that pyro-dom computes itself (`overflow-anchor`, `text-anchor`, `line-clamp`).
- `CSS.supports(conditionText)` parses a `<supports-condition>` and falls back to a single declaration in parentheses. `not` / `and` / `or`, declarations and `selector()` are evaluated as `@supports` would evaluate them. `font-tech()` and `font-format()` are false. The Stylo features Blitz enables are switched on at the first call, so answers do not depend on whether a document has been laid out.
- `CSS.registerProperty(definition)` validates with Stylo: the name, the syntax, and an initial value that is present, parses per the syntax and is computationally independent unless the syntax is `*`. A failure throws `SyntaxError`, and registering a name twice throws `InvalidModificationError`. Registrations are kept per document (`StyleEngine::register_property`). `getComputedStyle` returns the initial value, as written, for a registered property that no declaration sets. Not applied yet: `inherits: false`, typed computed values (`1em` stays `1em`), and `@property` rules. That would need Stylo's script registry wired into the layout engine's Stylist.

## getComputedStyle(el, pseudoElt)

The argument follows CSSOM:

- Text without a leading colon is ignored, and the result is the element's own style.
- `::before`, `::after`, `::first-letter` (each also with a single colon), `::selection`, `::marker`, `::placeholder`, `::backdrop` and `::file-selector-button` read that pseudo-element's style. Blitz returns it through `BaseDocument::pseudo_element_style`:
  - the stored eager style (resolved on demand under `display: none`);
  - otherwise Stylo's lazy cascade;
  - otherwise a style with no rules of its own that inherits from the element.
- Anything else that starts with a colon, or has trailing tokens, gives an empty style (`length` 0, every value "").

A `::before` or `::after` that no rule generates reads `content: none`. Values are Stylo's computed values. Lengths are not resolved against a pseudo box (`width: 50%` stays `50%`), and `attr()` in `content` is substituted.

Accessible names computed by the driver (`getByRole` / `getByLabel` name-from-content) include generated `::before` / `::after` strings, as Playwright's `getPseudoContent` does. Only a stored generated style is consulted, so an element without one costs a lookup.

## dataset, document.images, document.scripts

`DOMStringMap` converts names character by character, so non-ASCII names work in both directions. Attribute names with ASCII upper alphas are skipped, and `data-foo--bar` reads as `foo-Bar`. `document.images` and `document.scripts` collect HTML-namespace elements only. MathML elements have no `MathMLElement` interface or `dataset` yet.
