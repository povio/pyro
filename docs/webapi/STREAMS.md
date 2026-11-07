# Streams

`crates/lp-webapi/src/streams.js` implements the WHATWG Streams classes;
`streams.rs` makes them on a realm's first use and connects them to fetch and
Blob. A realm that never touches streams only has one global accessor pair per
interface name; the first read of a name replaces its accessor with the usual
data property, and assigning a name replaces it too.

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
- There is no `Request` class, so no `Request.body`. `CompressionStream` has no
  `"brotli"`.
- Transfer (`structuredClone(s, {transfer: [s]})`, `postMessage`): the new
  stream reads the transferred one, which the transfer locks; to or from a
  worker it is pumped through a cross-thread MessagePort (chunks are pushed and
  serialized, with no backpressure beyond the port's 64 MiB queue). A locked
  stream throws DataCloneError.
