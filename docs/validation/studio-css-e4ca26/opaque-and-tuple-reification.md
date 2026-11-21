# Typed OM composite values on Studio

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Chrome stable `158.0.8127.37`; LP `0.0.17` with JSC `0e0dd7038a7a-6f4649e395e0`. The main run uses the existing 8,082 matched URLs, six workers and wptserve with testdriver. The 287 StylePropertyMap URLs are a subset, not additional coverage. The 41 supplemental URLs use worker globals. The other 39 URLs remain withheld and receive no credit.

`9f1688bf` preserves a composite value as one property-associated CSSStyleValue. This avoids inferring a keyword or length from abbreviated serialization and avoids truncating comma lists. CSS-wide keywords, variable references, `text-shadow: none`, and `will-change: auto` keep their specialized representations. Five native tests passed. The `grid` page still fails the repeat-track/auto-flow case (missing value rather than wrong Typed OM subclass).

Against `bc801680`, the direct focus and matched main each gained 15 strict pages and 22 names, all also passing Chrome; zero pages, names or observations lost. Main: LP 5,643 strict / 180,702 names, Chrome 6,950 / 182,963. Chrome-only 1,581 / 8,136; LP-only 274 / 5,875. `at-media-matches-002` changed harness status 2 to 0 without converting the page. Supplemental unchanged: LP 27 / 577; Chrome 35 / 619. Exact URL and named differences are in the `difference-opaqueom-*` artifacts.
