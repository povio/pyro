# window.event

`window.event` follows the DOM "inner invoke" current-event steps, as upstream
Lightpanda (`Window.zig` `_current_event`) and Chromium do.

## Behaviour

- **Setting the current event.** While a listener is invoked:
  - the current event of the listener's realm global (a Window) is set to the
    dispatched event;
  - the event is not set when the invocation target is in a shadow tree;
  - afterwards the previous value is restored. This also happens when the
    listener throws, and around nested synchronous dispatch.
- **Outside dispatch** `window.event` is `undefined`.
- **Microtask checkpoint.** A task-level listener invocation is the outermost
  JavaScriptCore API call, so JSC drains the microtask queue as it returns, which
  is before the restore. Microtasks queued by the listener therefore still see
  the event, matching Chromium and upstream. Later tasks such as timers see
  `undefined`.
- **[Replaceable].** Assigning `window.event` defines an own data property on the
  global (writable, enumerable, configurable) that shadows the accessor. The
  realm's original `Object.defineProperty` is captured when pyro-webapi installs.

## Storage and lifetime

The current event is stored on the realm's traced window holder, the same
per-realm JS object that keeps window listeners. It is not a native owning
reference. It is cleared when the listener returns, so the event stays
collectable after dispatch, and the holder is released with the realm or the
page.

## Tests

In `crates/pyro-webapi/tests/dom_js.rs`:
- `window_event_is_the_current_event_during_listener_invocation`: identity,
  nested save/restore, restore on throw.
- `window_event_spans_the_callback_microtask_checkpoint_for_message_ports`:
  `onmessage`, `addEventListener('message')`, and the callback's microtask.
- `window_event_is_replaceable_and_skips_shadow_tree_targets`.
- `window_event_does_not_retain_the_event_after_dispatch`.

## Gaps

- Microtask timing for listeners invoked from inside other JavaScript (nested
  dispatch) follows the JSC drain rule: there is no checkpoint until the outer
  call returns, which matches the DOM.
