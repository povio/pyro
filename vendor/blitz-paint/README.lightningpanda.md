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

`overflow` on `body` does not clip when it is the viewport's.

The linked blitz-dom patch supplies `Node::is_root_element`. Rendering regressions
are covered in `crates/lp-render/tests/render.rs`; persistent-tree behavior is
covered by `crates/lp-render/tests/mirror.rs`.
