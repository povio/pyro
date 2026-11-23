# Intrinsic block-height transition readback

Mac Studio executed the exact MacBook-built development candidate `0b6f8dde54cb4dd082565a76ccda87f8803a0253` once. Archive SHA-256 `7e6affc6a99f29b58bf7da69cad7ac06944538adceb302090a956bbf509c2531`; Mach-O arm64 executable `c5006ad17d33f0de81121d997a3e52ddbf31cff103615746ebb87628dd57ab55`. Build provenance, compressed build log and the passing fixed-child native test receipt are included.

Reused original and direct parent arms from evidence commit `93a1fca2e72efeb60ad8e8ce7946b085fecb0fcc`, docs/validation/round7-calc-size-sampled/{baseline,candidate}.json.gz. No parent replay. WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; inventory identical across all three runs. Keyed by URL, complete name, and within-name occurrence ordinal: 3,303 matches, zero added/missing. All 13 harnesses completed OK. Candidate had zero restarts.

Original 2,665 → direct parent 2,697 → candidate 2,737 passes. No losses relative to either arm, 566 persistent failures. All 40 new passes were on physical height: 32 direct min-/max-content transition rows and eight nested min-content rows. All 40 match the retained signed Chrome 158.0.8127.37 reference; `comparison.json` also attributes each of the 32 preserved earlier gains. Reference: `docs/validation/studio-css-e4ca26/outcomes-stable.json.gz` at the tested commit.

For the direct endpoint pairs, all eight previously passing transition controls, 20 CSS Animation controls and 20 Web Animations controls remain passing. At 75% the direct intrinsic height reads 62.5px, and the nested min-content case reads 250px. Width and the other 11 pages show no status changes. Adjacent fit-content height at 75% remains 50px instead of 62.5px. This is not release or whole-suite validation.
