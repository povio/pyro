//! The layout of an SVG `<text>` as `SVGTextContentElement`'s query methods (`getNumberOfChars`,
//! `getComputedTextLength`, `getStartPositionOfChar`, `getExtentOfChar`, `getCharNumAtPosition`, ...)
//! and `getBBox()` answer it (SVG 2 § 11.5 "text layout algorithm", the parts that decide where a
//! character is).
//!
//! The characters are the text content's addressable UTF-16 code units after white-space processing
//! (`xml:space` / `white-space`: collapsed across the whole `<text>`, its leading and trailing space
//! removed, or preserved). Their advances are the ones the engine shapes: each run of one text node
//! is shaped with the canvas text shaper (`pyro_canvas::text`: Parley on the system fonts plus the
//! page's web faces, which is also what `measureText` answers), a character's advance being the
//! growth of the shaped run's width when the character (with its combining marks and joiners) is
//! appended, so kerning and ligatures are in it. The `x`, `y`, `dx`, `dy` and `rotate` lists of
//! `<text>` and `<tspan>` (a descendant's value replaces its ancestor's), the `textLength` /
//! `lengthAdjust` adjustment of each element that has one, and `text-anchor` per text chunk place
//! them. `writing-mode` picks the inline axis (`sideways-lr` runs upward); upright East Asian
//! characters of vertical text advance by the font size.
//!
//! Not done: bidirectional reordering (characters stay in logical order along the inline axis),
//! `<textPath>` (its characters are laid out as if on a straight line), `letter-spacing`
//! inside `textLength` ranges beyond what the shaper applies, `baseline-shift`.

use std::rc::Rc;

use pyro_canvas::text::{layout_with_faces, TextStyle};
use pyro_dom::computed::{resolve_len, LengthContext};
use pyro_dom::layout::{Layout, Rect};
use pyro_dom::{Dom, NodeId, NodeType};

use crate::page::PageDom;

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// One addressable character (a UTF-16 code unit) of a text, with where layout put it.
#[derive(Clone, Debug)]
pub(crate) struct CharBox {
    /// The text node it comes from and its UTF-16 offset in that node's data.
    pub node: NodeId,
    pub offset: u32,
    /// The second code unit of a surrogate pair, or a mark joined to the cluster before it: it has
    /// no position of its own (the cluster's) and no advance.
    pub cont: bool,
    /// The origin on the baseline, in the `<text>`'s user space.
    pub start: (f64, f64),
    /// The advance along the inline axis (after a `lengthAdjust="spacingAndGlyphs"` stretch).
    pub adv: f64,
    pub ascent: f64,
    pub descent: f64,
    /// `rotate`, in degrees.
    pub rotate: f64,
    pub vertical: bool,
    /// The inline axis points up (`sideways-lr`).
    pub backward: bool,
    /// How far the glyph box moves down for the `dominant-baseline`.
    pub shift: f64,
}

/// The characters of one `<text>` in order, with the range of each element inside it.
#[derive(Default)]
pub(crate) struct TextLayout {
    pub chars: Vec<CharBox>,
    ranges: Vec<(NodeId, usize, usize)>,
}

impl TextLayout {
    /// The characters `el` holds: `[start, end)` of [`TextLayout::chars`] (empty for an element that is
    /// not rendered text).
    pub fn range(&self, el: NodeId) -> (usize, usize) {
        self.ranges.iter().find(|(e, _, _)| *e == el).map_or((0, 0), |&(_, s, e)| (s, e))
    }

    /// The first code unit of the cluster `i` is in.
    fn head(&self, mut i: usize) -> usize {
        while i > 0 && self.chars[i].cont {
            i -= 1;
        }
        i
    }

    pub fn start(&self, i: usize) -> (f64, f64) {
        self.chars[self.head(i)].start
    }

    pub fn end(&self, i: usize) -> (f64, f64) {
        let c = &self.chars[self.head(i)];
        let d = if c.backward { -c.adv } else { c.adv };
        if c.vertical {
            (c.start.0, c.start.1 + d)
        } else {
            (c.start.0 + d, c.start.1)
        }
    }

