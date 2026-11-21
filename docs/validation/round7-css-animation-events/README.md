# CSS animation and transition event elapsed time

Frozen parent `305840f45afbc4f55bfa24f303c1e858d30e4c1d` is the direct parent of source `d8748c1e3861cad878e026c98e22d6233a16e602`. The pinned CSS Animations and CSS Transitions IDLs declare `elapsedTime` as `double`, not `unrestricted double`. Both constructors now coerce once, retain finite values (including negative times), and throw TypeError for non-finite results. Native event production is unchanged.

Ranked from archived round-six `difference-cssrootcycle-main-stable` named Chrome-only passes, by confidence in one shared animation-only cause:

1. Event interface constructors: 12 on two pages, the same six invalid inputs on each, one shared conversion. Selected.
2. Ready-promise pages: 7 on two pages, spanning cancellation and pause/resume; no evidence one change fixes both.
3. Effect setKeyframes pages: 8 on two pages, spanning timing overrides, exceptions and transition reversal. Deferred.

All six URLs are present in the archived 8,082-URL main cohort and disjoint from the 39 withheld URLs. Only these six were executed. Three independently port-allocated `wptserve --testdriver` testharness runs used four workers each, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, and the frozen runner. Both LP builds used `wptiter --features pyro/render`, four build jobs and patched JSC `0e0dd7038a7a-6f4649e395e0`.

| Engine | Build | WPT wall | Strict pages | Named passes | Observed names |
|---|---:|---:|---:|---:|---:|
| Parent | 224 s | 396 ms | 4/6 | 84 | 96 |
| Candidate | 22 s | 404 ms | 6/6 | 96 | 96 |
| Chrome 158.0.8127.37 | — | 741 ms | 6/6 | 96 | 96 |

Gained `animationevent-interface.html` and `transitionevent-interface.html`, six names per page: the `elapsedTime` rejection of NaN, ±Infinity, `'sample'`, a multi-element array and a plain object. Lost zero pages or names. Both pseudo-element controls, transition events-001 and transition-end-event-shorthands remained strict passes. No name-set changes, restarts or watchdog kills; candidate and Chrome pass sets are identical. No full-cohort result is inferred.

Sibling compressed files contain raw runner outputs (including URLs, harness status and each named result). Binary SHA-256: parent `aa936d79f2131c61f6c34e9cdaedf7d35344decff98b7751a63307ea715e20cc`; candidate `9569511b80b69fcadde2dd1be7103b93e61eec1e8541e50e3cf1a5da740f4cfe`.
