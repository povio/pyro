# Child frames (iframe) phase 1: browser implementation

Status: browser implementation; CDP routing and unchanged application preview validation are separate W3 integration work.
The implementation consumes W2 H1–H4 at `a05fb6fcb0742efc848b982fd6735c4880cfe494`. Scope and exclusions follow
the parent-approved phase 1: same-process child frames for `srcdoc`, `src` (HTTP) and `about:blank` documents,
sandbox flags, per-frame realms, lifecycle, teardown, and a restricted cross-origin `WindowProxy`.

Excluded:
- cross-frame layout, hit testing and input coordinates (input into a child frame fails explicitly);
- `postMessage` between frames;
- permissions policy `allow`;
- `loading=lazy`;
- popups;
- joint session history (child navigations do not add top-level history entries).

Reference behaviour: the Chrome 147 probe in `target/worker-handoff/iframe-assessment/`:
- a `sandbox="allow-downloads"` srcdoc frame is at `about:srcdoc`, with an opaque origin;
- page scripts do not run, but the default and utility CDP contexts exist and evaluate;
- the parent's `contentDocument` is `null`, while `contentWindow` exists.

## Model
- Each page owns a `FrameTree` (`Rc<RefCell<…>>`, shared like `PageNav`). The main frame stays `Page::main_frame`;
  child frames are stored flat with `parent: FrameId`, `owner: NodeId` (the `<iframe>` element in the parent's
  document) and `name`. Each child frame has its own `Frame` (url, loader id, realms, `PageDom`, network-idle state)
  and its own `PageNav`.
- Child documents live in the page's shared DOM store; each has its own `PageDom` (window target, request context
  with the child `frame_id`/`loader_id`).
- Request defaults (user agent, extra headers, cache bypass) are the page's: Chrome applies them per target, i.e.
  to every frame. lp-net activity is counted under the child loader, so network idle is per frame.

## Creation (H1, synchronous) and navigation (asynchronous)
- `iframe_connected(iframe)` runs inside script (e.g. `appendChild`) while `&mut Browser` is borrowed up the stack.
  It therefore goes through a page-owned `FrameHost` (scheduler, loop handle, observer cell, `Window` class id,
  page/context ids, init scripts) and a new lp-js entry point that creates a realm in the thread's current engine.
- What H1 does synchronously:
  - creates the child's initial `about:blank` document (origin inherited from the parent unless sandboxed), with
    realms for the default world and every init-script world;
  - registers the frame;
  - emits `FrameChildFrameCreated`, `RealmCreated` and `FrameNavigated` for the child;
  - posts a task to "process the iframe attributes".
- That task, running with `&mut Browser`, navigates the child to `about:srcdoc` (the `srcdoc` text; base URL = the
  parent document's; no network request) or to the `src` URL (an HTTP document through the context loader, as for
  the main frame), and commits it through the generalised `commit_document` (a frame target instead of the main
  frame).
- `iframe_attribute_changed(Src|Srcdoc)` navigates again. `Name` updates the frame name. `Sandbox` is recorded
  and applies at the next navigation (HTML).

## Sandbox
- Flags are parsed from the owner at document commit and combined with ancestor restrictions. Changing the attribute alone does not change the active document.
- Without `allow-same-origin` the document gets an opaque origin: realm origin `null`, `securityOrigin "://"`, and
  ephemeral storage as for other opaque documents.
- Without `allow-scripts`:
  - parser scripts are not executed;
  - `PageDom::set_scripting_disabled(true)` (H4) stops event-handler attributes and `javascript:` URLs;
  - the realms still exist and stay usable for CDP evaluation and isolated worlds (as in Chrome).

## Lifecycle and teardown
- Per frame: `FrameDomContentLoaded`, `FrameLoaded`, network almost-idle/idle (the existing events, with the
  child `frame_id`), the `<iframe>` element's `load` event in the parent document, and the parent's `load` waiting
  for its child frames' `load`.
- Teardown (H2 `iframe_disconnected`, a `src`/`srcdoc` change, parent document replacement, page close) is
  depth-first:
  1. for each descendant: dispose its `PageDom`, cancel timers and dispose its realms, emitting `RealmDestroyed`;
  2. emit `FrameChildFrameDetached`;
  3. forget its lp-net activity entry.
  Queued tasks for a removed frame are dropped by frame and loader checks (stale guard).

## Window relationships
- `parent`, `top`, `frames`, `length`, `frameElement` and `contentWindow` become real (lp-browser `Window`).
- Same-origin access returns the other frame's global. Cross-origin, including opaque, access returns a restricted
  cross-origin window object: allow-listed members (`window`, `self`, `parent`, `top`, `frames`, `length`,
  `closed`, `close`, `focus`, `blur`; `postMessage` is excluded in phase 1); everything else throws
  `SecurityError`.
