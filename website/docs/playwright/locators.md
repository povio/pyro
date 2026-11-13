# Locators and selectors

Locators work as in Playwright: they are lazy descriptions, resolved each time they are used, strict about matching one element for actions, and auto-waiting. The difference is where they resolve. Each awaited locator call is sent to the browser as one command, and the browser locates the element, runs the actionability checks, performs the action and waits for the page to settle before it replies.

```ts
const row = page.getByRole('row').filter({ hasText: 'INV-1042' });
await row.getByRole('button', { name: 'Edit' }).click();
await expect(row.getByRole('cell').nth(2)).toHaveText('Paid');
```

## getBy* locators

| Method | Notes |
| --- | --- |
| `getByRole(role, options)` | Every option: `name` (string or RegExp), `exact`, `checked`, `pressed`, `disabled`, `expanded`, `selected`, `level`, `includeHidden`. Hidden elements are skipped unless `includeHidden`. An unknown option throws. |
| `getByText(text, { exact })` | Case-insensitive substring by default; `exact: true` matches the whole, case-sensitive text. RegExp supported. |
| `getByLabel(text, { exact })` | Form controls by their `<label>` or `aria-label`. RegExp supported. |
| `getByPlaceholder(text, { exact })` | RegExp supported. |
| `getByAltText(text, { exact })` | A string only. A RegExp throws "not supported yet". |
| `getByTitle(text, { exact })` | A string only. A RegExp throws "not supported yet". |
| `getByTestId(id)` | A string only. Matches the `testIdAttribute` option (default `data-testid`). A RegExp throws "not supported yet". |

The role names and the name computation are the browser's own, the same code `ariaSnapshot()` and `toMatchAriaSnapshot` use.

`selectors.setTestIdAttribute()` does nothing. Set the attribute with the `testIdAttribute` option in `use` instead:

```ts
// playwright.config.ts
export default defineConfig({ use: { testIdAttribute: 'data-qa' } });
```

## Narrowing and combining

| Method | Notes |
| --- | --- |
| `locator(selectorOrLocator, options)` | Searches inside the current match. Takes the same options as `filter`. |
| `filter({ has, hasNot, hasText, hasNotText, visible })` | All five options. `has` and `hasNot` take locators. |
| `nth(i)`, `first()`, `last()` | Negative `nth` counts from the end. Filters and `nth` after `first()` / `last()` / `nth()` apply to the picked element, as in Playwright. |
| `and(locator)` | Elements that match both. Both locators must be in the same frame. |
| `or(locator)` | Elements that match either, in document order. Same-frame rule. |
| `all()` | One locator per current match. |
| `count()` | Does not wait. |

Strictness is Playwright's: an action or a single-element read on a locator that matches several elements fails with a strict mode violation. The message lists the matched elements:

```text
locator.click: strict mode violation: {"name":"Buy","role":"button"} matched 2 elements: <button> "Buy", <button> "Buy"
```

## Frames

```ts
// Locators inside an <iframe>
const card = page.frameLocator('#payment').getByLabel('Card number');
await card.fill('4242 4242 4242 4242');

// Same, from a locator for the <iframe>
await page.locator('#payment').contentFrame().getByRole('button', { name: 'Pay' }).click();

// The Frame object
const frame = page.frame({ name: 'payment' });
await frame?.getByLabel('Card number').fill('4242 4242 4242 4242');
```

- `FrameLocator` has `locator`, every `getBy*`, `frameLocator` (nested frames) and `owner()`. Its deprecated `first()`, `last()` and `nth()` are not implemented and are type errors; narrow the `<iframe>` locator before `contentFrame()` instead.
- `page.frames()`, `page.frame(nameOrOptions)`, `frame.childFrames()` and `frame.parentFrame()` list child frames. Once listed, a frame's `<iframe>` element carries a `data-pyro-frame` attribute, which the frame's locators use to find it. An app that serializes the DOM may see that attribute.
- `Frame.evaluate`, `content`, `title`, `waitForFunction` and `waitForLoadState` on a child frame run through the `<iframe>` element's window. A cross-origin frame fails with "the frame has no window (cross-origin or detached)".

## Selector strings

`page.locator(selector)` takes Playwright's selector syntax:

