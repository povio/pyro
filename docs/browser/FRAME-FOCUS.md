# Frame focus ownership

`Browser::focused_frame(page)` returns a fresh owned `Frame` snapshot, with the
same realm and `PageDom` handles as `frame`. It returns the root initially and
the selected child after element, window, or iframe-owner focus. Inspector
world selection does not change focus. The snapshot is not a retained input
target: consumers resolve it again for each command and reject `None`.

The page's `FrameHost` stores only a frame key and a native cancellation token.
It does not retain a focused `PageDom`, JS value, or editing element. Native
`PageHooks::request_focus` synchronously reconciles the chain, without registry,
DOM, or hook borrows across event dispatch. Ancestor documents activate their
iframe owners. Documents leaving the chain blur deepest first. Each callback
can start a newer transition; its shared token cancels remaining outer steps.
Local native focusing steps preserve same-document `relatedTarget` behavior.

Explicit element blur retains the active frame. Window focus and iframe-owner
focus select the destination body. Remembered editing selection is a separate
native input concern, not inferred by the browser from `activeElement`.
`PageHooks::is_focused` checks both frame identity and the current document
loader so a key handler that changes frames cannot apply default editing to
its old document.

Navigation cancels an in-progress transition and retains the navigating frame
identity with its new document. Removing a focused subtree resets the entire
chain to the root body and clears its remembered editing targets. Neither path
falls back to a previously focused parent input. Page closure has no remaining
focus snapshot. Unrelated pages, siblings, and inspector worlds are unaffected.

The browser regressions cover ancestor state, cross-document reentrancy,
window/owner/blur behavior, isolation, opaque and no-author-script frames,
nested removal, navigation, and removal during a focus callback. The bounded
Chromium147 reference is retained at commit
`f28ee99b3ee75ab46543214125be165b55927c26`, under `diag/frame-focus`. This
slice supplies native frame routing; it does not add cross-frame mouse
coordinates, scrolling, geometry, tab traversal, or OS window activation.
