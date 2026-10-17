# MutationObserver

`MutationObserver` and `MutationRecord` expose the native DOM registration and
record queues. Supported options are childList, attributes, characterData,
subtree, attributeFilter and both old-value flags. Options are converted before
borrowing the DOM; re-observing replaces options, disconnect discards queued
records, and takeRecords returns typed records without invoking the callback.
Added/removed node lists are static and retain node identity after removal.

Installation owns the realm's [native callback scheduling hook](../runtime/NATIVE_CALLBACK_HOOK.md).
The hook captures PageDom weakly, uses try_borrow for native reentrancy, and
queues one Promise reaction for pending notifications. It does not directly
invoke observer callbacks. Mutations queue ahead of Promise reactions created
later in the same script, including nested native-to-JS-to-native calls.
PageDom.after_task remains necessary for parser and other native mutations
outside JavaScript callbacks. Embedders must not replace this hook without
preserving that checkpoint behavior.

Callbacks execute in the observer's realm with the observer as this and the
second argument. Exceptions are reported without preventing other callbacks.
Detached subtree registrations expire at notification. If a scheduling realm
is disposed, its stale reaction is ignored and remaining observers can schedule
in another live realm. Call lifecycle::realm_disposed before disposing a realm;
PageDom.dispose cancels all observers and releases connected and pending roots.

Observer callback/target links are traced JS properties. Detached closure cycles
have no native self-root; connected observed nodes and pending deliveries retain
wrappers until no longer needed. Records retain all referenced native nodes.
There is no polling. Checkpoints visit newly pending observers; connected-target
reconciliation visits registered targets only when the DOM version changes.
Record queues preserve all observable records and are not byte-capped. This is
not a measured CPU or memory improvement claim.

This increment supports worlds sharing one PageDom. Observing a node from another
PageDom throws NotSupportedError. ResizeObserver, IntersectionObserver and
slotchange delivery are not implemented. Strings retain the DOM's existing UTF-8
limitation for lone UTF-16 surrogates. The queue/delivery model follows the
[DOM mutation observer algorithms](https://dom.spec.whatwg.org/#mutation-observers),
but these focused tests are not a full WPT conformance result.

Real JavaScriptCore tests cover record fields and stable node lists, option
validation and reentrant getters, filtering, batching and reentrant callbacks,
disconnect/re-observe/takeRecords, transient registrations, callback exceptions,
main/isolated realms, Promise ordering, native safe points, disposed scheduling
realms, detached closure collection and PageDom teardown.
