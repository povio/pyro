# Geometry rectangles and quadrilaterals

Main: same 8,082 CSS URLs, WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, LP 0.0.17/JSC 0e0dd7038a7a-6f4649e395e0, Chrome stable 158.0.8127.37. Counts are strict passing pages / passing named assertions. Parent 5ae4a0ec: 5,444 / 175,796. Rect/quad: 5,452 / 175,971. Chrome: 6,950 / 182,963. Relative to parent: +8/-0 pages, +176/-1 passes, +25/-0 observed names. LP/Chrome exclusive: 248/1,746 pages and 5,575/12,567 passes. Candidate complete harness status 0/1/2: 7,866/99/105; 11 timeouts, one hang. No missing URLs.

Seven strict gains are geometry tests; the eighth is the previously intermittent constructed stylesheet import test. The single full-run lost name concerns transition start time at generation. It passed three of three repeats on each binary (19 neighboring URLs, six jobs); retain the loss in the full comparison. No assertion tolerances changed. On the matched 28 geometry URLs: parent 15/905, candidate 22/1,080, Chrome 26/1,196. The Rust integration suite passed all 632 tests.

Separately, the 41 supplemental URLs (four UTF-16, 37 worker) all completed: parent 27/436, candidate 27/519, Chrome with certificate handling 35/619; all observed 874 names. No losses. The 39 crash/security exclusions were not run, and H2/testdriver limitations remain. Exact full and supplemental differences are archived alongside this note.
