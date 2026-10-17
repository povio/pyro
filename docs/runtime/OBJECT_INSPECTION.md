# Native object inspection

`Value::has_proxy_target() -> JsResult<bool>` queries the optional
`JSObjectGetProxyTarget` SPI without invoking getters or proxy traps. It returns
false for primitives, true when JSC exposes a target, and `Unsupported` for
objects if the symbol is absent. Global proxies are included. A revoked proxy
may retain its target (observed on W1's system JSC); callers must not interpret
false as evidence that a value is cloneable. A clone implementation can reject
true results and separately reject the exception from a captured `Array.isArray`
when checking a revoked proxy. The API never exposes or retains the target.

`Scope::is_host_object(Value) -> JsResult<bool>` identifies this engine's native
wrappers/closures using their native JSClass ancestry, even after JavaScript
changes their prototype. It also identifies same-VM globals, including a global
kept reachable after its Rust realm has been disposed. Global identity uses the
optional `JSObjectGetGlobalContext` SPI. Non-wrapper objects return `Unsupported`
when that SPI is absent; primitives return false. Values from another engine
return a type error before class inspection. No property lookup or page code is
involved, no payload is dereferenced, and the calls retain no additional roots.

Availability is exposed through `Capabilities::proxy_targets` and
`Capabilities::object_global_context`. Both SPI signatures come from WebKit's
[JSObjectRefPrivate.h](https://github.com/WebKit/WebKit/blob/main/Source/JavaScriptCore/API/JSObjectRefPrivate.h);
native behavior is implemented in
[JSObjectRef.cpp](https://github.com/WebKit/WebKit/blob/main/Source/JavaScriptCore/API/JSObjectRef.cpp).
Symbols are resolved through the existing optional `dlsym` table, not linked
unconditionally or inferred from object memory layouts.

These predicates are rejection tools, not structured-clone serialization or a
complete exotic-object classifier. In particular, ordinary-object checks based
on JavaScript prototypes or `Symbol.toStringTag` do not become sound for altered
Promise/iterator/generator objects merely by using these APIs. Clone semantics,
transfer behavior, supported types, and their limits belong to the binding.

The real-JSC tests cover hostile handlers and tag getters, revoked proxies,
primitives/functions, changed wrapper prototypes, native closures, cross-realm
globals retained through disposal and GC, and cross-engine rejection.