    /// The glyph cell of character `i`.
    pub fn extent(&self, i: usize) -> Rect {
        let c = &self.chars[self.head(i)];
        let end = self.end(i);
        let h = c.ascent + c.descent;
        if c.vertical {
            let (y0, y1) = (c.start.1.min(end.1), c.start.1.max(end.1));
            Rect { x: c.start.0 - h / 2.0, y: y0, width: h, height: y1 - y0 }
        } else {
            let (x0, x1) = (c.start.0.min(end.0), c.start.0.max(end.0));
            Rect { x: x0, y: c.start.1 - c.ascent + c.shift, width: x1 - x0, height: h }
        }
    }

    /// The sum of the advances of `[from, to)`.
    pub fn length(&self, from: usize, to: usize) -> f64 {
        self.chars[from..to].iter().map(|c| c.adv).sum()
    }

    /// The first character of `[from, to)` whose glyph cell holds `(x, y)`, as its index in the range.
    pub fn char_at(&self, from: usize, to: usize, x: f64, y: f64) -> i32 {
        for i in from..to {
            if self.chars[i].cont {
                continue;
            }
            let c = &self.chars[i];
            // The point in the (unrotated) frame of the character's origin.
            let (dx, dy) = (x - c.start.0, y - c.start.1);
            let (sin, cos) = (-c.rotate).to_radians().sin_cos();
            let p = (c.start.0 + dx * cos - dy * sin, c.start.1 + dx * sin + dy * cos);
            let r = self.extent(i);
            if p.0 >= r.x && p.0 <= r.x + r.width && p.1 >= r.y && p.1 <= r.y + r.height {
                return (i - from) as i32;
            }
        }
        -1
    }

    /// The union of the glyph cells of `[from, to)`: `getBBox()` of a text content element.
    pub fn bbox(&self, from: usize, to: usize) -> Option<Rect> {
        let mut out: Option<(f64, f64, f64, f64)> = None;
        for i in from..to {
            if self.chars[i].cont {
                continue;
            }
            let r = self.extent(i);
            out = Some(match out {
                None => (r.x, r.y, r.x + r.width, r.y + r.height),
                Some((x0, y0, x1, y1)) => (x0.min(r.x), y0.min(r.y), x1.max(r.x + r.width), y1.max(r.y + r.height)),
            });
        }
        out.map(|(x0, y0, x1, y1)| Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
    }
}

#[derive(Clone)]
struct Entry {
    ch: char,
    node: NodeId,
    offset: u32,
    el: NodeId,
    cont: bool,
    /// A space that white-space collapsing made out of white space.
    collapsed: bool,
}

struct ElStyle {
    family: String,
    size: f64,
    weight: f32,
    italic: bool,
    letter_spacing: f32,
    word_spacing: f32,
    vertical: bool,
    backward: bool,
    upright: bool,
    mixed: bool,
    /// 0 start, 1 middle, 2 end.
    anchor: i8,
    baseline: String,
}

struct Builder<'a> {
    page: &'a PageDom,
    dom: &'a Dom,
    layout: &'a Layout,
    entries: Vec<Entry>,
    ranges: Vec<(NodeId, usize, usize)>,
    /// `text` and `tspan` elements in tree order (the carriers of `x`, `y`, `dx`, `dy`, `rotate`).
    positioned: Vec<NodeId>,
    /// The elements with a `textLength`, in tree order.
    adjusted: Vec<NodeId>,
    after_space: bool,
    styles: Vec<(NodeId, Rc<ElStyle>)>,
}

fn svg_local(dom: &Dom, el: NodeId) -> Option<String> {
    let e = dom.element(el).ok()?;
    (e.name.ns == markup5ever::ns!(svg)).then(|| e.name.local.to_string())
}

/// The `<text>` that `el` is, or is inside through `tspan`, `a` and `textPath`.
fn text_root(dom: &Dom, el: NodeId) -> Option<NodeId> {
    let mut cur = Some(el);
    while let Some(c) = cur {
        match svg_local(dom, c).as_deref() {
            Some("text") => return Some(c),
            Some("tspan" | "a" | "textPath") => {}
            _ => return None,
        }
        cur = dom.parent_element(c).ok().flatten();
    }
    None
}

