# Percentage endpoint: Studio development QA

One verified `b99353ad0` candidate run; no parent or Chrome replay. All 3,303 occurrence keys matched the ten retained comparison arms and signed Chrome. All 13 harnesses completed OK, with zero browser restarts or watchdog kills.

Direct parent `c236cca7e` passed 3,009; candidate passed 3,017. All eight gains matched signed Chrome and were height transitions from `calc-size(auto, size)` to `50%` at −0.25, 0, 0.75 and 1.25, with both explicit-property and `transition: all` forms. At 0.75 both now return 137.5px. The other 3,295 statuses, including physical width, CSS Animations and Web Animations controls, did not change. The doubled-size variant did not gain. Original-to-candidate: +352/−0. Residual: 285 signed-only passes and one shared failure.

`compare.py` checks key equality and reproduces `comparison.json` from the compressed raw run and immutable prior receipts. The source vendor patch roundtrip was checked before freeze; the bundled MacBook native test passed exactly once. Archive SHA-256: `28f8e84379c3438313f531a926e7b243c8e68ed97da24cb0b189c4850f5227ee`; extracted executable: `64d73c6d91f3e2b994dcf82d59782091bafcb6bdcd7fd133a3302256ef833276`. Both and the internal checksum manifest were verified on Studio. Development only.
