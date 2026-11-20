# Mathematical italics in rendered text

Painting, innerText and Selection now share the mathematical italic character mapping. It applies to a single-character text node, not to a one-character selection inside a longer node. The raw DOM Range stringifier and textContent retain the source. A real-JSC integration test covers inherited style, the special h and nabla mappings, a partial selection and DOM readback.

Same pinned Studio JSC, WPT and Chrome as `font-feature-values.md`. Six jobs. In the 379-URL CSS text/selection/innerText focus, parent (`c0511a1e`) 337 strict pages / 5,200 passing names, candidate 338/5,313, Chrome 320/5,089. All observed 5,819 names. Gains: the 112 character mappings on `text-transform-math-auto-003.html` and the `math-auto` computed-value name. No lost pages or names.

| Main: 8,082 URLs | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,418 | 175,077 | 194,305 |
| Candidate | 5,419 | 175,190 | 194,305 |
| Chrome stable 158.0.8127.37 | 6,950 | 182,963 | 194,912 |

Full candidate/parent exclusives: 1/0 strict pages and 113/0 names. Candidate/Chrome exclusives: 246/1,777 strict pages, 5,568/13,341 passing names and 312/919 observed names. Parent and candidate both recorded 7,864/101/105 complete with status 0/1/2; 11 timeouts and one hang. Chrome: 7,986/23/62, ten timeouts and one hang. Outside main: 41 supplemental (prior LP 27/436, Chrome 35/619), 39 crash/security exclusions. H2/testdriver limitations unchanged. No expectation or inventory changes.
