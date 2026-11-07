# MessageChannel and MessagePort

`crates/lp-webapi/src/message_channel.rs` implements HTML §9.4 channel
messaging between the realms of a browser thread's VM (frames, popups,
isolated worlds) and, through `port_link.rs`, between threads (the page and
its dedicated or service workers, or two workers). The behaviour follows upstream
Lightpanda (`MessagePort.zig`) and Chromium, except for the gaps below.

- **Construction.** `new MessageChannel()` creates two entangled ports.
  `port1`/`port2` always return the same objects. Ports are `EventTarget`s with
  `onmessage` and `onmessageerror`.
- **Posting.** `postMessage(message, transferOrOptions)` structured-clones the
  message.
  - It uses the realm's original `structuredClone`, so page reassignment of
    `structuredClone` has no effect. The message is cloned into the realm of
    the entangled port, which differs from the sender's once a port was
    transferred to another frame or page.
  - Non-cloneable values throw DataCloneError, including functions and a
    MessagePort that is not in the transfer list.
  - The transfer list takes ArrayBuffers (detached) and MessagePorts. Listing
    the posting port itself throws DataCloneError; listing its entangled peer
    transfers it but drops the message (the channel is lost, as in HTML).
  - The message is queued on the entangled port. Posting on a closed,
    disentangled or detached port clones and then drops the message.
- **Transfer.** A transferred port (through `postMessage`, `structuredClone`
  or `window.postMessage`) is replaced in the receiving realm by a new port
  that takes over the entanglement and the undelivered messages (cloned into
  the new realm when it is another one), starts unstarted, and arrives in the
  data where the original was and in `event.ports`, in transfer-list order.
  The original is detached: it is closed, its posts are dropped and
  transferring it again throws DataCloneError.
- **Across threads.** A port transferred through `Worker.postMessage`, the
  worker global's `postMessage`, `ServiceWorker.postMessage`,
  `Client.postMessage` (from a service worker), a port that is itself
  entangled across threads, or a popup's `postMessage` leaves the VM. The first time one port
  of a pair does, the pair switches to a thread-safe pipe: each side has an
  inbox of serialized messages (the [cross-VM format](STRUCTURED_CLONE.md))
  and an owner, the poster of the thread whose port stands for it and that
  port's id in the thread's registry. The port that stays takes over its
  side; the leaving port's undelivered messages are serialized into the head
  of its side's inbox, and the side travels inside the message, without an
  owner, until the receiving realm deserializes it into a new port.
  - Posting serializes the message in the sender's realm, pushes it on the
    other side's inbox and posts one wake-up task to the owner's event loop,
    which sleeps until then (no polling). A side in transit collects its
    messages until its new port exists.
  - The receiving port deserializes a message only when it delivers it, one
    task per message, so messages that are still queued follow it if it
    moves again. A message that cannot be deserialized there fires
    `messageerror` (a plain trusted MessageEvent without data).
  - `close()` closes the side: its queued messages are dropped, later posts
    from either side are dropped, and the other side's owner is told so its
    pin is re-evaluated. The same happens when the port is collected, when
    its document goes away (the worker ends, or the page navigates), and
    when a message carrying a side is dropped before it was deserialized.
  - Posting the entangled port through a port drops the message (the
    channel is lost), as within a thread.
  - One side's inbox holds at most 64 MiB of messages: past that, posting
    throws DataCloneError (unless the inbox is empty).
- **Delivery.**
  - A started port delivers each message in its own task, in FIFO order, after
    the current task's microtasks.
  - Each message arrives as a trusted `MessageEvent` with `origin` `""`,
    `source` `null` and `ports` holding the transferred ports (the same array
    on every read).
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
- Within one thread no `messageerror` fires: deserialization in the same VM
  cannot fail.
- MessagePort has no `close` event (fired by recent Chromium when the
  entangled port goes away).
