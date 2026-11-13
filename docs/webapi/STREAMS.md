# Streams

`crates/pyro-webapi/src/streams.js` implements the WHATWG Streams classes;
`streams.rs` makes them on a realm's first use and connects them to fetch and
Blob. A realm that never touches streams only has one global accessor pair per
interface name; the first read of a name replaces its accessor with the usual
data property, and assigning a name replaces it too.

Transfer: `ReadableStream`, `WritableStream` and `TransformStream` are
transferable (`structuredClone(s, {transfer: [s]})`, `postMessage(m, [s])`
on a MessagePort, a window or a worker). Transfer is the standard's
cross-realm transform (`transferKind`/`transferOut`/`transferIn` in
streams.js): the original is piped into (or from) one port of a new
MessageChannel, and the receiving realm's new stream wraps the other port,
with `pull` messages for backpressure (high-water mark 0 on the readable
side, 1 on the writable side), and `close`, `error` and cancellation
travelling both ways. A locked stream throws DataCloneError; the original is
locked once transferred. Through a worker the ports cross threads like any
transferred MessagePort ([MessageChannel](MESSAGE_CHANNEL.md)). A realm
that never used streams pays nothing: the transfer step asks its streams
module only for an entry of a transfer list that is neither an ArrayBuffer
nor a MessagePort, and only if that module exists.

Implemented: `ReadableStream` (start/pull/cancel, strategies, `getReader`,
`tee`, `pipeTo`, `pipeThrough`, async iteration, `ReadableStream.from`),
default and BYOB readers, `WritableStream` and its writer, `TransformStream`,
`ByteLengthQueuingStrategy`, `CountQueuingStrategy`, `TextEncoderStream`,
`TextDecoderStream` (`TextDecoder.decode` now honors `stream: true`),
`CompressionStream` and `DecompressionStream`. The same names exist in
dedicated workers.

Compression (`compression.rs`, flate2's raw deflate with the gzip header and
trailer handled there): formats `"gzip"`, `"deflate"` (zlib) and
`"deflate-raw"`; another format is a TypeError. Chunks must be BufferSources
and come out as Uint8Arrays (empty outputs are not enqueued). Decompression
takes input split anywhere; corrupt data, a bad gzip checksum, bytes after the
end of the compressed data, or input that ends early error the stream with a
TypeError.

Bodies:
- `response.body` of a fetch is a byte stream that delivers bytes as they
  arrive. `bodyUsed` turns true on the first read or cancel, not on `getReader`;
  `text()` and the like reject while the stream is locked.
- `new Response(stream)` keeps the stream as its body (`body === stream`);
  consuming it drains the stream, `clone()` tees it.
- `Blob.stream()` is a byte stream of the blob's bytes, in one chunk.
- `fetch(url, {body: stream, duplex: 'half'})` drains the stream, then sends
  the bytes: the upload is buffered, not incremental. Without `duplex` it
  rejects with a TypeError, as Chrome does.

Gaps:
- Byte streams copy chunks instead of transferring them, and `byobRequest`
  views a private buffer whose bytes are enqueued on `respond()`. A BYOB read
  does transfer the caller's buffer.
- `tee()` of a byte stream uses the default algorithm with copied chunks.
- `clone()` of a fetched Response shares one body: while a clone will still
  read the whole body, the other's `.body` stream copies the bytes it delivers
  and leaves them buffered; two clones that both read through `.body` take the
  bytes from each other.
- A stream from another realm is not recognized as a body.
- There is no `Request` class, so no `Request.body`. `CompressionStream` has
  no `"brotli"`.
- A transferred byte stream arrives as a default stream (as the standard's
  cross-realm transform makes it): no BYOB reader on the receiving side.
- TextEncoderStream, TextDecoderStream, CompressionStream and
  DecompressionStream are not transferable themselves (as in browsers);
  their `readable` and `writable` are.
