# WebSocketStream and WebSocketError

The standalone RC.5 census found 412 signed-Chrome-only named passes on the eligible WebSocketStream pages. This development comparison freezes all 72 eligible target variants plus 20 neighboring classic WebSocket controls. Its SHA-256 is 652131d22e63887a169b1d29dabc8a6fd0b24332dc4a61120d5a36b1607c2854. The 12 previously withheld write variants stayed outside both local runs. Approved targets include Window, dedicated, shared and service workers, with default, WSS and HTTP/2-flagged variants. The signed Chrome stable 158.0.8127.37 projection reuses 80 verified shards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Both LP binaries were built against the pinned static JSC archive using wptiter/render and one compiler job. The parent is 78db92060394f87e18e5a210dd7be42a430fd6c4.

## Behavior

The stream interface uses the existing WebSocket transport and the realm's ReadableStream/WritableStream constructors. Each read credit authorizes one complete data message. The default readable source may prefetch one message to fill its queue before script calls read(). Once credit is exhausted, an additional socket read used to inspect a leading control frame is capped at 127 bytes per read. This is not a total transport or operating-system buffer bound: bytes already read while demand existed may remain buffered, and a control frame arriving behind an unread data frame cannot overtake it. Local close can continue the closing handshake without requiring the consumer to drain application messages. Frame validation and configured message-size limits use the existing transport path.

The writer has one outstanding sink write at a time. It resolves on the transport's completed write/flush notification, including a zero-byte message. That notification does not assert that a remote application consumed the bytes. The AbortSignal hook applies during connection establishment, is removed after opening and rejects both promise attributes on handshake abort. Disposing the owner drops the handle and can cancel blocked transport I/O. The normal page-teardown probe retained both socket and streams until the page was closed and observed the peer's close; this establishes owner teardown, not garbage collection of an otherwise active socket.

Binary messages produce Uint8Array per the current proposal and pinned WPT. Signed Chrome 158 returned ArrayBuffer with the same ordinary payload bytes and slicing boundaries. The proposal remains tentative. The classic socket keeps its event interface and coalesced bufferedAmount notifications. Classic-only lazy access was verified without touching the global Promise accessor. The preceding approved f2eba72f9 was never compiled: the executable uses e1840724e, with the stream-only promise capture adjustment.

- https://github.com/ricea/websocketstream-explainer/blob/master/README.md
- https://websockets.spec.whatwg.org/
- https://streams.spec.whatwg.org/

## Matched results

| Cohort | LP parent pass / observed | LP candidate pass / observed | Signed Chrome pass / observed | Strict pages parent / candidate / Chrome |
| --- | ---: | ---: | ---: | --- |
| 72 stream pages | 0/580 | 420/580 | 412/580 | 0 / 44 / 36 |
| 20 classic guards | 134/142 | 134/142 | 136/142 | 13 / 13 / 14 |

Every one of the 420 added assertions was already observed in the parent. There were zero lost passes, new names or disappeared names. The candidate recovered all 412 signed-only target occurrences and added eight LP-only target passes. All eight assert the binary Uint8Array read type on default/WSS pages across the four environments. Each environment gained 105 passes. The 160 remaining target failures are all in HTTP/2-flagged cases. In that group, both engines passed 40 of 200 named assertions. The archived diagnostics show handshake/connection errors; ordinary native pooled HTTP/2 and compression echo controls are reported separately. LP completed all 92 pages in both runs with zero restarts/watchdog kills. Signed Chrome recorded four target timeouts, all preserved in the source projection. Two classic guard names remain signed-only.

Ten direct controls used a normal local WebSocket server. They covered protocol negotiation, text and sliced/empty binary echoes, orderly and remote close, worker echo, reader replacement, write coercion and errors, abort before/during connect, ignored abort after opening, constructor and error branding, classic lazy initialization, and peer-observed page teardown. Nine output files match signed Chrome exactly. The tenth records the binary read-type difference above; its payload bytes and other outcomes match.

## Commands

Use the sibling cache layout in the measurement scripts. The signed projection and exact parent/candidate runs used the original expectation files and four page workers. Retain raw receipts separately; missing names and timeouts remain distinct.

    python3 signed-reference.py
    python3 run.py base
    ./candidate.sh e1840724efa3f95b8869dc77f57aad26f3bf1884
    node classic-lazy.cjs candidate ./pyro
    node classic.cjs candidate ./pyro
    node idl.cjs candidate ./pyro
    node echo.cjs candidate ./pyro
    node lifecycle.cjs candidate ./pyro
    node abort.cjs candidate ./pyro
    node flow.cjs candidate ./pyro
    node worker.cjs candidate ./pyro
    node conversion.cjs candidate ./pyro
    node teardown.cjs candidate ./pyro
    python3 run.py candidate
    python3 report.py
    python3 classify.py
    python3 families.py
    ./native.sh

The ordinary native selection includes two demand-controlled transport regressions, five existing positive transport guards, one binding unit and eight existing classic JS integration tests. All 16 passed. It does not include malformed wire cases or the pre-withheld WPT write pages.

Independent final-only QA reproduced all 92 lifecycle/strict page outcomes and the complete named occurrence ledger, including all 412 previously signed-only target names and eight LP-only occurrences. All ten direct output files also reproduced. Its receipt, identity hashes and ledgers are in qa/.
