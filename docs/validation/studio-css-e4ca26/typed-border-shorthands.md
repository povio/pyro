# Typed border shorthand assignment

Studio, implementation `d591fe9b`, parent `d611df67`; WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. A typed length represents a border width, not an entire composite border. String shorthand assignment and typed width longhands remain valid. Four native Typed OM integration tests passed.

On 287 matched StylePropertyMap URLs, LP went from 11,170 to 11,188 passing names, with 181 strict pages unchanged. Chrome passed 11,115 names and 219 strict pages. All 18 gained names were also Chrome passes; all were on `logical.html`.

The 8,082-URL main run confirmed the exact same gains, with zero lost pages, names, or observations: 5,628 strict pages, 180,680 passing names and 194,655 observed names. Chrome-only: 1,596 pages / 8,158 names; LP-only: 274 / 5,875. One nonpassing page, `css/css-conditional/at-media-matches-002.html`, changed from harness status 0 to 2. On two six-worker repeats each, both binaries finished it with status 0 and the same two failed assertions. The original timeout remains in the recorded results.

All 41 supplemental URLs completed. LP remained at 27 strict pages and 577 passing names, Chrome at 35 and 619; both observed 874 names. The 39 withheld URLs receive no credit. No expectations were edited. The separate declarative easing experiment changed none of the 63 focused URLs' pass sets and is not included.
