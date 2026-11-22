# HTML Origin values

The frozen ordinary selector at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 includes all 23 eligible html/browsers/origin/api variants and 31 neighboring URL origin/IDL and web messaging guards. Its SHA-256 is b0cf3fc3513f8cd542cb45935cb172ce1930cdf65fbfba5bbf09c6027a73ba1b. The large census identified 314 signed-only target names: 292 attributed to direct missing-interface diagnostics, 18 missing after page setup and four with a different or partial diagnostic. These are observations from c3372ad21; separate base and feature ledgers measure the actual implementation delta.

The exact 414fc7481 parent executable (SHA-256 cf802080039b40071484fc8ae2620bb84e4378e43d23705f892897222ce60a9c) was built from its archived tree fe96416f7e81b73f18684e930d45fe814bcc055f. The signed Chrome stable 158.0.8127.37 WPT projection reuses 52 original four-job shards with 54 observed pages; Chrome did not rerun these WPT pages for this study. Direct Chrome controls use the same signed executable independently.

LP baseline had zero passes in 323 observable Origin names (19 completed pages, two errors and two timeouts). Signed Chrome passed 314/341 and 20 strict pages across 23 completed pages. All 31 guard pages completed, with 1,026/1,029 and 29 strict for LP versus 1,010/1,029 and 28 strict for Chrome. No LP parent restart or watchdog kill was reported. All original harness errors, missing names and partial observations remain distinct from named failures. [HTML Origin interface](https://html.spec.whatwg.org/multipage/browsers.html#the-origin-interface)

The value representation compares URL tuple components, using registrable domains for schemeful site and retaining ports for same-origin equality. Fresh opaque origins remain distinct and copies retain their private identity. Only a primitive string or supported branded browser object has extraction semantics; arbitrary objects are not stringified and author-defined properties are not used as origin evidence. Origin.from on a global uses exact identity and the existing WindowProxy access decision. A posted event retains private sender identity captured on dispatch, separately from publicly exposed serialized origin text; constructed events do not acquire it from their init dictionaries.

## Commands

Use the sibling cache layout in run.py and signed-reference.py. Build each exact tree with the unchanged CI JSC archive selected by scripts/jsc/cache-key.sh and Cargo wptiter profile (inherits release-fast, LTO disabled, 16 codegen units, incremental, render enabled). Place the immutable baseline at base-pyro and the candidate at pyro.

    python3 run.py base
    node primary-initial.cjs base ./base-pyro
    node probe.cjs chrome158
    node blob-links-probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node blob-links-probe.cjs candidate ./pyro
    python3 report.py
    python3 ledgers.py
    python3 changed.py
    python3 check.py
    ./native.sh

Occurrence rows in named.jsonl.gz retain URL, original subtest name, one-based occurrence and the statuses of base, candidate and signed reference. Missing names are null, distinct from observed Fail, Timeout and Not Run. pages.json reports lifecycle/harness state and strict-pass flag. Strict pages require completion under an OK harness with at least one named subtest and every name passing.

## Matched outcome

| Corpus | LP base | LP feature | Signed Chrome |
|---|---:|---:|---:|
| Origin values, 23 URLs | 0/323 | 316/341 | 314/341 |
| URL and messaging guards, 31 URLs | 1,026/1,029 | 1,026/1,029 | 1,010/1,029 |
| Strict Origin pages | 0 | 22 | 20 |
| Strict guard pages | 29 | 29 | 28 |

All 23 Origin pages became complete with OK harness status. The two prior setup errors and two prior timeouts recovered. There were 298 gains on previously observed names and 18 newly observable passing names; zero previous passing names or observations were lost. Of the 316 gains 308 pass with an identical name in Chrome, two tentative file-scheme opaque-origin results fail in Chrome, and six LP function-string-generated names are unobserved under those exact strings in Chrome. The corresponding six Chrome-generated names are separately absent under LP. The originally defined invalid-input operations succeed in rejecting their inputs on both engines. generated-function-names.json preserves those strings by source-list position; none were renamed to improve the score. The only remaining LP target failures are 25 MathML-anchor expando cases also failing signed Chrome. Objects with an author-defined href do not thereby implement a hyperlink interface.

The candidate and signed reference match the complete ordinary primary direct fixture, including unique opaque origins, copy equality, PSL-domain/scheme/port comparisons, genuine URL record and hyperlink identity, same-origin popup extraction, a rejected author-created proxy with zero trap calls, received message provenance and dedicated worker globals. The supplemental fixture also checks revocation in a sandboxed document controlled by the test. Before revocation, LP reports [false,false,true] for the registered blob URL as a string, native URL object and hyperlink respectively; signed Chrome reports [true,true,true]. After revocation LP reports [false,false,true,true] for string, URL object and two already-associated hyperlinks, while Chrome reports [false,false,false,false]. Re-parsing the URL object after revocation reports false in both.

HTML Origin.from(string) and the URL API parser invoke the basic URL parser, which does not attach a blob URL entry. The hyperlink's encoding-aware URL parser invokes the full URL parser and retains its parsed URL record. The implementation captures available non-recoverable creator identities on ordinary hyperlink assignment, parsing and cloning; revoking the object URL prevents new records from capturing that identity. Creator registrations currently share the existing DOM's blob store. Cross-arena blob stores and bytes fetched across arenas have no new implementation in this change. File URL origin handling is implementation-defined under the URL Standard; LP uses a fresh opaque origin. [HTML Origin.from](https://html.spec.whatwg.org/multipage/browsers.html#dom-origin-from) · [URL parsing and origins](https://url.spec.whatwg.org/#concept-url-origin) · [HTML hyperlink processing](https://html.spec.whatwg.org/multipage/links.html#concept-hyperlink-url)

Independent source review cleared 490fa87a2, including the one-line correction to the optional parsed service-worker client source after compiling the 8c5ceb683 candidate. Independent runtime QA repeated all 54 URLs and both direct fixtures against the unchanged final binary. Every named and page result in both groups, and the entire primary and supplemental direct payloads, matched the implementation run exactly. The nine native guards passed, including genuine URL/DOM identity, ordinary window/opener messaging, SSE source origin, URL scheme/private suffix comparisons, and sparse hyperlink record cloning/revocation and namespace precedence. See qa/ for independent receipt hashes and native-results.txt for the test names.
