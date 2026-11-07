# structuredClone

The binding uses the runtime [object inspection predicates](../runtime/OBJECT_INSPECTION.md).
Its initial supported subset clones primitive values, ordinary object/array graphs,
Date, RegExp, Map, Set, boxed primitive values, ArrayBuffer and typed-array/DataView
views. It preserves cycles, shared references, sparse arrays and shared cloned
buffer backing. Enumerable own string properties are read in order; getters run
once, exceptions propagate, and properties are recreated as writable data
properties. Symbols and non-enumerable properties are omitted. RegExp.lastIndex
resets. Construction uses captured destination-realm intrinsics.

Error objects clone as HTML serializes them, with Chrome's extras: the
constructor is the one their `name` names when that is Error, EvalError,
RangeError, ReferenceError, SyntaxError, TypeError or URIError (otherwise
Error); the own data `message` (converted to a string), the own data `cause`
(cloned, cycles included) and the own `stack` string are kept as
non-enumerable properties; other properties are not copied. A DOMException
clones as a DOMException with its name and message (and `stack`); an object
that only inherits from DOMException.prototype is rejected.

Serializable host objects are cloned natively: a Blob or File becomes a new
wrapper of the base class (a subclass instance loses its subclass) over the
same immutable bytes, with its type, name and lastModified; a FileList becomes
a new FileList of such Files. Other host wrappers (nodes, Response, ...)
throw DataCloneError.

Platform objects the script does not recognize go to one native hook before
they are rejected: `guard(value, true)` (`crypto::clone_platform_object`)
returns their copy or undefined. It copies a `CryptoKey` of the realm (see
[Web Crypto](CRYPTO.md)) and an `ImageBitmap` (a new one over the same
pixels; DataCloneError when it is closed or not origin-clean); it runs only
on that rejection path, so ordinary graphs pay nothing. The cross-VM format carries CryptoKeys too (see
[Web Crypto](CRYPTO.md)).

Transfer lists (`{transfer}`) take ArrayBuffers, MessagePorts,
ReadableStream, WritableStream and TransformStream, ImageBitmaps and
OffscreenCanvases. A transferred ImageBitmap becomes a new one over the same
pixels and the original is closed (width and height 0); a transferred
OffscreenCanvas becomes a new one of its size (and placeholder `<canvas>`, if
any) and the original is detached: its size reads 0 and `getContext`, the
size setters, `transferToImageBitmap`, `convertToBlob` and drawing it throw
InvalidStateError. An OffscreenCanvas that has a context throws
InvalidStateError, a detached one DataCloneError, and an OffscreenCanvas
outside the transfer list DataCloneError (it is transferable only). Entries are validated
first: a duplicate, a detached ArrayBuffer, an already transferred port, a
locked stream or any other object throws DataCloneError, a non-object
TypeError. Nothing is detached unless the whole value clones. A transferred ArrayBuffer is copied
into the clone and then detached with `ArrayBuffer.prototype.transfer` (the
bytes are copied, not moved); a resizable one stays resizable. A transferred
MessagePort is replaced by a new port in the destination realm that takes over
its entanglement and undelivered messages, and the original is detached
([MESSAGE_CHANNEL](MESSAGE_CHANNEL.md)). A length-tracking typed array or
DataView becomes a fixed-length view.

A transferred stream is the Streams standard's cross-realm transform: the
validation step makes, in the destination realm, the new stream over new
unentangled ports (no side effect on the original); once the whole value has
cloned, the original is piped into (a ReadableStream) or from (a
WritableStream) a new MessageChannel of its realm whose other port moves into
the new stream's (`streams.js` `transferKind`/`transferIn`/`transferOut`). A
TransformStream transfers its two sides. Chunks are structured-cloned through
the ports with the standard's `pull` backpressure, errors and cancellation
travel both ways, and a chunk that cannot be cloned errors both streams.
Across realms (`clone_into`'s second pass), the poster realm's streams module
handles the first copy's streams (`CLONE_SOURCE`), so a stream posted to
another frame crosses two channels.

Functions, symbol values, proxies, other native host wrappers/globals, shared
buffers and other normally tagged unsupported types are rejected. Custom
Symbol.toStringTag properties/accessors are outside this subset and are
rejected without invoking their getters. The native proxy/host checks require
capability-gated JSC SPI and fail explicitly if unavailable.

Users: `structuredClone`, MessagePort and `window.postMessage` (cloned straight
into the receiving realm, which may be another frame or page of the thread's
VM). The cross-VM format (`worker_wire.js`: dedicated and service workers
and their clients, MessagePorts entangled across threads, popups,
BroadcastChannel, IndexedDB and history state) encodes
the same types, Error, DOMException, Blob, File, FileList, CryptoKey and
ImageBitmap included; Blob bytes and bitmap pixels travel beside the text,
shared rather than copied. Its transfer list also takes ImageBitmaps and
OffscreenCanvases (the canvas's size and placeholder travel beside the
text). Its
transfer list takes ArrayBuffers (copied, then detached), MessagePorts and
streams: transferred ports travel beside the text as sides of a thread-safe
pipe (`port_link.rs`, see [MESSAGE_CHANNEL](MESSAGE_CHANNEL.md)), listed ports
first, then each stream's ports, and only the listed ones become the event's
`ports`. Every transferable is checked again after the value is walked
(getters may have detached a buffer, transferred a port or locked a
stream) and before any is consumed, so a post that throws leaves its
transfer list as it was. A message's ports are claimed by its first
deserialization; a message dropped before that closes them. BroadcastChannel, IndexedDB and
history state transfer nothing, so a port or stream in their values throws
DataCloneError.

A remaining conformance limitation is significant: JSC's available C API does
not provide a complete exotic-object classifier. An exotic object deliberately
changed to appear as an ordinary record (for example a Promise with a replaced
prototype/tag) cannot always be distinguished. This implementation does not claim
correct structured serialization for such altered exotic objects. It is not a
complete implementation of the HTML structured serialization algorithms.

Cloning has no polling or document scan. Graph nodes, collection entries and
properties per object are limited to 65,536; nesting/prototype depth to 512;
copied ArrayBuffer bytes to 64 MiB per call. Exceeding these limits throws
NotSupportedError. These limits do not bound all transient JS allocation or RSS.
Temporary roots live in the JS call graph; installed closures capture intrinsics
and stateless native predicates, with no PageDom or persistent native self-root.

The motivating unchanged TOS profile failure is recorded in
[TOS URL smoke](../validation/TOS_URL_SMOKE.md). That run remains failed evidence. A [new unchanged pinned run](../validation/TOS_CLONE_SMOKE.md)
passes both profile/settings scenarios; this does not establish full TOS compatibility.

Semantics are compared with the [HTML structured serialization algorithms](https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal), subject to the explicit limits above.
