# symphonia-codec-aac 0.6.1, patched

The published `symphonia-codec-aac` 0.6.1 crate (crates.io; part of [Symphonia](https://github.com/pdeljanov/Symphonia)),
used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MPL-2.0, as upstream (`LICENSE`).
`Cargo.lock`, `Cargo.toml.orig` and `tests/` were left out of the copy, and the `[[test]]` section was removed
from `Cargo.toml`. No dependency was added.

## The change

`src/aac/mod.rs`: upstream refuses every stream with more than 2 channels ("aac too complex"), although the
element loop (`decode_ga`) already decodes any number of SCE, CPE and LFE elements. WPT `encrypted-media`
plays AAC-LC 5.1 (`AudioSpecificConfig` `11 b0`, channel configuration 6) and Chrome decodes it.

Channel configurations 3, 4, 5 and 6 of ISO/IEC 14496-3 Table 1.19 are accepted, next to the 1 and 2 of
before. Configuration 7 (7.1, which the ASC reader labels with the front-wide layout) and the program config
element stay unsupported, as do SBR, PS, coupling channels and non-LC object types.

* Elements arrive in the fixed order of the table: 3.0 is SCE centre, CPE left/right; 4.0 adds an SCE rear
  centre; 5.0 is SCE centre, CPE left/right, CPE rear left/right; 5.1 adds the LFE element last.
* Output planes follow the canonical order of a positioned layout (`Position` bit order), the order of
  Web Audio and WebCodecs: left, right, centre, LFE, rear left, rear right (5.1) and left, right, centre,
  rear centre (4.0). `element_planes` maps the element order to those planes, and `ChannelPair::synth_audio`
  takes the plane indices instead of writing to `channel`/`channel + 1`. Without it the centre channel
  would be written to the plane of the left channel.
* There is no down-mix in the decoder: the buffer has one plane per coded channel and the consumer decides
  (Pyro's MSE presentation has no audio output device, WebCodecs `AudioData` reports the coded channel
  count like Chrome).
* Mono and stereo take the same path as before (planes 0 and 1).
