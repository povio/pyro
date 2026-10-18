# FileReader

Native `FileReader` reads the immutable in-memory `Blob` and `File` byte store.
It provides `readAsArrayBuffer`, `readAsText`, `readAsDataURL`, and the legacy
`readAsBinaryString`, along with `EMPTY`/`LOADING`/`DONE`, stable `result` and
`error`, `abort`, and the six progress-event handlers.

Reads require an embedder with queued `PageHooks::post_task`; without that
service they throw `NotSupportedError` before changing state. Loadstart,
progress, and completion run as separate owner tasks. There is one progress
notification for this memory-backed read, not simulated streaming or polling.
The result is null until completion. Success fires load then loadend; a read
failure fires error then loadend. Abort while loading cancels remaining tasks,
clears the result and fires abort then loadend synchronously. Starting a new
read in a terminal listener suppresses the old read's loadend. Repeated reads
while loading throw `InvalidStateError` without replacing the pending read.

Text decoding accepts Encoding Standard labels, otherwise the Blob MIME
charset parameter, then UTF-8; BOM detection takes precedence and malformed
input is replaced. MIME charset extraction supports ordinary semicolon-separated
parameters with optional quoted values, not the full MIME parsing grammar.
Data URLs use the Blob type or `application/octet-stream`, with base64 data.
Binary strings map bytes directly to U+0000–U+00FF.

`PageDom::set_file_reader_limits` configures the per-read and aggregate pending
input limits (both 64 MiB by default) and concurrent-read limit (128). Byte-limit
failures deliver `NotReadableError` asynchronously without retaining the Blob;
exceeding the concurrent-operation limit throws `QuotaExceededError` before
starting. `file_reader_usage` reports pending count and logical input bytes.
Limits cannot change during a pending read. This bounds pending inputs and
queued work, not author-retained results or the Blob store itself. Encoded text
and base64 results can be larger than their inputs.

The reader keeps Blob/result/error edges in private traced wrapper properties.
Only active operations pin the reader; completion and abort release the Blob
edge and operation pin. Back-references from Blobs, results or listeners are
collectable. At most one owner callback is posted per document, including
repeated read/abort cycles; queued phases carry IDs, not buffers or roots.
`PageDom::dispose` and `lifecycle::realm_disposed` cancel pending operations,
release their roots/bytes and suppress callbacks. Embedders must call these
existing teardown APIs before discarding their task queues or realms.

There is no filesystem-backed Blob reader, disk-error simulation,
`FileReaderSync`, worker support, or stream API. This increment addresses the
actual JSZip 3.10.1 Blob conversion used by docx-preview 0.3.7; XML parsing and
same-window `MessageEvent.source` remain separate dependencies. Package and
unchanged TOS validation must identify the exact combined candidate.
