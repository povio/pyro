# Typed registered initials and container units

`02f8daf8` compared with `02846648`, Studio Chrome stable 158.0.8127.37 and WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Same LP/JSC configuration and six-worker harness as the page descriptor checkpoint. In the 127 focused URLs and the 8,082 main URLs, `container-units-computational-independence.html` gained all six unit assertions and became a strict pass. No strict pages, named passes, or observed names lost; no per-URL harness-status changes. The native registration test also passed, including the nested calc rejection and viewport-unit acceptance controls. Stylo vendor patch reconstruction succeeded.

The 41 supplemental URLs stayed at 27 strict pages and 577 names, vs Chrome 35 and 619. The 39 withheld URLs remain outside both sets. See the `cqunit` outcome and difference artifacts for exact membership.
