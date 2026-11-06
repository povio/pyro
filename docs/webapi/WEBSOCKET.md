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
    `Accept-Language`, `Sec-WebSocket-Version: 13`, `Sec-WebSocket-Key` and
    `Sec-WebSocket-Protocol`. It also carries the jar's cookies for the
    `http(s)` form of the URL (credentials mode "include"), the context's and
    page's extra headers, and Basic credentials taken from URL user info. No
    `Referer` is sent.
  - The connection fails when the response is not 101, the Upgrade, Connection
    or Accept headers are wrong, or the server sends extensions.
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
- **Receiving.**
  - Server frames must be unmasked and have no reserved bits.
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
    text payloads as is, binary as base64.
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

- No extensions are offered, so `extensions` is always `""` and there is no
  permessage-deflate.
- WebSockets over HTTP/2 (RFC 8441) are not supported.
- Connections in shared and service workers are not supported (those workers
  do not exist).
- `WebSocketStream` is not implemented.
- Frames are reported to CDP once per message, not once per wire frame.
