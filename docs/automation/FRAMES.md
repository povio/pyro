# Child-frame CDP integration, phase 1

The page's existing CDP session owns its native child frames. `Page.getFrameTree`
returns the nested browser snapshots; children do not create additional Target
sessions. The browser dependency is `17cc11a84db2d5c147f731b75cdad4424536ad53`,
including the WebAPI iframe hooks at `a05fb6fcb0742efc848b982fd6735c4880cfe494`.
See [browser frame design and limits](../browser/FRAMES-DESIGN.md).

## Supported protocol paths

- Frame attachment precedes child context announcements. Child navigation keeps
  the frame ID, destroys only that frame's old contexts, and announces its new
  main/isolated worlds after `Page.frameNavigated`. Child removal destroys its
  contexts before detach. Main and sibling contexts survive child navigation.
- `Runtime.evaluate`, `callFunctionOn`, console/exception routing, and remote
  handles use the owning frame's realm. `Page.createIsolatedWorld` accepts a live
  frame in the current target. Init scripts run in child worlds too;
  `runImmediately` applies to all current frames.
- `Page.navigate({frameId})` navigates that child without modifying top-level
  session history. Pending navigation replies are scoped to their frame; detach
  and supersession finish affected pending requests. `Page.reload` and the
  history commands remain top-level operations.
- Lifecycle events and late replay use each frame's loader and reached state.
  The unscoped `domContentEventFired`/`loadEventFired` events remain top-level.
  Security origins come from native document identity: inherited `about:srcdoc`
  retains the parent origin; opaque sandbox documents report `://` in the frame
  description while their runtime origin remains native `null`.
- `DOM.describeNode` identifies iframe owners and includes their actual
  `contentDocument` when depth permits. `DOM.getFrameOwner` returns a native
  backend node ID. `DOM.resolveNode` defaults to the node's owning main realm;
  an explicit realm must belong to that document. Backend IDs are weak,
  generation checked, and bounded across the target's live documents; replaced
  documents cannot be resolved through surviving remote wrappers.
- Network and Fetch route child requests by context plus native frame identity.
  Child document requests use their loader IDs; fetch/XHR retain native request
  IDs. Navigating/removing one child releases its captures without clearing
  parent/sibling response bodies. The existing process/page byte and record
  budgets still apply across all frames.

## Explicit boundary

Cross-frame geometry and focus routing are not implemented by the browser.
`DOM.getBoxModel`, `getContentQuads`, `focus`, and `scrollIntoViewIfNeeded` reject
nodes inside child documents. Focusing a native iframe owner also rejects.
Mouse dispatch rejects a hit on an iframe owner instead of treating the point
as a main-document target. `Input.dispatchKeyEvent` and `insertText` reject while
any child frame is live: documents can independently retain focused nodes, and
there is no native focused-frame chain. `Window.focus` is currently a no-op, so
root `activeElement` cannot establish a safe narrower policy. Accurate input
requires a subsequent native focus-transition hook and browser focus chain.

Read-only Playwright `frameLocator` lookups/evaluation and frame navigation are
covered. Child locator click/fill are not supported by this phase. Playwright
converts `DOM.getBoxModel` errors into a `null` bounding box; the direct CDP test
also checks the explicit protocol error. There is no fake child geometry.

Stable WindowProxy behavior across replacement, cross-frame messages, full
sandbox/permissions policy, cross-frame coordinates, joint history, and frame
file uploads remain outside this increment. This is not a TOS iframe or DOCX
compatibility claim; no TOS fixture or assertion was changed for this work.

## Regression coverage

`crates/lp-cdp/tests/engine_cdp/frames.rs` exercises real browser documents:
opaque sandbox srcdoc, native owner/DOM identity, stale handles, sibling-safe
navigation/removal, late lifecycle replay, HTTP document/fetch response routing,
per-frame body cleanup, Fetch fulfillment and detach cancellation, concurrent
child navigation, inherited/opaque origins, isolated-world init scripts, and
explicit input/geometry rejection. A script that removes another frame during
`runImmediately` is also covered: removed snapshots are skipped and future
script installation still succeeds. The initial four feature contracts all
failed on the browser-only dependency before the CDP implementation.

`tests/e2e/about-blank.mjs` adds four Playwright 1.59.1 flows: parser-created frames and parent reload, sandboxed srcdoc
with `contentFrame`/`frameElement`, child HTTP navigation/fetch/route fulfillment,
and nested `frameLocator` traversal/removal with sibling and handle isolation.
Use the existing pinned `bench/package.json` dependencies and pass the exact
browser binary as the script's argument. Validation counts and immutable build
provenance belong in the implementation PR and retained local logs, separately
from W1's browser-only validation and any future unchanged TOS run.
