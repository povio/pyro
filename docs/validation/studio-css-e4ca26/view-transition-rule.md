# View-transition descriptors in CSSOM

Source `13f975a9`, parent `00a456a8`. LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`, Studio Chrome stable `158.0.8127.37`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Main runs used the same 8,082 URLs, six jobs and testdriver. The 179 focused URLs are a subset of main, not additional coverage.

Both descriptor pages became strict passes: `navigation/at-rule-cssom.html` went from 0/9 to 9/9; `navigation/with-types/at-rule-with-types-parsing.html` went from 0/16 to 16/16. Chrome passed all 25. The parser now uses Stylo's descriptor grammar and canonical serialization; the CSSOM wrapper caches a frozen identifier array per wrapper. A binding test verifies identity, escaping, duplicates, and retention after removing the rule.

Main: LP 5,711 → 5,713 strict pages and 181,440 → 181,469 named passes; Chrome 6,950 and 182,963. No lost passes or observed names. Chrome alone passed 1,515 pages and 7,393 names; LP alone passed 278 and 5,899. LP recorded 11 timeouts and one hang; Chrome recorded 10 timeouts and one hang. The 41 supplemental URLs were unchanged (LP 27 strict / 577 names, Chrome 35 / 619); 39 withheld URLs remain unscored. Adjacent `vtrule` artifacts contain the exact directional sets and outcomes.
