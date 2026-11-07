# WebSocket and CloseEvent

`crates/lp-webapi/src/websocket.rs` and `websocket.js` implement the HTML
`WebSocket` interface and `CloseEvent` in windows and dedicated workers. The
connection (RFC 6455) is lp-net's: `crates/lp-net/src/websocket.rs`, on the
browser context's I/O thread. Where the spec and Chrome differ, Chrome wins.

- **Lazy.** Both interfaces are made in JS on a realm's first read of either
  name, like streams. Until then a realm has one accessor pair per name and
  nothing else. Assigning the name replaces it, as for any global.
- **Constructor.**
  - The URL is resolved against the document's base URL (a worker's script URL).
  - `http:`/`https:` become `ws:`/`wss:`.
  - These throw `SyntaxError`: a URL that fails to parse, another scheme, and a
    fragment.
  - `protocols` is a string, a sequence or an option bag `{protocols}`. Each
    subprotocol must be an HTTP token, and none may repeat (ASCII
    case-insensitive). A violation is a `SyntaxError`.
  - A secure (https) document that opens a `ws:` connection to a host that is
    not potentially trustworthy fails asynchronously with `error` and `close`.
    It does not throw.
  - Fetch "bad ports" fail asynchronously too.
- **Handshake.**
  - The handshake goes over the context's connector: the context proxy as a
    CONNECT tunnel (for `ws:` as well), and rustls with the context's certificate
    policy for `wss:`. Only HTTP/1.1 is offered by ALPN.
  - The request carries Chrome's headers: `Origin`, `User-Agent`,
    `Accept-Language`, `Sec-WebSocket-Version: 13`, `Sec-WebSocket-Key`,
    `Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits`
    (Chrome's offer) and `Sec-WebSocket-Protocol`. It also carries the jar's cookies for the
    `http(s)` form of the URL (credentials mode "include"), the context's and
    page's extra headers, and Basic credentials taken from URL user info. No
    `Referer` is sent.
  - The connection fails when the response is not 101, the Upgrade, Connection
    or Accept headers are wrong, or the extension negotiation is bad (see
    permessage-deflate below).
  - It also fails on a subprotocol the client did not offer, or on no
    subprotocol when the client offered some. That last case is Chrome's rule.
  - Redirects are not followed.
  - The response's `Set-Cookie` headers are stored when the `open` task runs, so
    script never sees them earlier (WPT `cookies/007`).
- **Events.**
  - Every network event is its own task on the owner's event loop. The I/O
    thread posts it through the page's remote poster; nothing polls.
  - `open`, `message`, `error` and `close` are trusted.
  - A `message` is a `MessageEvent` whose `origin` is the socket URL's origin.
    Messages that arrive once `readyState` is not OPEN are dropped.
  - Failing the connection (handshake error, protocol violation, `close()`
    while CONNECTING) sets CLOSED, fires `error`, then `close` with
    `wasClean: false` and code 1006.
  - A TCP close without a Close frame fires only `close` (1006, unclean), as in
    Chrome.
- **Sending.**
  - `send()` takes a string (USVString, sent as UTF-8 text), a Blob (its bytes
    at the time of the call), an ArrayBuffer or a view (binary).
  - It throws `InvalidStateError` while CONNECTING.
  - `bufferedAmount` grows by the payload's byte length. It shrinks only in a
    task once the bytes are written, so it does not change during a sync XHR.
    Those updates are coalesced into one task at a time.
  - After `close()`, sends only grow `bufferedAmount`.
- **Closing.**
  - `close(code, reason)` applies WebIDL `[Clamp] unsigned short` to the code.
    The code must be 1000 or 3000–4999, else `InvalidAccessError`. The reason is
    a USVString of at most 123 UTF-8 bytes, else `SyntaxError`.
  - The client waits up to 60 s for the server's Close frame, then up to 2 s for
    the TCP close. These are Chromium's timeouts.
  - A received Close is echoed with its code. Its code is checked with
    Chromium's rules, and its reason must be UTF-8.
  - A Close without a code reports 1005.
- **permessage-deflate (RFC 7692).** `crates/lp-net/src/ws_deflate.rs`.
  - The response's `Sec-WebSocket-Extensions` is checked as Chromium checks
    it. The connection fails, with Chromium's message, on a value that does
    not parse, an extension other than `permessage-deflate`, a second
    `permessage-deflate`, an unknown or repeated parameter, a value on
    `server_no_context_takeover`/`client_no_context_takeover`, or window bits
    that are missing or not 8–15 (no leading zero; quoted values are
    unquoted first).
  - `extensions` is the response's header values joined with `", "`, as in
    Chromium, or `""` when the server accepted nothing.
  - Inbound messages with RSV1 set are inflated with a 32 KiB window, which
    decodes any `server_max_window_bits`. The window carries over between
    messages; it is reset after a final DEFLATE block and with
    `server_no_context_takeover`. Uncompressed messages (RSV1 clear) are
    accepted too.
  - Inflation stops at the message size limit, so a small compressed
    message cannot expand without bound: going over fails the connection
    with 1009. Data that does not inflate fails it with 1002 "Invalid frame
    header" (Chromium's message). RSV1 on a control or continuation frame is
    1002 as well.
  - Every outbound data message is deflated (level 6, sync flush, the
    trailing `00 00 ff ff` removed) and sent with RSV1. The window carries
    over unless the server sent `client_no_context_takeover`.
  - The compressor always uses a 32 KiB window. When the server sets
    `client_max_window_bits` below 15, the compressor is reset before every
    message, and a message longer than that window is sent uncompressed.
    RFC 7692 allows this for any message.
  - The compressor (about 300 KiB) and decompressor (about 40 KiB) are
    allocated on a connection's first message in each direction. Connections
    without the extension do not allocate them, and their per-frame cost is
    one flag test.
  - `bufferedAmount` and CDP count and show uncompressed payloads, as in
    Chrome.
- **Receiving.**
  - Server frames must be unmasked. RSV2 and RSV3 must be clear, and RSV1
    too unless permessage-deflate was negotiated.
  - Control frames must not be fragmented and carry at most 125 bytes.
  - Continuation frames must follow a fragmented message.
  - Text must be valid UTF-8.
  - A message larger than the context's body limit (256 MiB by default) fails
    the connection with 1009.
  - Each of these violations fails the connection with Close 1002, 1007 or
    1009.
  - Pings are answered.
  - `binaryType` is `"blob"` (the default) or `"arraybuffer"`. Other values are
    ignored.
- **Lifetime.**
  - A socket keeps its `WebSocket` object alive until it is closed.
  - Disposing its realm (navigation, world removal) or its document (page
    teardown, worker termination) aborts the connection at once, with no
    further events.
- **CDP.**
  - `Network.webSocketCreated`, `webSocketWillSendHandshakeRequest`,
    `webSocketHandshakeResponseReceived`, `webSocketFrameSent` and
    `webSocketFrameReceived` are sent in Chrome's shapes. Data messages only:
    text payloads as is, binary as base64. With permessage-deflate, the
    payloads are the uncompressed messages, the handshake request headers
    show the offer, and the response headers show the server's answer, as
    in Chrome.
  - `webSocketFrameError` (with the failure message) and `webSocketClosed` are
    sent too.
  - They go to the sessions of the page that owns the creating frame, while
    Network is enabled. Playwright's `page.on('websocket')`, `framesent`,
    `framereceived`, `socketerror` and `close` work.
  - `routeWebSocket` is Playwright's in-page mock and works on top of the native
    class.
  - These events are built only while a client observes the context's network.
  - `Fetch` interception does not apply to WebSockets, as in Chrome.

## Not implemented

- permessage-deflate is the only extension. Others in the response fail
  the connection, as in Chrome. A server that accepts permessage-deflate
  cannot be refused: no setting turns the offer off.
- The compression level is fixed, and every message is compressed, even
  when that makes it larger. Chrome does the same.
- Compression and inflation run on the context's I/O thread in one pass per
  message, so a very large message delays the context's other network work
  while it is processed.
- Extra HTTP headers cannot replace the `Sec-WebSocket-Extensions` offer.
- WebSockets over HTTP/2 (RFC 8441) are not supported.
- Connections in shared and service workers are not supported (those workers
  do not exist).
- `WebSocketStream` is not implemented.
- Frames are reported to CDP once per message, not once per wire frame.
