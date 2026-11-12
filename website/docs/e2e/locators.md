# Locators

A locator describes which element a step acts on or checks. In the native driver a locator is plain data: the builders only produce a JSON description, and the browser resolves it against the live DOM each time the step is tried. Build locators once and reuse them freely.

```ts
import { button, dialog, row, label, css, text } from '@povio/lightning-panda';

const confirm = dialog().hasText('will be closed').button('Close Position', { exact: true });
const editExport = row('Export').button('Edit');
```

## Top-level builders

All are exported from `@povio/lightning-panda`.

| Builder | Matches |
|---|---|
| `role(role, name?, { exact? })` | Elements with this ARIA role (explicit or implicit) and, if given, this accessible name |
| `button(name?, opts?)` | `role('button', ...)` |
| `link(name?, opts?)` | `role('link', ...)` |
| `heading(name?, opts?)` | `role('heading', ...)` |
| `dialog(name?, opts?)` | `role('dialog', ...)` |
| `textbox(name?, opts?)` | `role('textbox', ...)` |
| `checkbox(name?, opts?)` | `role('checkbox', ...)` |
| `combobox(name?, opts?)` | `role('combobox', ...)` |
| `option(name?, opts?)` | `role('option', ...)` |
| `row(name?, opts?)` | `role('row', ...)` |
| `cell(name?, opts?)` | `role('cell', ...)` |
| `tab(name?, opts?)` | `role('tab', ...)` |
| `menuitem(name?, opts?)` | `role('menuitem', ...)` |
| `label(text, { exact? })` | An element whose `aria-label` matches, or an `input`, `textarea`, `select` or `button` whose `<label>` text matches |
| `placeholder(text, { exact? })` | An `input` or `textarea` whose `placeholder` matches |
| `text(text, { exact? })` | The smallest elements whose text matches (see [Text matching](#text-matching)) |
| `testId(id, attribute?)` | Elements whose `data-testid` equals `id`; pass `attribute` to use another one, for example `testId('row-1', 'data-test')` |
| `css(selector)` | Elements matching a CSS selector |
| `inFrame(frameLocator)` | A scope inside an iframe's document (see [Iframes](#iframes)) |

`name` and `text` arguments take a string or a `RegExp`.

## Chaining: locators inside a locator

Every locator has the same builders as methods. Each one searches inside every match of the locator it is called on:

```ts
dialog().button('OK')                       // a button inside the dialog
row('EXP-1001').button('Edit')              // the Edit button in that row
css('.card').text('Total')                  // text inside .card
css('form#signup').label('Email')           // the Email field of that form
```

| Method | Same as |
|---|---|
| `loc.locator(css)` | `css(...)` inside `loc` |
| `loc.role(role, name?, opts?)` | `role(...)` inside `loc` |
| `loc.button(...)`, `loc.link(...)`, `loc.textbox(...)` | the role shortcuts inside `loc` |
| `loc.text(text, opts?)` | `text(...)` inside `loc` |
| `loc.label(text, opts?)` | `label(...)` inside `loc` |
| `loc.placeholder(text, opts?)` | `placeholder(...)` inside `loc` |
| `loc.testId(id)` | `testId(id)` inside `loc` (default attribute) |

Only `button`, `link` and `textbox` have method shortcuts. For other roles inside a scope use `loc.role('row', 'EXP-1')`.

The inner locator matches strictly inside the outer element, except `text()`: as in Playwright's `getByText`, the scope element itself can match. `css('td').text('Domestic', { exact: true })` finds the `<td>` whose text is "Domestic".

## Filters

Filters narrow a locator's matches. Each returns a new locator. Several filters on one locator must all hold.

| Filter | Keeps matches that |
|---|---|
| `.visible(visible = true)` | Have a layout box with non-zero width and height (or, with `false`, do not) |
| `.hasText(text)` | Contain this text anywhere inside (string: case-insensitive substring; or a `RegExp`) |
| `.hasNotText(text)` | Do not contain it |
| `.has(locator)` | Contain at least one element the inner locator matches |
| `.hasNot(locator)` | Contain none |

```ts
dialog().hasText('will be closed')                // the dialog with that text
row().has(cell('Domestic', { exact: true }))      // rows that have that cell
text('Ghost text').visible()                      // ignore hidden copies
css('li').hasNot(button('Delete'))                // items without a Delete button
```

`hasText` always matches a case-insensitive substring (or a `RegExp`); it has no `exact` option.

## Picking one match

| Method | Picks |
|---|---|
| `.first()` | The first match in document order |
| `.last()` | The last match |
| `.nth(i)` | The match at index `i`; a negative index counts from the end (`-1` is the last) |

Filters run first, then the pick. The builders enforce that order: calling a filter after `.first()`, `.last()` or `.nth()` throws `filter before first() / last() / nth()` when you build the locator. A pick that is out of range matches nothing, so the step waits.

Use a pick only when any of several matches will do. To name one specific element, scope or filter the locator instead.

## Text matching

These rules apply to role names, `text`, `label` and `placeholder`:

- **A string** matches as a **case-insensitive substring** after whitespace normalization (runs of whitespace become one space, ends trimmed). `text('domestic')` matches "Domestic flights".
- **`{ exact: true }`** makes it a whole, case-sensitive match after normalization. `text('domestic', { exact: true })` does not match "Domestic".
- **A `RegExp`** is tested against the normalized text and ignores `exact`. Flags `i`, `m` and `s` are kept; `g`, `y` and `u` are dropped by the builder. The browser runs it with JavaScript semantics: look-ahead, look-behind, backreferences and named groups work.

`text()` finds the **smallest** elements whose text matches: an element matches when its text matches and none of its child elements' text does. `<script>`, `<style>`, `<noscript>`, `<template>`, `<head>` and `<title>` are skipped. With an exact string, an element also matches when one run of its own text nodes equals the string, so `text('TS-1', { exact: true })` finds `<span><span>|</span>TS-1</span>`.

## Hidden elements

| Locator kind | Hidden elements |
|---|---|
| Role locators (`role`, `button`, `dialog`, ...) | Not matched. An element hidden from the accessibility tree (`display: none`, `visibility: hidden`, `aria-hidden="true"` on it or an ancestor) has no role. |
| `text`, `css`, `testId`, `label`, `placeholder` | Matched. Add `.visible()` when the page keeps hidden copies, such as a desktop and a mobile menu. |

`button('Save')` therefore never matches a hidden button, while `text('Save')` matches hidden and visible copies alike and fails strict mode on an action if both exist. `check(loc).visible()` on a locator that matches only hidden elements waits; `check(loc).hidden()` holds.

## Strict mode

An action, and a check that reads one element (`text`, `containsText`, `value`, `checked`, `css`, `focused`, `name`, `enabled`, `visible`, `hidden`), needs exactly one match. When the locator matches several elements, the step fails at once, without waiting for its timeout, and lists up to five of them:

```text
RunError: step 0 (Click {"css":"li button"}): strict mode violation: {"css":"li button"} matched 3 elements: <button> "x", <button> "x", <button> "x"
```

Fix the locator: scope it (`row('Export').button('Edit')`), filter it (`.hasText(...)`, `.visible()`), or make the name exact. Reads (`loc.read(...)`) follow strict mode too. `check(loc).count(n)` and `read(as, 'count')` accept any number of matches.

## Iframes

Locators search the main document. To search an iframe's document, name the `<iframe>` element with a locator in the parent document and scope through it:

```ts
css('#editor').frame().button('Bold')          // Playwright's frameLocator('#editor').getByRole(...)
inFrame(css('#editor')).text('Inside')         // the same scope, as a function
text('Inside').inFrame(css('#editor'))         // any locator, searched in that frame
css('#outer').frame().frame(css('#inner')).button('OK')  // a nested iframe
```

`loc.frame()` returns a scope with `locator`, `role`, `button`, `link`, `textbox`, `text`, `label`, `placeholder`, `testId` and `frame` (for a nested iframe). While the frame has no document yet the step waits with `the frame has no document yet`.

## Priority order

Prefer locators the way a user finds things:

1. **Role and accessible name**: `button('Save')`, `link('Docs')`, `dialog().button('OK')`. They follow what the user sees and survive markup changes.
2. **Label, placeholder, text**: `label('Email')`, `placeholder('Search')`, `text('Saved')`.
3. **Test id**: `testId('row-42')` for elements that have no stable name.
4. **CSS**: `css('#out')` as a last resort.

Semantic locators cost about the same as CSS ids: they run in Rust against the browser's DOM, not in injected JavaScript.

## Locators without a builder

The wire format has a few locator fields that the native builders do not produce. The `Locator` class is exported, so you can pass a spec directly. Its shape is `LocatorSpec` in `src/steps.ts`; the browser rejects unknown fields.

| Field | Meaning |
|---|---|
| `xpath` | An XPath expression, relative to `within` or the document: `new Locator({ xpath: '//h2' })` |
| `or` | Joined with these locators' matches in document order: `new Locator({ css: 'h2', or: [{ css: 'h3' }] })` |
| `and` | Kept only where each of these matches too |
| `checked`, `pressed` (`true`, `false`, `'mixed'`), `disabled`, `expanded`, `selected`, `level` | Role states, as Playwright's `getByRole` options; they need `role`: `new Locator({ role: 'heading', level: 3 })` |
| `includeHidden` | Lets a role locator match hidden elements. The accessible name of a `display: none` element can be empty, so match such elements by role or test id rather than by name |
| `of` | Another locator's matches as the base set, so a filter or a pick can follow a pick: `new Locator({ of: { role: 'button', nth: 0 }, filter: { hasText: 'Bold' } })` |

A `Locator` built this way has every method of the builders' locators (`.click()`, `.read()`, chaining). See [SDK: locators and steps](/reference/steps) and [LP.run wire format](/reference/lp-run).
