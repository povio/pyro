//! A whole document's layout for `DOMSnapshot.captureSnapshot`: every DOM
//! node (with `::before` and `::after`) in tree order, each one's box and its
//! text fragments, built only when asked for.
//!
//! The values are the unrounded layout, not the layout units
//! [`Geometry::element_box`] reports to scripts, so that a comparison against
//! Chromium can tell its 1/64 px arithmetic from other differences.
//!
//! How they map to Chromium's (`inspector_dom_snapshot_agent.cc`):
//!
//! - A box's rect is [`Geometry::adjusted_box`], the border box in document
//!   coordinates, as `RectInDocument` is the layout object's absolute bounding
//!   box. Inline boxes are the union of their fragments, in both.
//! - A text fragment is one text node on one line: the advances of its
//!   clusters, without the collapsible white space a line ends in (Chromium's
//!   fragment leaves it out), from baseline less whole-px ascent to baseline
//!   plus whole-px descent (Chromium's fragment has the font's rounded
//!   metrics, not the line's height). Its y is where the glyphs are painted.
//!   Chromium breaks a fragment at collapsed white space and for bidi, so its
//!   boxes are finer: merge those on a line to compare.

use blitz_dom::{BaseDocument, NodeId};
use kurbo::Rect;
use rustc_hash::FxHashMap;

use crate::geometry::{in_layout_tree, line_box_origin, line_slot, utf16_offsets, Geometry};

/// A document's layout ([`crate::Renderer::layout_snapshot`]).
#[derive(Clone, Debug, Default)]
pub struct LayoutSnapshot {
    /// Tree order; parents come first.
    pub nodes: Vec<SnapshotNode>,
    pub lines: Vec<SnapshotLine>,
    pub document_width: f64,
    pub document_height: f64,
}

#[derive(Clone, Debug)]
pub struct SnapshotNode {
    /// Index in [`LayoutSnapshot::nodes`] (`None`: the root element).
    pub parent: Option<usize>,
    pub kind: SnapshotKind,
    /// `[x, y, width, height]`, CSS px; `None`: no box.
    pub rect: Option<[f64; 4]>,
    /// Inside (or itself) a `position: fixed` box: viewport coordinates.
    pub fixed: bool,
    pub text: Vec<SnapshotText>,
}

