# Unchanged office cases on percentage-width repair

All four canonical common cases in `master-data/business-entities/office.e2e.ts` passed exactly once, with no skipped/retried cases, on engine **bf4fc5605a31633940ae52acf82d90cea952bdbe** and TOS **6603beaddd4aaee1e20f04bfa769b208763658a6**. The office file, shared business-entity helpers, seed, auth fixture, and manifest are byte-identical to frozen e6cf6953. No diagnostic logging or assertion changes are present. Other Chromium-default office cases are outside this common selection.

Exact locked-release build on macOS26.7.1/M1, SDK26.5, rustc1.99.1; source checkout clean. Immutable binary SHA **7e593218ebf1af89c7f3d4e4adffffdcfb2cfa0e2309d450158026934ef1a8ab**, repository linker signing verified strictly with allow-jit=true. Engine candidate includes its published pointer-fix dependency a182fc1. This is targeted compatibility evidence, not a same-production-except-one-line performance comparison.

Fresh isolated run/template databases used the existing authorized local Docker PostgreSQL17 service, backend-token localStorage auth and direct-lane version-gate bypass. Per-run source hashes remained fixed; the pre/post database sets matched and no new run database remained. The existing retained baseline template was preserved.

The original frozen badge failure and paired measurement diagnostics remain unchanged in the sibling office-widths directory. Native generic percentage resolution changes the container sizing and allows the original labels to render. W2's reduced probe still reports approximate intrinsic badge widths and gap; this result does not claim flex/font/full-CSS parity or support for percentage heights and automatic containing-block widths.

The immutable full-suite result remains729/742. Final combined-candidate742 validation is separate and pending.
