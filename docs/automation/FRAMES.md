# Child-frame CDP integration

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

## Native focus and keyboard routing

With browser dependency `40f9b4ea786f63088f9dd217b6cece49b9762135` and native
focus transitions `59a940ea855a7506a714c34e1646ec15dffa5fdf`, key/text commands
resolve a fresh owned `Browser::focused_frame` snapshot for their page. A missing
snapshot is an error, never a fallback to the parent editable. Runtime context
selection does not select the input frame. Parent inputs remain usable while
children are live, and native child, sibling, window and owner focus transitions
select the target document. `DOM.focus` accepts connected, focusable child nodes
and iframe owners without exposing child coordinates.

Native event dispatch revalidates focus after handlers. Same-document keydown
focus changes retarget editing; cross-document changes suppress that command's
old default action. For split CDP key sequences, a raw keydown which changes the
focused document suppresses one subsequent char. A second char, keyup or new
keydown clears that suppression; `insertText` is independent. State is per page,
not per inspector realm or session, and invalid commands do not consume it.

Removing a focused subtree clears the old chain and routes subsequent keys to
root BODY; it does not restore a previously focused parent input. Navigation
uses the native current document. Remembered editing selection after explicit
blur/window focus, complete cross-frame Tab traversal, OS focus and IME are not
claimed by this increment; frame identity does not substitute for those APIs.

## Child-frame geometry

A child document's viewport is its iframe element's content box in the
parent's layout (Chrome's: a default iframe gives 300x150), with the parent
viewport's device pixel ratio and media features. `innerWidth`/`innerHeight`,
`clientWidth` of the root, `matchMedia` and every layout query inside the
child reflect it. The iframe box is read again, when the child's layout is
next read, only after the parent's layout inputs changed (a parent restyle or
relayout, an emulated resize); pages without iframes do no work.

When the box changes after the first sizing, the child `window` (and its
`visualViewport`) gets one `resize` task, then `change` on media query lists
that flipped, matching Chrome's sequence for restyles and
`Emulation.setDeviceMetricsOverride`. The change is noticed where the child
viewport is re-read: a layout read in the child, the parent's emulated resize
task, and, for a child window with event listeners, the child's after-task
checkpoint (at most every max(16 ms, twice the re-read's cost), like
ResizeObserver; top documents and listener-less child windows return at
once).

`DOM.getBoxModel`, `getContentQuads` and Playwright bounding boxes report child
geometry in main-frame viewport coordinates, as Chrome does, and mouse input at
main-frame coordinates reaches child content.

Stable WindowProxy behavior across replacement, cross-frame messages, full
sandbox/permissions policy, cross-frame coordinates, joint history, and frame
file uploads remain outside this increment. This is not a TOS iframe or DOCX
compatibility claim; no TOS fixture or assertion was changed for this work.

## Regression coverage

`crates/pyro-cdp/tests/engine_cdp/frames.rs` exercises real browser documents:
opaque sandbox srcdoc, native owner/DOM identity, stale handles, sibling-safe
navigation/removal, late lifecycle replay, HTTP document/fetch response routing,
per-frame body cleanup, Fetch fulfillment and detach cancellation, concurrent
child navigation, inherited/opaque origins, isolated-world init scripts, and
explicit geometry rejection and native focus/key routing. A script that removes another frame during
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

The focus increment retains ten direct-CDP contracts for parent/child/sandbox
routing, sibling and page isolation, nested removal, child DOM.focus,
same-document handler reentrancy, cross-frame default suppression and raw-char
sequence boundaries/invalid input. Eight initial contracts were captured red
before integration; the same-document native bug turned green on W2's dependency.
The Chromium147 HTTP reference scripts/results remain on preparation commit
`f28ee99b3ee75ab46543214125be165b55927c26` under `diag/frame-focus`, independently
of this production delta. Two real Playwright flows add parent/child fill/press
across three sandbox modes and raw-key/nested-removal routing. These local flows
are not a new TOS or performance result.
