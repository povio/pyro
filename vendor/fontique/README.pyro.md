# fontique 0.12.0, patched

The published `fontique` 0.12.0 crate (crates.io; part of
[linebender/parley](https://github.com/linebender/parley)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: Apache-2.0 OR MIT, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: three changes.

## `unicode-range`

`@font-face` [`unicode-range`](https://drafts.csswg.org/css-fonts-4/#unicode-range-desc) in font matching.
Google Fonts declares one face per script subset (cyrillic-ext, cyrillic, greek-ext, greek, vietnamese,
latin-ext, latin), all with the same family, weight and style, and each subset's own character map also covers
characters other subsets are for. fontique yields every face of the best attribute match (`match_fonts`) in
registration order, which is the order the fonts finished loading, and parley takes the first whose character
map covers a cluster. So Pyro drew some characters from a face Chrome does not use for them: in ToS's
account statements every capital A came from another Inter subset than the rest of its word, and pdf.js split
"Account" into "A" and "ccount".

- `unicode_range.rs`: `UnicodeRange`, the code points a font is used for (ordered, merged intervals; the
  default is every code point).
- `font.rs`: `FontInfoOverride::unicode_range` and `FontInfoOverride::precedence`, kept on `FontInfo`
  (`FontInfo::unicode_range`, `FontInfo::precedence`).
- `collection/query.rs`: `QueryFont::unicode_range`. `QueryFont::charmap` is limited to it
  (`Charmap::limited_to` in `charmap.rs`): a code point outside the range maps to no glyph, so parley's
  coverage check skips the face for it. The fonts of a family that match a query's attributes equally are
  yielded highest precedence first, then in registration order (a stable sort in `load_bucket`): CSS checks
  the faces of a composite font last `@font-face` rule first, whichever loaded first, and blitz-dom gives
  each rule a precedence rising in rule order. `Query::first_available_font` skips fonts whose range
  excludes U+0020, as CSS's first available font does.

Fonts registered without these overrides (system fonts, `Renderer::register_font`) cover every code point and
have precedence 0. Covered by blitz-dom's `unicode_ranges_keep_their_code_points` and
`font_face_unicode_ranges_and_rule_order_reach_the_query` and pyro-render's `font_unicode_range` tests (fontique's
own tests need parley's development fonts, which the published crate leaves out).

## The scan of font directories

`Collection::load_fonts_from_paths` (Pyro's system fonts under `serve --fork`, where the platform
font API cannot be used) opened, mapped and read one file after another, and rebuilt a family for every face
it found. On macOS (372 files, 380 families) that took 10-11 ms, 7 ms of it opening files, and it was the
longest step of a fork template's start. Now `scan::list_files` lists the files (a directory entry's own type
instead of a `stat` each), up to four threads read them (`scan::scan_file`), and the faces are registered on
the calling thread in the order the walk found the files, each family built once: 4 ms, with the same
families, faces and face order (a dump of both was compared). The threads are joined before the call returns.

## Synthetic bold from weight 600

`FontInfo::synthesis` asked for emboldening only when the requested weight was more than 200 above the face's
(and the face has no `wght` axis), so `font-weight: 600` on a family with only a regular face (ToS's Lato
headings) was drawn thin. Chromium synthesizes bold when the request is at or above 600 and the face is below
it (measured with a web font declared at 300-700 and with single-face system families: 600, 700 and 900 are
emboldened on a 400 face, 500 is not, and 900 on a 700 face is not). `SYNTHETIC_BOLD_WEIGHT` in `font.rs` is that
rule. The amount drawn is blitz-paint's (`synthetic_bold`). One case differs: on macOS Chromium does not
embolden Impact (Core Text reports its only face as bold, though its `OS/2` weight is 400); pyro does, as
Chromium does on other platforms. Covered by `static_weight_synthesis_is_for_a_bold_request_on_a_lighter_face`
here and pyro-render's `a_missing_bold_face_is_synthesized`.

## Updating or dropping the patch

Copy the published crate from `~/.cargo/registry/src/*/fontique-<version>` and re-apply `UPSTREAM.patch`
(`git apply` from the crate root), or delete this directory and the `[patch.crates-io]` entry when upstream
matches fonts by `unicode-range` itself (the scan change is speed only; the bold rule is one condition in
`FontInfo::synthesis`).

## Slant of variable fonts (WPT css-fonts, round 3 lane F)

`src/font.rs` (`FontInfo::synthesis`, `slant_axis_value`, `FontInfoOverride::slant_range`): the `slnt` axis counts
counter-clockwise and CSS angles clockwise, so `font-style: oblique 10deg` is `slnt -10` (upstream set `slnt 14`
for italic, a backslant). A `@font-face` rule's `font-style` descriptor says how far the axis may go: a range
(`oblique 0deg 10deg`) clamps the angle, `normal` leaves the axis alone and the slant is faked, as for a font
without the axis (WPT `variations/font-slant-1`, `-2c`, `-3`, `slnt-variable`, `slnt-backslant-variable`).
Without a descriptor the font's own axis is used as before. Test:
`slant_synthesis_follows_the_axis_direction_and_the_allowed_range`.

## Empty `@font-face` family names (WPT css-fonts, round 3 lane F)

`src/collection/mod.rs` (`register_scanned_font`): a face registered under an override family name `""` is kept
(CSS Fonts 4: a quoted family name may be empty, `font-family: ""`); a font file without a name of its own is still
left out (WPT `font-palette-empty-font-family`).

## `@font-face` `size-adjust` and metrics overrides (WPT css-fonts, round 3 lane F)

`src/font.rs` (`FaceMetrics`, `FontInfoOverride::face_metrics`, `Synthesis::face_metrics`, `size_adjust`,
`without_size_adjust`): a face registered with `size-adjust`, `ascent-override`, `descent-override` or
`line-gap-override` keeps them, and the synthesis a query gives for it carries them to the shaper (parley_engine
applies them), as it carries the variation settings.

## `@font-face` `font-feature-settings` and `font-variation-settings` (WPT css-fonts, round 3 lane F)

`src/font.rs` (`FaceSettings`, `FontInfoOverride::face_settings`, `Synthesis::face_settings`): a face's descriptor
features and variation settings are interned in a process-wide table (`FaceSettings::intern`, behind `std`; equal
settings share an id) and faces and their synthesis carry the id, so `Synthesis` stays `Copy`. parley_engine
applies them.

## Font matching with `@font-face` ranges (WPT css-fonts, round 3 lane F)

`src/font.rs` (`FontInfoOverride::weight_range`, `width_range`, `FontInfo::matched_weight`, `matched_width`,
`matched_style`, `synthesis`), `src/matching.rs` (`match_font`, `match_fonts`): a face whose `@font-face` rule gives
a `font-weight` or `font-stretch` range, or an oblique `font-style` range, is a candidate at the value of its range
nearest the one asked for (CSS Fonts 4 §5.2; a reversed range is the same range). Upstream kept one value per
attribute (the lower bound). A variable font's `wght`/`wdth` axis is held to the range, and a bold request on a face
whose range has no bold in it is faked bold, as on a face without the axis (WPT `matching/range-descriptor-reversed`,
`variations/font-descriptor-range-reversed*`, `variations/font-weight-3`).

A face whose `font-style` range is all oblique (`oblique 45deg 67.5deg`) has its `slnt` axis held to the angle of the
range nearest the request (0deg for normal, 14deg for italic) whatever is asked (WPT
`matching/fixed-stretch-style-over-weight`).
