# Navigation API

`window.navigation` and its interfaces (`Navigation`, `NavigateEvent`,
`NavigationHistoryEntry`, `NavigationDestination`, `NavigationTransition`,
`NavigationCurrentEntryChangeEvent`) are made in JS
(crates/pyro-browser/src/navigation.js) on a realm's first use, over the
frame's session history in `PageNav` (crates/pyro-browser/src/nav.rs).

## What works

- `entries()`, `currentEntry` (the same object while the entry lives),
  `canGoBack`, `canGoForward`: the same-origin entries around the current
  one. Entries have `key` (kept when the entry is replaced), `id`, `url`,
  `index`, `sameDocument` and `getState()`.
- A `navigate` event at `navigation` before every navigation of the
  document: link and area activation (GET), `location` assignments and
  setters, `location.reload()`, `history.pushState`/`replaceState`, fragment
  navigations, same-document and cross-document traversals, and
  `navigation.navigate()`/`reload()`/`back()`/`forward()`/`traverseTo()`.
  `navigationType`, `destination`, `canIntercept` (same origin, and not a
  traversal to another document), `hashChange`, `signal` and `info` are set;
  `preventDefault()` cancels a cancelable one (`navigateerror` with an
  AbortError, the promises reject).
- `intercept({handler})`: the navigation commits as a same-document one (the
  URL changes as with pushState, nothing loads), `currententrychange` fires,
  the handlers run, then `navigatesuccess` (or `navigateerror` when one
  rejects). `navigation.transition` is set meanwhile.
- `navigate(url, {state, info, history})` returns `{committed, finished}`;
  `updateCurrentEntry({state})`. State is kept as JSON, like `history.state`.
- `currententrychange` comes before `popstate` on a same-document traversal
  or fragment navigation.

## Cost

Until a realm reads `navigation` (or one of the interface names), nothing is
made and each navigation path checks one empty list. Afterwards each
navigation calls into the realm's module twice.

## Gaps

- The events are made by script: `isTrusted` is false.
- `userInitiated` is always false; `formData` and `sourceElement` are null;
  form submissions (POST) fire no `navigate` event, and a GET form
  submission fires one as a link would.
- The state given to `navigate()` for a cross-document navigation is not kept
  by the new document, and `navigation.activation`, entry `dispose` events,
  `NavigateEvent.scroll()` scrolling and `focusReset` are not implemented.
- Child frames' `navigate` events fire only for navigations of that frame.