- A port transferred to a page while its document navigates away, or whose
  pipe was made by a page that went away while both its ports were in
  transit, arrives closed: a page's disposal closes the pipes it made whose
  sides no port owns (which also releases ports that travel inside each
  other's undelivered messages).
- A port's undelivered messages are serialized when it leaves the thread;
  past 16 levels of ports nested in undelivered messages of ports, the inner
  messages are dropped.
- `new MessageEvent(type, {ports})` keeps the ports; `source` in the init
  dictionary is ignored.

## window.postMessage

`crates/lp-webapi/src/window_message.rs` and `crates/lp-browser/src/frames.rs`
(`post_message`) implement HTML's window post message steps for frames and
popups of a browser thread:

- Arguments follow WebIDL overload resolution: `(message, targetOrigin,
  transfer)` with three arguments, otherwise an object (or nullish) second
  argument is the `{targetOrigin, transfer}` dictionary. `targetOrigin` is `*`,
  `/` (the source's origin) or an absolute URL whose origin must match the
  target document's; anything else throws SyntaxError (from the realm of the
  `postMessage` called). An opaque-origin URL never matches.
- The message is structured-cloned straight into the target window's
  main-world realm (DataCloneError and getter exceptions propagate), with
  transfers; a mismatched target origin clones, transfers and then drops it.
  A primitive message without a transfer list is not cloned, so pages using
  `postMessage` as a macrotask scheduler pay one task per message, as before.
- Delivery is a task on the target document: a trusted `MessageEvent` with
  `origin` the source's serialized origin, `ports` the transferred ports, and
  `source` the source's WindowProxy as the reading realm sees it (its global
  when same-origin, the cross-origin stand-in otherwise, the popup's `opener`,
  or in an opener the object its `window.open` returned). The event keeps no
  root of the source window: `source` is resolved when read.
- The source is the incumbent window: the realm of the innermost script,
  listener, timer or other native-to-JavaScript entry that is running
  (`lp_js::entry_realm`). Calling a function of another realm directly from
  script does not change it, so a message posted from a function defined in
  another frame names the caller's window, where HTML would name the
  function's.
- Cross-origin windows (frames, and a popup's cross-origin opener) expose
  `postMessage`, `location` (only setting `href`, assigning `location`, and
  `replace()`; URLs resolve against the accessing document, `javascript:` is
  ignored), `frames[i]` / indexed access to their child windows, `closed`,
  `length`, `window`/`self`/`frames`/`parent`/`top`, `focus`, `blur` and
  `close`; everything else throws SecurityError.
- Gaps: a message between a frame inside a popup (or inside its opener) and
  the other page has `source` null; messages from the opener through the
  `window.open` result (see [POPUP_WINDOW](POPUP_WINDOW.md)) have `source`
  null; opaque origins compare by their serialization ("null"), not their
  identity; `opener` is not exposed on cross-origin windows; a popup's view of
  its cross-origin opener has no indexed access.

Same-window `window.postMessage` events expose the receiving world's Window
as `event.source`. The source is stored as a native window target key, with no
event-to-global JavaScript persistent root. This supports schedulers that accept
only messages where `event.source === window`, including JSZip's `setImmediate`
fallback. Delivery uses the embedder's queued `PageHooks::post_task`; embedders
without hooks retain the existing inline fallback.

Tests: `crates/lp-webapi/tests/dom_js.rs` (`message_channel_*`,
`message_port_*`), `crates/lp-webapi/tests/messaging_js.rs` (clone types,
transfers, event ports, same-window postMessage, the cross-VM format, stream
transfer), `crates/lp-webapi/src/port_link.rs` (the pipe),
`crates/lp-browser/src/tests.rs` (`message_ports_cross_*`,
`a_port_from_a_worker_*`, `two_workers_talk_*`, `streams_transfer_*`,
`crypto_keys_cross_*`) and `crates/lp-browser/tests/messaging.rs` (frames,
cross-origin windows, popups, history state, streams to a frame).
