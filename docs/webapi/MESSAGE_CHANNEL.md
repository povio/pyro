# MessageChannel and MessagePort

`crates/lp-webapi/src/message_channel.rs` implements HTML §9.4 channel
messaging within one realm. The behaviour follows upstream Lightpanda
(`MessagePort.zig`) and Chromium, except for the transfer gap below.

- **Construction.** `new MessageChannel()` creates two entangled ports.
  `port1`/`port2` always return the same objects. Ports are `EventTarget`s with
  `onmessage` and `onmessageerror`.
- **Posting.** `postMessage(message, transferOrOptions)` structured-clones the
  message.
  - It uses the realm's original `structuredClone`, so page reassignment of
    `structuredClone` has no effect.
  - Non-cloneable values throw DataCloneError, including functions and host
    objects such as a MessagePort.
  - The message is queued on the entangled port. Posting on a closed or
    disentangled port clones and then drops the message.
- **Delivery.**
  - A started port delivers each message in its own task, in FIFO order, after
    the current task's microtasks.
  - Each message arrives as a trusted `MessageEvent` with `origin` `""`,
    `source` `null` and an empty `ports`.
  - Messages wait until the port is started. `start()` or assigning a function to
    `onmessage` starts it; `addEventListener('message', …)` does not.
- **Closing.** `close()` disentangles both ports and drops the closed port's
  undelivered messages. Delivery tasks that are already queued check the closed
  state, and nothing is delivered after page disposal.
- **Lifetime.** A port is pinned exactly while it is started, open, and either
  entangled or holding undelivered messages. Its listeners keep running when only
  closures reference the channel, which React's scheduler relies on.
  - Queued messages survive GC even after the peer closes, or when the receiver
    starts after its peer was collected.
  - The pin is re-evaluated on enqueue, start, delivery and close (including the
    peer's), and when the peer is collected. The peer's `Drop` only records the
    surviving port; `PageDom::after_task` reconciles it at the next safe point.
    No JS runs from the drop, with or without page hooks.
    Every pin is released when the page is disposed.
  - A started, drained receiver whose sender became unreachable is released,
    not retained until page disposal.
  - A queued but unstarted port is not pinned: if nothing can reach it to call
    `start()`, it is collectable together with its queue.
  - An unclosed, started, entangled port lives until page disposal. The channel
  references its ports only through traced hidden properties on its wrapper, so
  listeners or expandos that reference the channel stay collectable once the
  ports are closed.

## BroadcastChannel

`new BroadcastChannel(name)` (HTML §9.5) reaches every other open channel of
that name whose document has the same origin in the same browser context
(pages of one CDP browser context; documents without an embedder each form
their own), across all pages of the browser thread.

- `postMessage(message)` serializes the message once in the sender's realm
  (the worker wire format, so non-cloneable values throw DataCloneError and
  nothing is transferred) and each receiver deserializes it in its own realm,
  in a task on its own page, as a trusted `MessageEvent` with `origin` the
  sender's, `source` null and no `ports`; a message that cannot be
  deserialized there fires `messageerror`. Other channels of the sending
  document receive it, the sending channel does not. Posting on a closed
  channel throws InvalidStateError.
- An open channel is pinned, so listeners added without keeping a JS
  reference keep running; `close()` releases the pin, and every pin goes with
  the page.
- Gap: workers have no BroadcastChannel.

Tests: `crates/lp-browser/tests/popups.rs`
(`broadcast_channel_reaches_same_origin_documents_of_the_browser_context`).

Gaps of MessageChannel, all explicit:
- Transferring anything throws NotSupportedError, whether as
  `postMessage(m, [port])` or `{transfer: [...]}`. This includes MessagePorts,
  so ports never leave their realm and are never shipped to workers or frames.
- No `messageerror` events: same-realm deserialization cannot fail.
- `window.postMessage` still passes values without cloning; see
  [STRUCTURED_CLONE](STRUCTURED_CLONE.md).

Same-window `window.postMessage` events expose the receiving world's Window
as `event.source`. The source is stored as a native window target key, with no
event-to-global JavaScript persistent root. This supports schedulers that accept
only messages where `event.source === window`, including JSZip's `setImmediate`
fallback. Delivery uses the embedder's queued `PageHooks::post_task`; embedders
without hooks retain the existing inline fallback. Port events and constructed
MessageEvents without a source continue to return `null`.

This does not add cross-window messaging, target-origin enforcement, transfer
support, or `MessageEventInit.source` handling. It does not change message-data
retention or cloning semantics.

Tests (`crates/lp-webapi/tests/dom_js.rs`):
- `message_channel_delivers_cloned_messages_fifo_as_tasks`
- `message_port_start_close_transfer_and_clone_errors`
- `message_port_lifetime_pins_started_ports_until_close`
- `message_channel_with_self_referencing_listener_is_collected_after_close`
- `message_port_keeps_queued_messages_across_gc_when_the_peer_closes`
- `message_port_started_late_with_a_collected_peer_still_delivers_across_gc`
- `message_port_unstarted_queued_ports_and_disposal_hold_no_roots`
- `message_port_receiver_is_released_when_its_sender_becomes_unreachable`
- `message_port_peer_collection_is_reconciled_at_after_task_without_hooks`
