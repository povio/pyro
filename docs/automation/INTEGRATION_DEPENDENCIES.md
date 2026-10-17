# Protocol integration dependency inventory

This follow-up is based on main `aeac8cff2f88575d652e2943aaa3edc7a7fa7167` (PRs 1–3 merged).
The original tested integration branch remains at `f27a3e72d99fe2fd53f7eb8d9305f8b1395dc6fe`.
The follow-up includes main in its ancestry so the PR diff has a single, current merge base.

## Previously published browser/CLI dependency

Network and Fetch need the existing HTTP loader and per-context network client. These files are carried
verbatim from f27a3e7; this follow-up does not implement new browser/realm WebAPI registration.

| Published commit | Dependency supplied |
| --- | --- |
| `7c3c872c346df79a332e07bc903382cf8955440e` | HTTP/data document loading, HTML script ordering, loading tests |
| `be9bf0237a2858974e7cb061cd23b8d342f605a2` | Browser contexts with separate NetClient instances |
| `de4b5b6c9d54894f6f310fcaa3449adc082eda7b` | Existing Location/History/Performance and session navigation state |
| `8d2f66c8c367fce1151eacdbe57f52897e77c330` | Loader/navigation API cleanup |
| `4e9432a66bd5def6045a2af6e1295415de2c5747` | Existing CLI fetch path over the loader |

Dependency diff: `crates/lp-browser/{Cargo.toml,src/lib.rs,src/loader.rs,src/nav.rs,src/window.rs,tests/loading.rs}`,
`crates/lightningpanda/{Cargo.toml,src/main.rs}`, their workspace dependency edges, and the previously published
compatibility/optimization documentation plus module-metadata regression in `crates/lp-js/tests/runtime.rs`.
The finalizer repair is already in main as f90494e (equivalent to integration cba2671); this PR does not reapply it.
All external Cargo.lock versions/checksums are preserved. The base merge removed one duplicate identical
ryu-js package block caused by ordering differences.

## Owned protocol and benchmark changes

- Network events and response capture use the loader's native request IDs and per-context clients. Logical
  payload budgets, bounded subscriptions and coalesced wakeups prevent unlimited protocol buffering.
- Fetch request-stage continuation, fulfillment, failure and Basic authentication use native decisions.
  Disabling/detaching resolves pending decisions; terminal events release IDs after pattern changes.
- Redirects get distinct Fetch pause IDs linked to their previous hop; auth retries retain the hop ID.
  Binary fulfillment headers accept NUL-separated byte pairs and preserve duplicates/native bytes.
- Browser CDP sessions support explicit child page attachment, scoped replies/events and cascading detach.
  Browser and page sessions share the connection cap; manual sessions survive disabling auto-attach.
- Storage get/set/clear accesses isolated cookie jars, with native validation and CDP expiry normalization.
- Same-host CPU/memory tooling and limited HTTP measurements are retained in `bench/results/http-cpu-d673567`.
  Those measurements identify rewrite **d673567**, not this follow-up head. They are not TOS measurements.

DOM/net/WebAPI owner head d2fa35a (including cookie repair 3557e95) is already in main through PR3 and is not
reintroduced as a source diff. Browser DOM/WebAPI registration, full TOS flows, TLS, response-stage Fetch,
and custom response phrases remain gaps. Static maintainability review and executed tests have distinct
scopes; neither is presented as the blocked independent correctness/performance review.
