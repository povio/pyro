# URLSearchParams

The URLSearchParams binding stores ordered name/value pairs in Rust
and uses the existing url crate's form-url-encoded parser and serializer. It
supports string, iterable-pair and record construction; size; append, set,
delete, has, get, getAll, sort and toString; live entries/keys/values iteration;
and forEach. Duplicate names retain order, set retains the first matching
position, optional value arguments filter delete/has, and sort is stable by
UTF-16 code units. USVString conversion replaces lone surrogates. Argument
conversion and user callbacks run outside native RefCell borrows.

Iterators hold the pair store without retaining a PageDom or a JS self-root.
They read one current entry at a time, remain finished once exhausted, and use
native receiver branding across worlds. Per-realm conversion/result helpers
are released through lifecycle::realm_disposed and PageDom.dispose. There is no
polling, whole-list iterator snapshot or claimed CPU/memory benchmark result.

Fetch and XHR serialize a URLSearchParams body natively, snapshot it at send,
and supply `application/x-www-form-urlencoded;charset=UTF-8` when no author
Content-Type exists. Overriding the object's toString does not alter BodyInit
serialization. Later mutations do not change an in-flight request body.

The [URL binding](URL.md) provides a live SameObject searchParams association.
[FormData](FORM_DATA.md) and the [Response constructor](FETCH.md) are bound separately. The original missing
URLSearchParams exception is preserved in the [pinned TOS identity run](../validation/TOS_IDENTITY_SMOKE.md);
the [next pinned run](../validation/TOS_QUERY_PARAMS_SMOKE.md) passed URL assertions
but exposed a missing URL constructor. Neither run establishes TOS compatibility.
The API targets the [URLSearchParams algorithms](https://url.spec.whatwg.org/#interface-urlsearchparams),
but focused tests are not a full WPT conformance claim.

Tests cover malformed percent input, duplicate/empty names, iterable and record
conversion, conversion failures, lone surrogates, stable UTF-16 sorting,
reentrant mutation, live iteration/forEach, cross-world method branding, GC, and
real local HTTP fetch/XHR body bytes, content type and send-time snapshots.
