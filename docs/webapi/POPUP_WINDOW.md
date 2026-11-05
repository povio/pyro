# `window.open` and popups

`window.open(url, target, features)` opens a new page in the opener's browser
context. CDP clients see a target with `openerId`, paused under auto-attach
until `Runtime.runIfWaitingForDebugger`, so Playwright's `popup` event fires.

| Call | Returns |
| --- | --- |
| `_self`, `_top`, `_parent` | navigates this page, returns this window |
| `noopener` or `noreferrer` feature | `null` |
| a name this document opened earlier, still open | that popup's object (navigated to `url` unless `url` is empty) |
| anything else | the popup object |

A URL that does not parse throws `SyntaxError` in every case.

## What the popup gets

A popup opened without `noopener` (`lp_browser::Browser::adopt_popup`, called by
the CDP layer when the target exists):

- **`window.opener`**: the opener's top-level window — the real global when
  same-origin, otherwise a cross-origin stand-in (`closed`, `close()`,
  `focus()`, `blur()`, `window`/`self`/`top`/`parent`, `length`; anything else
  throws `SecurityError`). Assigning it (e.g. `null`) shadows it.
- **session storage**: a new namespace holding a copy of the opener's session
  storage area, taken when `window.open` ran (HTML copies it into the new
  top-level traversable). Changes diverge from then on and the opener gets no
  `storage` events from it. Local storage is the browser context's, shared.
  `noopener` popups start empty.
- **about:blank documents** take the opener's origin and base URL, as a child
  frame's do from its parent.
- **`window.close()`** asks the embedder to close the page
  (`PageEvent::CloseRequested`; the CDP layer closes the target). Only popups
  are script-closable; other pages ignore the call, as Chrome does.

## The popup object

Every page has its own JS context and DOM; the returned object stands in for
the popup's WindowProxy. One object per opener realm and popup, so the named
reuse above returns the same object.

Always available:

- `closed`: true after `close()`, when the page is gone, or when the popup was
  refused (target budget, a sandboxed frame without `allow-popups`, an embedder
  with no CDP layer).
- `close()`, and `focus()`/`blur()`, which do nothing.
- `location`: assigning it, `location.href = url`, `location.assign(url)` and
  `location.replace(url)` navigate the popup. URLs resolve against the opener;
  `javascript:` URLs are ignored. `href` reads the popup document's URL
  (`about:blank` before the page exists). Reading `href` or `toString()`
  throws `SecurityError` once the popup's origin differs from the opener's.
- `window`, `self`, `frames`, `top`, `parent` (the object itself), `opener`
  (the opener's global), `name`.
- `postMessage(message, targetOrigin, transfer)`: structured-clones the
  message in the opener (DataCloneError propagates) and fires a trusted
  `MessageEvent` in the popup's document as a task, `origin` the opener's,
  `source` null. Dropped when the popup page does not exist yet or
  `targetOrigin` does not match its origin.
- `fetch(...)`: the popup's own `fetch` once its page exists; before that it
  runs in the opener's realm (the initial about:blank document shares the
  opener's origin and base URL, so only the Response's realm differs).
- `localStorage`, `sessionStorage`: the popup's areas. Before the page
  exists, `sessionStorage` is the copied area the popup will adopt (writes
  through it are visible in the popup; `storage` events for them go to the
  opener's other same-origin documents rather than the popup's).

Once the popup page exists and is same-origin, every other Window member reads
through to the popup's real global (`popup.document`, `addEventListener`,
...). Cross-origin reads throw `SecurityError`; before the page exists they are
undefined.

## Gaps

- The session storage copy covers the opener document's origin only (the
  namespace exposes no other origin's area), and `StorageNamespace` has no
  snapshot: the copy is made through the opener's `Storage` object.
- Named reuse is per opener document: a name is forgotten when the opener
  navigates, and another page's popup of that name is not found.
- A popup's `window.opener` is the opener page's current top-level window; it
  survives the opener's navigation, where browsers would too, but does not
  track a `noopener` navigation of the opener.
- `location.replace` adds a history entry, as `assign` does.
- The target reports `canAccessOpener: true` even for `rel=opener` links,
  whose popups do not get `window.opener` (no ticket).
