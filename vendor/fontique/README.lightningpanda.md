# fontique 0.12.0, patched

The published `fontique` 0.12.0 crate (crates.io; part of
[linebender/parley](https://github.com/linebender/parley)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: Apache-2.0 OR MIT, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff.

`@font-face` [`unicode-range`](https://drafts.csswg.org/css-fonts-4/#unicode-range-desc) in font matching.
Google Fonts declares one face per script subset (cyrillic-ext, cyrillic, greek-ext, greek, vietnamese,
latin-ext, latin), all with the same family, weight and style, and each subset's own character map also covers
characters other subsets are for. fontique yields every face of the best attribute match (`match_fonts`) in
registration order, which is the order the fonts finished loading, and parley takes the first whose character
map covers a cluster. So Lightningpanda drew some characters from a face Chrome does not use for them: in ToS's
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
`font_face_unicode_ranges_and_rule_order_reach_the_query` and lp-render's `font_unicode_range` tests (fontique's
own tests need parley's development fonts, which the published crate leaves out).

## Updating or dropping the patch

Copy the published crate from `~/.cargo/registry/src/*/fontique-<version>` and re-apply `UPSTREAM.patch`
(`git apply` from the crate root), or delete this directory and the `[patch.crates-io]` entry when upstream
matches fonts by `unicode-range` itself.