#[derive(Clone, Debug)]
pub enum SnapshotKind {
    Element { name: String, attrs: Vec<(String, String)> },
    Text(String),
    /// `::before` or `::after`, whose boxes and generated text it holds.
    Pseudo(&'static str),
}

/// One text node's fragment on a line.
#[derive(Clone, Debug)]
pub struct SnapshotText {
    pub rect: [f64; 4],
    /// UTF-16 offset into the node's data.
    pub start: u32,
    /// UTF-16 length of what it laid out.
    pub length: u32,
    /// The text laid out (white space collapsed, case transformed).
    pub text: String,
    pub baseline: f64,
    /// Unrounded font metrics, CSS px.
    pub ascent: f64,
    pub descent: f64,
    pub font_size: f64,
}

/// A line box of an inline formatting context.
#[derive(Clone, Debug)]
pub struct SnapshotLine {
    /// The text node the line's first fragment belongs to.
    pub first_text: NodeId,
    /// `[x, y, width, height]`, the line's block extent and its advance.
    pub rect: [f64; 4],
    pub baseline: f64,
}

/// Clusters of one text node on a line, in visual order.
struct Piece {
    owner: usize,
    x0: f64,
    x1: f64,
    bytes: (usize, usize),
    trailing_white_space: bool,
    top: f64,
    bottom: f64,
    baseline: f64,
    ascent: f64,
    descent: f64,
    font_size: f64,
}

impl Geometry {
    pub(crate) fn layout_snapshot(&self, doc: &BaseDocument, size: (f64, f64)) -> LayoutSnapshot {
        let (fragments, lines) = self.text_fragments(doc);
        let mut out = LayoutSnapshot { lines, document_width: size.0, document_height: size.1, ..Default::default() };
        let mut stack = vec![(doc.root_element().id, None, None)];
        while let Some((id, parent, pseudo)) = stack.pop() {
            let Some(node) = doc.get_node(id) else { continue };
            let index = out.nodes.len();
            let text: Vec<SnapshotText> = fragments.get(&id).cloned().unwrap_or_default();
            let kind = match (pseudo, node.text_data(), node.element_data()) {
                (Some(p), _, _) => SnapshotKind::Pseudo(p),
                (None, Some(t), _) => SnapshotKind::Text(t.content.clone()),
                (None, None, Some(e)) => SnapshotKind::Element {
                    name: e.name.local.to_string(),
                    attrs: e.attrs().iter().map(|a| (a.name.local.to_string(), a.value.clone())).collect(),
                },
                (None, None, None) => continue,
            };
            let (rect, fixed) = if matches!(kind, SnapshotKind::Text(_)) {
                (text_rect(&text), self.adjusted_box(doc, id).is_some_and(|b| b.fixed))
            } else {
                let rendered = node.primary_styles().is_some_and(|s| {
                    let display = s.clone_display();
                    !display.is_none() && !display.is_contents()
                });
                match (rendered, self.adjusted_box(doc, id)) {
                    (true, Some(b)) => (Some([b.x, b.y, b.width, b.height]), b.fixed),
                    _ => (None, false),
                }
            };
            out.nodes.push(SnapshotNode { parent, kind, rect, fixed, text });
            if node.element_data().is_some() {
                // Reversed: the stack gives them back in tree order.
                if let Some(after) = node.after() {
                    stack.push((after, Some(index), Some("::after")));
                }
                for &child in node.children.iter().rev() {
                    stack.push((child, Some(index), None));
                }
                if let Some(before) = node.before() {
                    stack.push((before, Some(index), Some("::before")));
                }
            }
        }
        out
    }

