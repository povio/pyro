# IndexedDB transaction creation turn

Independent serial dev builds used one Cargo job. Source trees, executable and JSC digests, embedded IDB source checks, validation scripts and logs accompany the raw results. The parent native regression used the test-only transplant after the parent executable was archived.

The four frozen globals each passed 3/5 on the exact parent and 5/5 on the candidate, its repeat and archived signed Chrome 158. All harnesses completed OK. Eight named failures became passes. The candidate passed the transplanted microtask/listener/write/reopen regression and both existing IDB guards.

The shared pyro-js EntryGuard was exercised through IDB callers. This independent run includes no dedicated non-IDB entry/exit control. Behavior of unrelated callers is unverified here.
