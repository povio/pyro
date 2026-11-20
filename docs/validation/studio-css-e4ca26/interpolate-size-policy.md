# Inherited intrinsic-size interpolation policy on Studio

Parent: `6750f6f0` (binary from `1973f6a1`). Same WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, JSC `0e0dd7038a7a-6f4649e395e0`, six jobs, wptserve and local Chrome stable `158.0.8127.37` as the any-basis checkpoint. The inherited, non-animatable `interpolate-size` property defaults to `numeric-only`. With `allow-keywords`, compatible keyword/length pairs use the intrinsic calculation for width, height and their minimums. Different keywords do not become compatible. Transitions sample using the after-change policy; keyframe effects use the underlying style rather than a declaration inside a keyframe. Maximum sizes and mixing independent percentages with the intrinsic placeholder remain incomplete.

The same 40 URLs (calc-size interpolation, interpolate-size, and css-sizing animation controls) completed with OK harness status in all three builds. Of 7,123 observed names, parent passed 5,240, candidate 5,859, and stable 7,122. Strict pages: 18, 25, and 39 respectively. Candidate gains/losses versus parent: **+619/-0 named, +7/-0 strict**. No change in observed names. The first candidate lost eight endpoint assertions: CSSOM returned the computed `calc-size()` for inline-size/block-size instead of the used physical size. Mapping logical to physical axes using the computed writing mode before size readback recovered all eight; the totals here use that revision.

The 41 supplemental URLs and 39 security/crash exclusions remain outside main counts. The supplemental reference is unchanged (LP 27 strict/436 named; stable 35/619), not a fresh candidate run. H2 and testdriver environment limitations still apply.

## Main comparison

| All 8,082 URLs | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,374 | 172,629 | 194,311 |
| Candidate | 5,386 | 173,281 | 194,312 |
| Chrome stable | 6,950 | 182,963 | 194,912 |

Candidate versus parent: +12/-0 pages, +652/-0 named passes, +1/-0 observed names. Against stable, exclusive sets are 242/1,806 pages, 5,391/15,073 passing names, and 252/852 observed names (candidate/stable). Both LP runs had 101 harness errors, 105 harness timeouts, 11 runner timeouts and one hang; stable had 23/62/10/1 respectively. No new full-run loss to repeat. Run-to-run timing variation from prior checkpoints is not considered fixed.