    /// Every text node's fragments by node, and the line boxes.
    fn text_fragments(&self, doc: &BaseDocument) -> (FxHashMap<NodeId, Vec<SnapshotText>>, Vec<SnapshotLine>) {
        let mut fragments: FxHashMap<NodeId, Vec<SnapshotText>> = FxHashMap::default();
        let mut boxes = Vec::new();
        for (id, node) in doc.tree().iter() {
            let Some(text) = node.element_data().and_then(|e| e.inline_layout_data.as_ref()) else { continue };
            if !node.flags.is_inline_root() || text.text_nodes.is_empty() || !in_layout_tree(doc, id) {
                continue;
            }
            let Some((t, _)) = self.transform(doc, id) else { continue };
            let origin = t * line_box_origin(doc, id, text);
            let scale = text.layout.scale() as f64;
            let offsets: Vec<Vec<u32>> = text
                .text_nodes
                .iter()
                .map(|(n, range)| {
                    let source = doc.get_node(*n).and_then(|n| n.text_data()).map_or("", |d| d.content.as_str());
                    utf16_offsets(source, &text.text[range.clone()])
                })
                .collect();
            let owner = |byte: usize| {
                let i = text.text_nodes.partition_point(|(_, r)| r.end <= byte);
                text.text_nodes.get(i).filter(|(_, r)| r.start <= byte).map(|_| i)
            };
            for line in text.layout.lines() {
                let m = line.metrics();
                let origin = origin * line_slot(doc, id, m, scale);
                let mut pieces: Vec<Piece> = Vec::new();
                for item in line.items() {
                    let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                    let metrics = run.run().font_metrics();
                    let baseline = run.baseline() as f64;
                    let (a, d) = (metrics.ascent as f64, metrics.descent as f64);
                    let mut x = run.offset() as f64;
                    for cluster in run.run().visual_clusters() {
                        let advance = cluster.advance() as f64;
                        let range = cluster.text_range();
                        let at = owner(range.start);
                        if let Some(i) = at.filter(|_| !cluster.is_hard_line_break()) {
                            let white = text.text[range.clone()].chars().all(|c| matches!(c, ' ' | '\t' | '\n'))
                                && !range.is_empty()
                                && collapses_white_space(doc, text.text_nodes[i].0);
                            pieces.push(Piece {
                                owner: i,
                                x0: x,
                                x1: x + advance,
                                bytes: (range.start, range.end),
                                trailing_white_space: white,
                                top: baseline - a.round(),
                                bottom: baseline + d.round(),
                                baseline,
                                ascent: a,
                                descent: d,
                                font_size: run.run().font_size() as f64,
                            });
                        }
                        x += advance;
                    }
                }
                while pieces.last().is_some_and(|p| p.trailing_white_space) {
                    pieces.pop();
                }
                let map_rect = |x0: f64, y0: f64, x1: f64, y1: f64| {
                    let r = origin.transform_rect_bbox(Rect::new(x0 / scale, y0 / scale, x1 / scale, y1 / scale));
                    [r.x0, r.y0, r.width(), r.height()]
                };
                let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
                for p in &pieces {
                    x0 = x0.min(p.x0);
                    x1 = x1.max(p.x1);
                }
                if let Some(first) = pieces.first() {
                    boxes.push(SnapshotLine {
                        first_text: text.text_nodes[first.owner].0,
                        rect: map_rect(x0, m.block_min_coord as f64, x1, m.block_max_coord as f64),
                        baseline: map_rect(0.0, first.baseline, 0.0, first.baseline)[1],
                    });
                }
                // One fragment per text node on the line.
                let mut seen: Vec<usize> = Vec::new();
                for p in &pieces {
                    if !seen.contains(&p.owner) {
                        seen.push(p.owner);
                    }
                }
                for i in seen {
                    let own = || pieces.iter().filter(|p| p.owner == i);
                    let first = own().next().expect("an owner has a piece");
                    let x0 = own().map(|p| p.x0).fold(f64::INFINITY, f64::min);
                    let x1 = own().map(|p| p.x1).fold(f64::NEG_INFINITY, f64::max);
                    let top = own().map(|p| p.top).fold(f64::INFINITY, f64::min);
                    let bottom = own().map(|p| p.bottom).fold(f64::NEG_INFINITY, f64::max);
                    let lo = own().map(|p| p.bytes.0).min().expect("a piece");
                    let hi = own().map(|p| p.bytes.1).max().expect("a piece");
                    let (node, range) = &text.text_nodes[i];
                    let map = &offsets[i];
                    let at = |b: usize| map[(b - range.start).min(map.len() - 1)];
                    let laid = &text.text[lo..hi];
                    fragments.entry(*node).or_default().push(SnapshotText {
                        rect: map_rect(x0, top, x1, bottom),
                        start: at(lo),
                        length: laid.encode_utf16().count() as u32,
                        text: laid.to_owned(),
                        baseline: map_rect(0.0, first.baseline, 0.0, first.baseline)[1],
                        ascent: first.ascent / scale,
                        descent: first.descent / scale,
                        font_size: first.font_size / scale,
                    });
                }
            }
        }
        (fragments, boxes)
    }
}

/// The union of a text node's fragments.
fn text_rect(text: &[SnapshotText]) -> Option<[f64; 4]> {
    let mut acc: Option<Rect> = None;
    for t in text {
        let r = Rect::new(t.rect[0], t.rect[1], t.rect[0] + t.rect[2], t.rect[1] + t.rect[3]);
        acc = Some(acc.map_or(r, |a| a.union(r)));
    }
    acc.map(|r| [r.x0, r.y0, r.width(), r.height()])
}

/// Whether a line ending in `id`'s white space drops it (`white-space-collapse: collapse`).
fn collapses_white_space(doc: &BaseDocument, id: NodeId) -> bool {
    use style::computed_values::white_space_collapse::T;
    let parent = doc.get_node(id).and_then(|n| n.parent).and_then(|p| doc.get_node(p));
    parent.and_then(|p| p.primary_styles()).is_none_or(|s| s.clone_white_space_collapse() == T::Collapse)
}
