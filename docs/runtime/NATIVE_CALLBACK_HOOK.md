# Native callback scheduling hook

`Realm::set_native_callback_hook(Option<NativeCallbackHook>)` installs an
owner-thread hook after a native callback body returns and before JavaScript
continues. `NativeCallbackHook` is `Rc<dyn for<'s> Fn(&Scope<'s>)>`. Installation
fails for a disposed realm/engine; None uninstalls it and realm disposal releases
its captures.

The hook can inspect pending native mutations and enqueue a microtask in that
realm. Callback-local borrows have ended. Outer native callbacks can still hold
borrows during native-to-JS-to-native reentrancy, so embedder inspection must use
safe borrowing and defer if unavailable. Ordinary nested callbacks each reach
this boundary: limiting delivery to the outermost native call would order a
mutation behind Promise reactions created later inside reentrant JavaScript.
Callbacks invoked by the hook itself suppress the hook in the same realm.

The runtime protects a successful raw JS return value across hook allocations or
GC. Thrown JS values already own a Persistent root; other errors materialize
after the hook. Native termination skips scheduling, and termination raised by
hook work still propagates. Hook panics are caught by the existing native error
boundary and reset the recursion guard. Hooks should capture PageDom weakly,
queue work rather than execute observer callbacks, and must not dispose the
realm while a native call is active.

This is a scheduling primitive, not MutationObserver or a generic microtask
checkpoint. Parser/native mutations outside JS callbacks still need the browser
safe point. Tests cover Promise ordering, reentrant nesting, released body
borrows, recursive scheduling suppression, GC of returned/thrown wrappers,
64/64 payload cleanup, panic recovery, disposal and termination.