| Selector | Example |
| --- | --- |
| CSS | `button.primary`, `css=button.primary` |
| Text | `text=Sign in`, `"Sign in"` (quoted text) |
| Role | `role=button[name="Buy"]`, with `[checked]`, `[pressed]`, `[disabled]`, `[expanded]`, `[selected]`, `[level=2]`, `[include-hidden]` |
| Id | `id=main` |
| Test id | `data-testid=save`, `data-test-id=save`, `data-test=save`, and the configured `testIdAttribute` |
| XPath | `xpath=//li`, or any selector starting with `//`, `..` or `(//` |
| Chains | `#list >> li >> nth=1` |
| `nth=`, `visible=` | `li >> nth=-1`, `li >> visible=true` |
| `aria-ref=` | `aria-ref=e5`: the element of a ref from the last AI-mode ARIA snapshot. See [ARIA snapshots](/playwright/advanced#ai-mode-and-aria-ref). |
| Playwright's internal engines | `internal:role`, `internal:text`, `internal:label`, `internal:attr`, `internal:testid`, `internal:has`, `internal:has-not`, `internal:has-text`, `internal:has-not-text`, `internal:and`, `internal:or`, `internal:chain`, `internal:control=enter-frame`. These are what Playwright's locators serialize to, so selectors copied from Playwright's error messages or codegen work. |

### CSS pseudo-classes from Playwright

CSS selectors may use Playwright's extensions:

| Pseudo-class | Meaning |
| --- | --- |
| `:has-text("text")` | Contains the text, case-insensitive. |
| `:text("text")` | Matches the text as `getByText` does. |
| `:text-is("text")` | Exact text. |
| `:text-matches("regex", "flags")` | Text matches the RegExp. |
| `:visible` | Visible elements only. |
| `:nth-match(selector, n)` | The n-th match (1-based). Cannot be combined with other selectors in the same compound. |
| `:light(selector)` | Accepted; same as the plain selector. |

Standard CSS, including `:has()`, `:is()`, `:not()` and attribute selectors, is evaluated by the browser's selector engine.

### Not supported

| Selector | What happens |
| --- | --- |
| Layout pseudo-classes `:left-of()`, `:right-of()`, `:above()`, `:below()`, `:near()` | Throws: "the layout pseudo-class :near() (deprecated in Playwright) is not supported". Playwright deprecated them. |
| A `+` or `~` combinator right after a Playwright pseudo-class, for example `li:has-text("A") + li` | Throws. Plain CSS `li + li` works. |
| Custom engines (`selectors.register`) | Throws. |
| `getByTestId`, `getByAltText`, `getByTitle` with a RegExp | Throws "not supported yet". |
| Unknown `internal:` engines | Throws. |

## Shadow DOM

Measured on version 0.0.33 and again on the current code: CSS selectors, `getByRole`, `getByLabel` and `getByPlaceholder` do not find elements inside open shadow roots, while `getByText` does. Playwright's locators pierce open shadow roots. If your app renders into shadow DOM (web components), locate those elements by text, or test them through their host element.

## Actions

| Method | Supported options | Notes |
| --- | --- | --- |
| `click()` | `button`, `clickCount` (1 or 2), `modifiers`, `position`, `force`, `trial`, `timeout` | `clickCount` above 2 throws. `delay` is ignored. |
| `dblclick()` | same as `click` | |
| `tap()` | same as `click` | A click; there is no touch event sequence. |
| `hover()` | `modifiers`, `position`, `force`, `trial`, `timeout` | |
| `fill(value)`, `clear()` | `timeout` | `force` is ignored. |
| `press(key)` | `timeout` | Keys and chords such as `Enter`, `Shift+Tab`, `Control+a`. `delay` is ignored. |
| `pressSequentially(text)` | `timeout` | Types the text. `delay` is ignored. |
| `check()`, `uncheck()`, `setChecked(v)` | `trial`, `timeout` | `force` is ignored. |
| `selectOption(values)` | `timeout` | Values, labels, `{ value, label, index }`, lists, or `null`. Returns the selected values. |
| `setInputFiles(files)` | `timeout` | Paths or `{ name, mimeType, buffer }` payloads. |
| `focus()`, `blur()` | `timeout` | Do not wait for the page to settle. |
| `dragTo(target)` | `timeout` | Position options are ignored. |
| `dispatchEvent(type, init)` | `timeout` | Picks the event class from the type (`MouseEvent`, `KeyboardEvent`, `FocusEvent`, ...). |
| `scrollIntoViewIfNeeded()`, `selectText()` | `timeout` | |
| `highlight()` | | Does nothing. |

`trial: true` runs the visibility and enabled checks only. Keys held with `page.keyboard.down('Shift')` apply to later clicks and presses.

## Reads

`textContent`, `innerText`, `innerHTML`, `inputValue`, `getAttribute`, `allTextContents`, `allInnerTexts`, `boundingBox`, `isVisible`, `isHidden`, `isEnabled`, `isDisabled`, `isChecked`, `isEditable`, `evaluate`, `evaluateAll`, `evaluateHandle`, `elementHandle`, `elementHandles`, `ariaSnapshot`, `screenshot`, `waitFor({ state })`.

`isVisible()` and `isHidden()` do not wait, as in Playwright. `waitFor` takes `attached`, `detached`, `visible` (default) and `hidden`.

`locator.describe()` from recent Playwright versions is not implemented.

## Next

- Assertions on what you located: [Assertions](/playwright/assertions).
- Shadow DOM and other locator failures: [Triage the first red run](/recipes/playwright-triage-first-failures#elements-inside-a-shadow-root).
