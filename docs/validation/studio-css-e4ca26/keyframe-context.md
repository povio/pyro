# Keyframe declaration context

Source `35b761a5`, parent executable `importurl` (`610b47ea`); LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`. Six jobs, wptserve and testdriver. Keyframe rule parsing, appending, cssText replacement and property mutation use the rule's property restrictions. The initial focused attempt incorrectly accepted animation-duration; correcting the property metadata fixed the remaining failure. The new JS integration test passed and the Stylo patch round-tripped against the parent.

On the 8,082 main URLs LP improved from 5,730 to 5,732 strict pages and 181,545 to 181,548 named passes. Both keyframes-allowed-properties and CSSOM rule-restrictions now pass strictly; declaration-context also gained its nested @scope assertion. No former strict pages, named passes, or observed names were lost. Chrome: 6,950 pages and 182,963 names; 1,499 Chrome-only pages and 7,316 names, 281 LP-only pages and 5,901 names. LP: 11 timeouts, one hang; Chrome: ten timeouts, one hang. Focus is a 53-URL subset of main.

Separate 41-URL worker supplement unchanged: LP 27 pages / 577 names versus Chrome 35 / 619. The other 39 URLs remain withheld, not passing. The keyframe-prefixed artifacts contain directional differences for all three cohorts and compact per-assertion outcomes for main and supplement.
