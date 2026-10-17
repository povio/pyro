# structuredClone

The binding uses the runtime [object inspection predicates](../runtime/OBJECT_INSPECTION.md).
Its initial supported subset clones primitive values, ordinary object/array graphs,
Date, RegExp, Map, Set, boxed primitive values, ArrayBuffer and typed-array/DataView
views. It preserves cycles, shared references, sparse arrays and shared cloned
buffer backing. Enumerable own string properties are read in order; getters run
once, exceptions propagate, and properties are recreated as writable data
properties. Symbols and non-enumerable properties are omitted. RegExp.lastIndex
resets. Construction uses captured destination-realm intrinsics.

Functions, symbol values, proxies, native host wrappers/globals, shared buffers,
Error objects and other normally tagged unsupported types are rejected. Nonempty
transfer lists throw NotSupportedError without detaching anything. Custom
Symbol.toStringTag properties/accessors are outside this initial subset and are
rejected without invoking their getters. The native proxy/host checks require
capability-gated JSC SPI and fail explicitly if unavailable.

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
[TOS URL smoke](../validation/TOS_URL_SMOKE.md). That run remains failed evidence;
passing focused tests does not imply the profile flow or full TOS passes.

Semantics are compared with the [HTML structured serialization algorithms](https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal), subject to the explicit limits above.
