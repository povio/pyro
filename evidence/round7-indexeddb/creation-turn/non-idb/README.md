# Shared script entry control

The same fixture ran serially on the archived parent and candidate. Both matched expected.json on all eight trace steps and all recorded identity checks. The parent listener invoked a same-origin child-created callback, which synchronously dispatched an event back into the outer window. Both callers resumed in order. The next listener and the return from synchronous dispatch preceded the queued microtask.

The fixture observes callback globals, window relationships and the event receiver. Source inspection shows script evaluation and native JavaScript calls install EntryGuard in pyro-js realm.rs and value.rs. These are behavioral observations of shared entry points. They do not sample the native TLS value, count guard drops, or trigger the IDB-specific observed-turn branch. No native instrumentation was used.

An initial parent fixture waited for an about:blank iframe load notification and was stopped without producing a trace. Both recorded runs use the synchronously available child window and the same hashed fixture. Source commit and tree pairs are candidate then parent. Binary and fixture SHA-256 digests and UTC run times accompany the traces.
