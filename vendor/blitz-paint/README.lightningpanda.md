# blitz-paint 0.3.0-beta.4, patched

This is the published MIT/Apache-2.0 `blitz-paint` crate from upstream commit
`92b11f1bd8ab7975411328b0406a95a09445b4e7` (`packages/blitz-paint`).

- `src/text.rs`: paint non-atomic inline ancestor backgrounds behind nested text,
  from outermost to innermost; stop at block/atomic boxes and skip contents boxes.
  Upstream contribution: `xban1x:codex/inline-ancestor-backgrounds` in DioxusLabs/blitz.
- `src/render/border.rs`: skip zero-width border edges (Tailwind's preflight
  `border: 0 solid`); their degenerate polygons cover nothing in a raster but
  print as hairlines in PDF output.
- `src/debug_overlay.rs`: use the real root-element predicate for fixed boxes;
  adapted from https://github.com/DioxusLabs/blitz/pull/1224.
- `src/lib.rs`, `src/text.rs`, `src/render.rs`: `paint_scene_recording_text` paints like
  `paint_scene_with` and also returns each text glyph run it drew (`TextRun`: the element,
  the first glyph, the source text and each glyph's range in it, the content area). PDF output
  uses it for source text (ligatures), link areas and tags. `paint_scene`/`paint_scene_with`
  record nothing.

- `src/render.rs`: `overflow` on `body` does not clip when it is the viewport's.
- `src/render.rs`, `src/text.rs`: vertical writing modes. The inline layout stays logical (x inline,
  y block), and each of its lines is painted through a quarter turn (`draw_vertical_inline_layout`):
  backgrounds, decorations and selection go through the same transform, so `TextRun` bounds, link
  areas and PDF glyph runs need nothing else. A line starts at its unrounded inline position; the box
  is at whole pixels (Chromium rounds text only in the block axis). `VerticalLine` centers the runs of
  `mixed`/`upright` text on the line's central baseline (`sideways` keeps the alphabetic one) and stands
  CJK and `upright` runs upright, one glyph at a time around its em box. Not done: shaping in the
  vertical direction, so `vert` forms (CJK punctuation) are missing and upright Latin keeps its
  horizontal advance. Covered by `crates/lp-render/tests/vertical_writing_mode.rs`.

- `src/lib.rs`, `src/render.rs`, `src/text.rs`: `paint_scene_for_print` and `paint_scene_recording_text` (PDF) draw an
  inline root's text at its layout-unit x; boxes stay on whole pixels, and so do link areas (`TextRun::bounds`). Chromium
  keeps the fraction in the inline axis (37.781 px of padding: its text at 37.781, ours was at 38). Screenshots
  (`paint_scene`) are unchanged: `text_shift` stays 0 there.

The linked blitz-dom patch supplies `Node::is_root_element`. Rendering regressions
are covered in `crates/lp-render/tests/render.rs`; persistent-tree behavior is
covered by `crates/lp-render/tests/mirror.rs`.