/// What a cached text layout depends on: the DOM version (nodes, attributes, text and the styles they
/// select), the layout viewport (percentages, viewport units) and the faces the shaper can use (the
/// realm's `FontFaceSet` generation and which CSS faces have their bytes). Hover and focus do not
/// change the DOM version, so a `:hover` rule that restyles the text is seen at the next change.
fn cache_key(page: &PageDom, dom: &Dom, layout: &Layout) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    dom.version().hash(&mut h);
    let v = layout.viewport();
    (v.width, v.height).hash(&mut h);
    page.canvas_faces().0.hash(&mut h);
    for face in page.css_font_faces().iter() {
        for source in &face.sources {
            if let pyro_dom::font_face::FontSource::Url(url, _) = source {
                page.font_url_bytes(url).is_some().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// The layout of the `<text>` that `el` is part of (empty when it is not in one), computed once per
/// [`cache_key`] and `<text>`.
pub(crate) fn layout_of(page: &PageDom, dom: &Dom, layout: &Layout, el: NodeId) -> Rc<TextLayout> {
    let Some(root) = text_root(dom, el) else { return Rc::default() };
    let key = cache_key(page, dom, layout);
    if let Ok(mut cache) = page.svg_text_cache.try_borrow_mut() {
        if cache.0 != key {
            *cache = (key, Default::default());
        } else if let Some(hit) = cache.1.get(&root) {
            return hit.clone();
        }
    }
    let built = Rc::new(build_layout(page, dom, layout, root));
    if let Ok(mut cache) = page.svg_text_cache.try_borrow_mut() {
        if cache.0 == key {
            cache.1.insert(root, built.clone());
        }
    }
    built
}

fn build_layout(page: &PageDom, dom: &Dom, layout: &Layout, root: NodeId) -> TextLayout {
    // A text that is not rendered has no characters.
    if layout.style().has_display_none(dom, root) {
        return TextLayout::default();
    }
    let mut b = Builder {
        page,
        dom,
        layout,
        entries: Vec::new(),
        ranges: Vec::new(),
        positioned: Vec::new(),
        adjusted: Vec::new(),
        after_space: true,
        styles: Vec::new(),
    };
    b.element(root, 0);
    b.build()
}

fn is_upright_char(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x303F | 0x3040..=0x30FF | 0x3100..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// Whether `c` joins the cluster before it (a combining mark, a variation selector, a joiner, an
/// emoji modifier), or `prev` joins it to what follows (a joiner).
fn joins(prev: char, c: char) -> bool {
    prev == '\u{200D}'
        || matches!(c as u32,
            0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F | 0x0E31 | 0x0E34..=0x0E3A
            | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F
            | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

/// A computed `font-weight` as a number: the keywords (`normal` 400, `bold` 700; `bolder` and `lighter`
/// are resolved by the cascade, the fallbacks here are their usual results) as well as numbers.
fn font_weight(v: &str) -> f32 {
    let v = v.trim();
    match v.to_ascii_lowercase().as_str() {
        "normal" => 400.0,
        "bold" | "bolder" => 700.0,
        "lighter" => 100.0,
        n => n.parse::<f32>().ok().filter(|w| w.is_finite() && *w >= 1.0).map_or(400.0, |w| w.min(1000.0)),
    }
}

fn px_value(v: &str) -> f32 {
    v.trim().strip_suffix("px").and_then(|n| n.trim().parse().ok()).unwrap_or(0.0)
}

impl<'a> Builder<'a> {
    fn style_of(&mut self, el: NodeId) -> Rc<ElStyle> {
        if let Some((_, s)) = self.styles.iter().find(|(e, _)| *e == el) {
            return s.clone();
        }
        let (dom, layout) = (self.dom, self.layout);
        let prop = |name: &str| layout.computed_style_value(dom, el, name);
        // The computed `font-size` is in user units: `zoom` is not in it.
        let size = f64::from(px_value(&prop("font-size")));
        let mode = prop("writing-mode");
        let vertical = matches!(mode.as_str(), "vertical-rl" | "vertical-lr" | "sideways-rl" | "sideways-lr" | "tb" | "tb-rl");
        let sideways = mode.starts_with("sideways");
        let orientation = prop("text-orientation");
        let anchor = match prop("text-anchor").trim() {
            "middle" => 1,
            "end" => 2,
            _ => 0,
        };
        let family = prop("font-family");
        let style = prop("font-style");
        let s = Rc::new(ElStyle {
            family: if family.trim().is_empty() { "serif".to_string() } else { family },
            size: if size.is_finite() { size.max(0.0) } else { 0.0 },
            weight: font_weight(&prop("font-weight")),
            italic: style.starts_with("italic") || style.starts_with("oblique"),
            letter_spacing: px_value(&prop("letter-spacing")),
            word_spacing: px_value(&prop("word-spacing")),
            vertical,
            backward: mode == "sideways-lr",
            upright: !sideways && orientation.trim() == "upright",
            mixed: !sideways && orientation.trim() != "sideways",
            anchor,
            baseline: prop("dominant-baseline").trim().to_string(),
        });
        self.styles.push((el, s.clone()));
        s
    }

    /// Whether the white space of text directly in `el` is preserved: the nearest `xml:space`, else the
    /// `white-space` property.
    fn preserves(&self, el: NodeId) -> bool {
        let mut cur = Some(el);
        while let Some(c) = cur {
            match self.dom.get_attribute_ns(c, Some(XML_NS), "space").ok().flatten() {
                Some("preserve") => return true,
                Some("default") => return false,
                _ => {}
            }
            if svg_local(self.dom, c).as_deref() == Some("text") {
                break;
            }
            cur = self.dom.parent_element(c).ok().flatten();
        }
        let ws = self.layout.computed_style_value(self.dom, el, "white-space");
        ws.starts_with("pre") || ws == "break-spaces"
    }

    fn element(&mut self, el: NodeId, depth: u32) {
        if depth > 64 {
            return;
        }
        let dom = self.dom;
        let start = self.entries.len();
        let index = self.ranges.len();
        self.ranges.push((el, start, start));
        match svg_local(dom, el).as_deref() {
            Some("text" | "tspan") => self.positioned.push(el),
            _ => {}
        }
        if dom.get_attribute(el, "textLength").ok().flatten().is_some() {
            self.adjusted.push(el);
        }
        if let Ok(children) = dom.children(el) {
            for child in children {
                match dom.node_type(child) {
                    Ok(NodeType::Text | NodeType::CDataSection) => self.text(child, el),
                    Ok(NodeType::Element) => {
                        if matches!(svg_local(dom, child).as_deref(), Some("tspan" | "a" | "textPath"))
                            && !self.layout.style().has_display_none(dom, child)
                        {
                            self.element(child, depth + 1);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.ranges[index].2 = self.entries.len();
    }

    /// The characters of text node `node` (a child of `el`) after white-space processing.
    fn text(&mut self, node: NodeId, el: NodeId) {
        let dom = self.dom;
        let Ok(Some(data)) = dom.text_content_ref(node) else { return };
        let preserve = self.preserves(el);
        let mut offset = 0u32;
        for ch in data.chars() {
            let units = ch.len_utf16() as u32;
            let white = matches!(ch, ' ' | '\t' | '\n' | '\r');
            let emit = if preserve {
                Some((if white { ' ' } else { ch }, false))
            } else if white {
                if self.after_space {
                    None
                } else {
                    Some((' ', true))
                }
            } else {
                Some((ch, false))
            };
            if let Some((c, collapsed)) = emit {
                self.after_space = !preserve && c == ' ';
                self.entries.push(Entry { ch: c, node, offset, el, cont: false, collapsed });
                if units == 2 {
                    self.entries.push(Entry { ch: c, node, offset: offset + 1, el, cont: true, collapsed: false });
                }
            }
            offset += units;
        }
    }

    /// Shapes each text node's characters and places them.
    fn build(mut self) -> TextLayout {
        // A collapsed space at the end of the text is removed.
        while self.entries.last().is_some_and(|e| e.collapsed) {
            self.entries.pop();
        }
        let entries = std::mem::take(&mut self.entries);
        let n = entries.len();
        for r in &mut self.ranges {
            r.1 = r.1.min(n);
            r.2 = r.2.min(n);
        }
        let styles: Vec<Rc<ElStyle>> = entries.iter().map(|e| self.style_of(e.el)).collect();

        // Advances, ascents and descents.
        let mut adv = vec![0.0f64; n];
        let mut ascent = vec![0.0f64; n];
        let mut descent = vec![0.0f64; n];
        let mut i = 0;
        while i < n {
            let mut j = i;
            while j < n && entries[j].node == entries[i].node && Rc::ptr_eq(&styles[j], &styles[i]) {
                j += 1;
            }
            self.measure(&entries[i..j], &styles[i], &mut adv[i..j], &mut ascent[i..j], &mut descent[i..j]);
            i = j;
        }

        // Resolved positions: the lists of every element, a descendant's value over its ancestor's.
        let mut xs: Vec<Option<f64>> = vec![None; n];
        let mut ys: Vec<Option<f64>> = vec![None; n];
        let mut dxs: Vec<Option<f64>> = vec![None; n];
        let mut dys: Vec<Option<f64>> = vec![None; n];
        let mut rot: Vec<f64> = vec![0.0; n];
        for &el in &self.positioned {
            let (s, e) = self.range_of(el);
            let (w, h) = self.percent_base(el);
            let xl = self.lengths(el, "x", w);
            let yl = self.lengths(el, "y", h);
            let dxl = self.lengths(el, "dx", w);
            let dyl = self.lengths(el, "dy", h);
            let rl = self.numbers(el, "rotate");
            let mut k = 0usize;
            for i in s..e {
                if entries[i].cont {
                    continue;
                }
                if let Some(&v) = xl.get(k) {
                    xs[i] = Some(v);
                }
                if let Some(&v) = yl.get(k) {
                    ys[i] = Some(v);
                }
                if let Some(&v) = dxl.get(k) {
                    dxs[i] = Some(v);
                }
                if let Some(&v) = dyl.get(k) {
                    dys[i] = Some(v);
                }
                if let Some(&last) = rl.last() {
                    rot[i] = rl.get(k).copied().unwrap_or(last);
                }
                k += 1;
            }
        }

        // The text position moves along the inline axis; an absolute `x` or `y` starts a text chunk.
        let mut pos = vec![(0.0f64, 0.0f64); n];
        let mut chunk = vec![false; n];
        let (mut cx, mut cy) = (0.0f64, 0.0f64);
        for i in 0..n {
            if entries[i].cont {
                pos[i] = pos[i - 1];
                continue;
            }
            let mut starts = i == 0;
            if let Some(x) = xs[i] {
                cx = x;
                starts = true;
            }
            if let Some(y) = ys[i] {
                cy = y;
                starts = true;
            }
            cx += dxs[i].unwrap_or(0.0);
            cy += dys[i].unwrap_or(0.0);
            pos[i] = (cx, cy);
            chunk[i] = starts;
            let st = &styles[i];
            if st.vertical {
                cy += if st.backward { -adv[i] } else { adv[i] };
            } else {
                cx += adv[i];
            }
        }

        // `textLength`, outer elements first.
        for &el in &self.adjusted {
            let (s, e) = self.range_of(el);
            let Some(target) = self.text_length(el) else { continue };
            let chars: Vec<usize> = (s..e).filter(|&i| !entries[i].cont).collect();
            let (Some(&first), Some(&last)) = (chars.first(), chars.last()) else { continue };
            let vertical = styles[first].vertical;
            let sign = if styles[first].backward { -1.0 } else { 1.0 };
            let inl = |p: (f64, f64)| if vertical { p.1 } else { p.0 };
            let set = |p: &mut (f64, f64), v: f64| {
                if vertical {
                    p.1 = v
                } else {
                    p.0 = v
                }
            };
            let origin = inl(pos[first]);
            let extent = (inl(pos[last]) + sign * adv[last] - origin) * sign;
            if !(extent > 0.0) {
                continue;
            }
            let spacing = dom_length_adjust(self.dom, el) != 2;
            let shift_after;
            if spacing {
                if chars.len() < 2 {
                    continue;
                }
                let delta = (target - extent) / (chars.len() - 1) as f64;
                for (k, &i) in chars.iter().enumerate() {
                    let v = inl(pos[i]) + sign * delta * k as f64;
                    set(&mut pos[i], v);
                }
                shift_after = sign * delta * (chars.len() - 1) as f64;
            } else {
                let scale = target / extent;
                for &i in &chars {
                    let v = origin + (inl(pos[i]) - origin) * scale;
                    set(&mut pos[i], v);
                    adv[i] *= scale;
                }
                shift_after = sign * (target - extent);
            }
            // What follows in the same chunk keeps its distance from the end.
            let mut i = e;
            while i < n && !chunk[i] {
                if !entries[i].cont {
                    let v = inl(pos[i]) + shift_after;
                    set(&mut pos[i], v);
                }
                i += 1;
            }
        }

        // `text-anchor` per chunk.
        let mut a = 0;
        while a < n {
            let mut b = a + 1;
            while b < n && !chunk[b] {
                b += 1;
            }
            let anchor = styles[a].anchor;
            if anchor != 0 {
                let mut last = b - 1;
                while last > a && entries[last].cont {
                    last -= 1;
                }
                let st = &styles[a];
                let sign = if st.backward { -1.0 } else { 1.0 };
                let inl = |p: (f64, f64)| if st.vertical { p.1 } else { p.0 };
                let total = inl(pos[last]) + sign * adv[last] - inl(pos[a]);
                let shift = if anchor == 1 { -total / 2.0 } else { -total };
                for p in &mut pos[a..b] {
                    if st.vertical {
                        p.1 += shift;
                    } else {
                        p.0 += shift;
                    }
                }
            }
            a = b;
        }
        // The marks and second halves of pairs sit where their cluster does.
        for i in 0..n {
            if entries[i].cont {
                pos[i] = pos[i - 1];
            }
        }

        let chars = (0..n)
            .map(|i| {
                let st = &styles[i];
                let shift = match st.baseline.as_str() {
                    "hanging" => ascent[i] * 0.8,
                    "middle" => st.size * 0.25,
                    "central" => (ascent[i] - descent[i]) / 2.0,
                    "mathematical" => ascent[i] * 0.5,
                    "text-before-edge" | "text-top" => ascent[i],
                    "text-after-edge" | "text-bottom" | "ideographic" | "bottom" => -descent[i],
                    _ => 0.0,
                };
                CharBox {
                    node: entries[i].node,
                    offset: entries[i].offset,
                    cont: entries[i].cont,
                    start: pos[i],
                    adv: adv[i],
                    ascent: ascent[i],
                    descent: descent[i],
                    rotate: rot[i],
                    vertical: st.vertical,
                    backward: st.backward,
                    shift,
                }
            })
            .collect();
        TextLayout { chars, ranges: self.ranges }
    }

    fn range_of(&self, el: NodeId) -> (usize, usize) {
        self.ranges.iter().find(|(e, _, _)| *e == el).map_or((0, 0), |&(_, s, e)| (s, e))
    }

    /// Shapes the characters of one text node with one style: advance per character (on its first code
    /// unit), and the font's ascent and descent.
    fn measure(&self, group: &[Entry], st: &ElStyle, adv: &mut [f64], ascent: &mut [f64], descent: &mut [f64]) {
        let chars: Vec<(usize, char)> = group.iter().enumerate().filter(|(_, e)| !e.cont).map(|(k, e)| (k, e.ch)).collect();
        if chars.is_empty() || st.size <= 0.0 {
            return;
        }
        let (key, faces) = crate::canvas::canvas_web_faces(self.page, &st.family);
        let style = TextStyle {
            family: &st.family,
            size: st.size as f32,
            weight: st.weight,
            italic: st.italic,
            letter_spacing: st.letter_spacing,
            word_spacing: st.word_spacing,
            lang: None,
            variant_caps: "normal",
            kerning: true,
        };
        let whole: String = chars.iter().map(|(_, c)| *c).collect();
        let full = layout_with_faces(&whole, &style, key, &faces);
        for k in 0..group.len() {
            ascent[k] = full.ascent;
            descent[k] = full.descent;
        }
        let mut starts: Vec<usize> = Vec::new();
        for (ci, (_, c)) in chars.iter().enumerate() {
            if ci == 0 || !joins(chars[ci - 1].1, *c) {
                starts.push(ci);
            }
        }
        // Prefixes are shaped again for each cluster: quadratic, so a long run measures per cluster.
        let prefixes = chars.len() <= 400;
        let mut prefix = String::new();
        let mut previous = 0.0f64;
        for (ki, &cs) in starts.iter().enumerate() {
            let ce = starts.get(ki + 1).copied().unwrap_or(chars.len());
            let cluster: String = chars[cs..ce].iter().map(|(_, c)| *c).collect();
            prefix.push_str(&cluster);
            let width = if ce == chars.len() {
                full.width
            } else if prefixes {
                layout_with_faces(&prefix, &style, key, &faces).width
            } else {
                previous + layout_with_faces(&cluster, &style, key, &faces).width
            };
            let mut advance = width - previous;
            previous = width;
            if st.vertical && (st.upright || (st.mixed && is_upright_char(chars[cs].1))) {
                advance = st.size + f64::from(st.letter_spacing);
            }
            adv[chars[cs].0] = advance;
        }
    }

    /// The nearest `svg`'s viewport size, which percentages of the text's lengths are of.
    fn percent_base(&self, el: NodeId) -> (f64, f64) {
        let dom = self.dom;
        let mut cur = Some(el);
        while let Some(c) = cur {
            if svg_local(dom, c).as_deref() == Some("svg") {
                if let Some(vb) = dom.get_attribute(c, "viewBox").ok().flatten() {
                    let v: Vec<f64> = vb.split(|ch: char| ch.is_whitespace() || ch == ',').filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
                    if v.len() == 4 {
                        return (v[2], v[3]);
                    }
                }
                let side = |name: &str, fallback: f64| {
                    dom.get_attribute(c, name).ok().flatten().and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok()).unwrap_or(fallback)
                };
                let viewport = self.layout.style().viewport();
                return (side("width", f64::from(viewport.width)), side("height", f64::from(viewport.height)));
            }
            cur = dom.parent_element(c).ok().flatten();
        }
        let viewport = self.layout.style().viewport();
        (f64::from(viewport.width), f64::from(viewport.height))
    }

    fn length_context(&self, el: NodeId) -> LengthContext {
        let (dom, style) = (self.dom, self.layout.style());
        let root = dom.node_document(el).ok().and_then(|doc| dom.document_element(doc).ok().flatten());
        LengthContext {
            viewport_width: f64::from(style.viewport().width),
            viewport_height: f64::from(style.viewport().height),
            ..LengthContext::fonts(style.computed_font_size(dom, Some(el)), style.computed_font_size(dom, root))
        }
    }

    /// A list of lengths in user units (a percentage of `base`); empty when absent or when any item is
    /// not a length.
    fn lengths(&self, el: NodeId, name: &str, base: f64) -> Vec<f64> {
        let Some(value) = self.dom.get_attribute(el, name).ok().flatten() else { return Vec::new() };
        let cx = self.length_context(el);
        let mut out = Vec::new();
        for item in value.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()) {
            match length(item, &cx, base) {
                Some(v) => out.push(v),
                None => return Vec::new(),
            }
        }
        out
    }

    fn numbers(&self, el: NodeId, name: &str) -> Vec<f64> {
        let Some(value) = self.dom.get_attribute(el, name).ok().flatten() else { return Vec::new() };
        let mut out = Vec::new();
        for item in value.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()) {
            match item.parse::<f64>() {
                Ok(n) if n.is_finite() => out.push(n),
                _ => return Vec::new(),
            }
        }
        out
    }

    /// `textLength` as a positive length.
    fn text_length(&self, el: NodeId) -> Option<f64> {
        let value = self.dom.get_attribute(el, "textLength").ok().flatten()?;
        let (w, _) = self.percent_base(el);
        let v = length(value, &self.length_context(el), w)?;
        (v > 0.0).then_some(v)
    }
}

/// One `<length>` in user units: a plain number, or a CSS length / percentage of `base`.
fn length(item: &str, cx: &LengthContext, base: f64) -> Option<f64> {
    let item = item.trim();
    let v = match item.parse::<f64>() {
        Ok(n) => n,
        Err(_) => {
            let (px, pct) = resolve_len(item, cx)?;
            px + pct / 100.0 * base
        }
    };
    v.is_finite().then_some(v)
}

/// `lengthAdjust`: 1 `spacing` (also for a missing or invalid value), 2 `spacingAndGlyphs`.
fn dom_length_adjust(dom: &Dom, el: NodeId) -> u8 {
    match dom.get_attribute(el, "lengthAdjust").ok().flatten() {
        Some("spacingAndGlyphs") => 2,
        _ => 1,
    }
}
