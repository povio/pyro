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
