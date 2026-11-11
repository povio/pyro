# blitz-paint main (c27b4ca), patched

`packages/blitz-paint` of DioxusLabs/blitz main at commit `c27b4ca14158dc457f35c78eb32b8c4fec906b7e` (see
`vendor/blitz-dom/README.lightningpanda.md`), MIT/Apache-2.0. `UPSTREAM.patch` (applies with `git apply`
from the root of an upstream checkout at that commit) is the whole diff.

- `src/text.rs`: paint non-atomic inline ancestor backgrounds behind nested text,
  from outermost to innermost; stop at block/atomic boxes and skip contents boxes.
  Upstream contribution: `xban1x:codex/inline-ancestor-backgrounds` in DioxusLabs/blitz.
- `src/render/border.rs`: skip zero-width border edges (Tailwind's preflight
  `border: 0 solid`); their degenerate polygons cover nothing in a raster but
  print as hairlines in PDF output.
- `src/debug_overlay.rs`: use the real root-element predicate for fixed boxes;
  adapted from https://github.com/DioxusLabs/blitz/pull/1224.
- `src/lib.rs`, `src/text.rs`, `src/render.rs`: `paint_scene_recording_text` paints like
  `paint_scene_for_print` and also returns each text glyph run it drew (`TextRun`: the element,
  the glyph count and a `glyphs_fingerprint` of the glyphs as drawn, the source text and each
  glyph's range in it, the content area). Only drawn runs are recorded (zero-size text is not
  drawn): the PDF matches each drawn glyph run to the next recorded one. PDF output uses it for
  each glyph's text on page boxes and paper cuts (ligatures, glyphs OpenType features
  substituted), and for link areas and tags on paper cuts. `paint_scene`/`paint_scene_with`
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

- `src/render/border.rs`: `draw_collapsed_edges` paints the winning borders of a collapsed table without uniform ones,
  snapped to whole pixels of the page as Chromium does. Cells and tables in it draw no border of their own.

- `src/render.rs` (`paint_top_layer`, `paint_backdrop`): the document's top layer (blitz-dom's
  `top_layer_nodes`) is skipped in tree order and painted after the root element, bottom first, each element over
  its `::backdrop` (background color, opacity, `backdrop-filter` over the painted area), positioned as a box hoisted
  to the root element. With an empty top layer the only cost is one emptiness check per box. Covered by
  `crates/lp-cdp/tests/engine_cdp.rs` (`top_layer_*`).

- `src/render.rs` (`render_node`): each level of the paint recursion runs through blitz-dom's `util::ensure_stack`,
  which grows the stack on demand, so a tree of any depth paints instead of overflowing the thread's stack and
  aborting the process (see blitz-dom's "Deep trees grow the stack on demand").

The linked blitz-dom patch supplies `Node::is_root_element`. Rendering regressions
are covered in `crates/lp-render/tests/render.rs`; persistent-tree behavior is
covered by `crates/lp-render/tests/mirror.rs`.

## Moving from 0.3.0-beta.4 to main

- `src/render.rs`: main computes clipping in `should_clip_node`; `overflow` on `body` that is the
  viewport's is left out there. Print's `backgrounds` switch also guards main's `paints_own_box` branch
  (#1319).
- `src/render.rs`: main draws inline text from the unrounded content box origin (#1382); the offset stays
  zero (see blitz-dom), so screenshots and print are as before. Vertical writing modes return before it.
- `src/text.rs`: glyph runs are recorded (`TextRun`) before main's zero-size check (#1402), and the
  vertical-line and source-text arguments go with main's `TextPaintOptions`; the `background-clip: text`
  mask (#1340) passes none.
- Font embolden (#1357, #1361, #1364, #1444) stays off: lightningpanda enables neither `font-embolden`
  feature.
