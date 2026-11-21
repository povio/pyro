# Computed style across document boundaries

`getComputedStyle` used the Window receiver's layout for an element from a different document. On the matchedsize parent the 24 iframe viewport-unit registrations read back empty. The candidate uses the element's document for live declarations and checks that embedding frames are rendered. The first owner-only experiment fixed the viewport tests but lost the parent-window hidden-iframe assertion. The final candidate recovered it and gained the corresponding test from the iframe's own window; no test expectation changed.

Against `matchedsize`, 57 focused URLs gained three strict pages and 27 names. The complete matched run gained nine strict pages and 43 names, with no lost pages, names, or observed names. The supplemental 41 URLs were unchanged (27 pages and 577 names vs Chrome's 35 and 619); the other 39 remain withheld, not passed. On the main 8,082 URLs LP has 5,711 strict pages and 181,440 named passes; Chrome has 6,950 and 182,963. Chrome-only: 1,517 pages and 7,422 names. LP-only: 278 pages and 5,899 names. LP recorded 11 timeouts and one hang and Chrome 10 timeouts and one hang.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, JSC `0e0dd7038a7a-6f4649e395e0`. See the adjacent `renderedstyle` artifacts for exact per-URL outcomes and directional sets.
