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
[Web Crypto](CRYPTO.md)); it runs only on that rejection path, so ordinary
graphs pay nothing. Worker messages do not carry CryptoKeys.

Transfer lists (`{transfer}`) take ArrayBuffers, MessagePorts and
ReadableStreams (see [Streams](STREAMS.md)). Entries are
validated first: a duplicate, a detached ArrayBuffer, an already transferred
port or any other object throws DataCloneError, a non-object TypeError. Nothing
is detached unless the whole value clones. A transferred ArrayBuffer is copied
into the clone and then detached with `ArrayBuffer.prototype.transfer` (the
bytes are copied, not moved); a resizable one stays resizable. A transferred
MessagePort is replaced by a new port in the destination realm that takes over
its entanglement and undelivered messages, and the original is detached
([MESSAGE_CHANNEL](MESSAGE_CHANNEL.md)). A length-tracking typed array or
DataView becomes a fixed-length view.

Functions, symbol values, proxies, other native host wrappers/globals, shared
buffers and other normally tagged unsupported types are rejected. Custom
Symbol.toStringTag properties/accessors are outside this subset and are
rejected without invoking their getters. The native proxy/host checks require
capability-gated JSC SPI and fail explicitly if unavailable.

Users: `structuredClone`, MessagePort and `window.postMessage` (cloned straight
into the receiving realm, which may be another frame or page of the thread's
VM). The cross-VM format (`worker_wire.js`: workers, BroadcastChannel and
history state) encodes the same types, Error, DOMException, Blob, File and
FileList included; Blob bytes travel beside the text, shared rather than
copied. MessagePorts and ReadableStreams in a worker message's transfer list
cross VMs as cross-thread links ([MESSAGE_CHANNEL](MESSAGE_CHANNEL.md)); the
message's ports are its event's `ports`, and a ['P'] or ['S'] node stands for a
port or stream in its data.

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