- `contentDocument` and `frameElement` are `null` across origins.

## For lp-cdp (W3)
- Events: the existing `PageEvent` variants, carrying the child `frame_id`. No field changes, so lp-cdp keeps
  compiling. `FrameChildFrameCreated{parent, frame_id}` precedes all other events for that frame.
  `FrameChildFrameDetached` follows that frame's `RealmDestroyed` events.
- Accessors:
  - `Browser::frames(page) -> Vec<FrameInfo { id, parent: Option<FrameId>, owner: Option<NodeId>, url, loader_id,
    name, origin, network_idle, dom_content_loaded, loaded }>`;
  - `Browser::frame(page, &FrameId) -> Option<Frame>` (owned snapshot with cloned Realm/PageDom handles);
  - `Browser::frame_of_owner(page, NodeId) -> Option<FrameId>`;
  - `Browser::isolated_world_in_frame(page, &FrameId, name) -> Result<Realm, JsError>`;
  - `Browser::navigate_frame(page, &FrameId, NavigateOpts) -> Result<LoaderId, JsError>`.
  These accessors never return a live frame-registry borrow across JavaScript. CDP must scope context
  destruction, pending navigation and lifecycle replay to the affected frame, keeping siblings alive.

## Tests (lp-browser, real HTTP where it matters)
- Sandboxed srcdoc: the child document's text is readable through the child realm; page scripts did not run; the
  realm evaluates; the parent's `contentDocument` is `null`; the cross-origin `contentWindow` throws on a
  disallowed property.
- Same-origin srcdoc and `src`: `contentDocument`, `parent`, `top` and `frameElement` relationships hold. The
  `src` request carries the child frame and loader ids and the page's extra headers.
- Event order: created, then realms, then navigated, then DCL/load per frame, then the iframe element's `load`,
  then the parent `load`. Removal: realms destroyed, then detached. Nested frames tear down depth-first.
  Replacement drops stale queued navigations.
- `src` change re-navigation; network idle per frame; init-script worlds in every frame.

## Ownership and safe points

`Page` retains its public main-frame view through `DocumentState`; each child has the same document loader,
stylesheet state and navigation machinery. `FrameHost` owns child states and an owner-thread root snapshot.
Bindings and window relationship wrappers retain the host weakly. A root snapshot can also hold isolated
worlds created by cross-frame access; root synchronization and teardown preserve those worlds until disposal.
No registry/DOM borrow spans realm creation, script evaluation or an observer callback.

Connection creates the initial realm synchronously via `Engine::create_realm_in_current_thread`, which rejects
an absent or dropped owning engine. Disconnection removes the frame from lookup immediately; disposal waits
until the browser checkpoint so a child removing itself can finish the active native/JS call. Retirement is
bounded by the removed subtree and drains depth-first. Child count is capped at 128 per page and depth at 32;
exceeding either reports an exception and creates no additional frame.

Pending document fetches have cancellation handles and loader guards. Detach/replacement drops cancellation
ownership and forgets both the committed and uncommitted loader activity; late completions cannot resurrect
the document. A failed initial child fetch reports
`FrameNavigateFailed`, leaves the initial empty document usable, and releases the parent's load blocker.
Timers, animation callbacks and parser scripts are suppressed for a script-disabled document, while explicit
inspector evaluation and inspector init-script worlds remain available.

## Bounded behavior and validation scope

`child_frames.rs` covers synchronous blank creation, srcdoc, actual document identity, inherited/opaque
origins, sandbox navigation, named and unnamed isolated worlds, init-script worlds, nested removal,
self-removal, parent-load ordering, failed/cancelled loads, stale completions and repeated realm cleanup.
`child_frame_network.rs` uses a local HTTP server for document/script/fetch request correlation, headers,
inherited base resolution, frame network idle and activity cleanup. `lp-js/tests/nested_realm.rs` exercises
safe realm creation from a native callback and rejects calls without a live owning engine.

Same-origin window access deliberately returns the other realm's global, as agreed for this phase. It does
not provide stable WindowProxy identity across document replacement. The cross-origin object provides only
the documented allow-list; it does not expose the other document. Frame name metadata is supported, while
named window-property lookup is not implemented. HTTP error response bodies still commit; transport failures
do not synthesize a browser error page. There is no joint session history, cross-frame message delivery,
cross-frame layout/input or popup support in this slice. Native HTTPS and other existing loader limitations
remain. Inherited base URLs do not implement HTML `<base>` or cookie inheritance for blank/srcdoc documents.
Opaque storage retains the existing isolated ephemeral-storage policy. No full browser or TOS compatibility,
performance improvement, or resolution of the separately tracked system-JSC trap is claimed.
