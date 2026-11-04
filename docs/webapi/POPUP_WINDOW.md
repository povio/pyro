# `window.open` and popups

`window.open(url, target, features)` opens a new page in the opener's browser
context. CDP clients see a target with `openerId`, paused under auto-attach
until `Runtime.runIfWaitingForDebugger`, so Playwright's `popup` event fires.

| Call | Returns |
| --- | --- |
| `_self`, `_top`, `_parent` | navigates this page, returns this window |
| `noopener` or `noreferrer` feature | `null` |
| anything else | a minimal popup object |

A URL that does not parse throws `SyntaxError` in every case.

## The popup object

Every page has its own JS context and DOM, so there is no cross-page
WindowProxy. The returned object has only:

- `closed`: true after `close()`, when the page is gone, or when the popup was
  refused (target budget, a sandboxed frame without `allow-popups`, an embedder
  with no CDP layer).
- `close()`, and `focus()`/`blur()`, which do nothing.
- `location`: assigning it, `location.href = url`, `location.assign(url)` and
  `location.replace(url)` navigate the popup. URLs resolve against the opener;
  `javascript:` URLs are ignored. Before the popup commits a document, `href`
  reads `about:blank`. Reading `href` or `toString()` throws `SecurityError`
  once the popup's origin differs from the opener's.

## Gaps

Everything else is undefined rather than faked, so these fail where they are
used:

- `popup.document`, `popup.postMessage`, `popup.sessionStorage`, `popup.fetch`
  and every other Window member.
- `window.opener` is null in the popup, so `opener.postMessage` does not work.
  The target still reports `canAccessOpener: true`, as Chrome does.
- `window.close()` does not exist, so a popup cannot close itself.
- `location.replace` adds a history entry, as `assign` does.
- Named targets always open a new page; `window.open(url, name)` does not reuse
  the earlier window of that name.
