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
    peer's), and when the peer is collected (reconciled in a separate task).
    Every pin is released when the page is disposed.
  - A started, drained receiver whose sender became unreachable is released,
    not retained until page disposal.
  - A queued but unstarted port is not pinned: if nothing can reach it to call
    `start()`, it is collectable together with its queue.
  - An unclosed, started, entangled port lives until page disposal. The channel
  references its ports only through traced hidden properties on its wrapper, so
  listeners or expandos that reference the channel stay collectable once the
  ports are closed.

Gaps, all explicit:
- Transferring anything throws NotSupportedError, whether as
  `postMessage(m, [port])` or `{transfer: [...]}`. This includes MessagePorts,
  so ports never leave their realm and are never shipped to workers or frames.
- No `messageerror` events: same-realm deserialization cannot fail.
- `window.postMessage` still passes values without cloning; see
  [STRUCTURED_CLONE](STRUCTURED_CLONE.md).

Tests (`crates/lp-webapi/tests/dom_js.rs`):
- `message_channel_delivers_cloned_messages_fifo_as_tasks`
- `message_port_start_close_transfer_and_clone_errors`
- `message_port_lifetime_pins_started_ports_until_close`
- `message_channel_with_self_referencing_listener_is_collected_after_close`
- `message_port_keeps_queued_messages_across_gc_when_the_peer_closes`
- `message_port_started_late_with_a_collected_peer_still_delivers_across_gc`
- `message_port_unstarted_queued_ports_and_disposal_hold_no_roots`
- `message_port_receiver_is_released_when_its_sender_becomes_unreachable`
