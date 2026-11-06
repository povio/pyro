//! `HTMLElement.innerText` / `outerText`: port of upstream's rendered-text
//! collection (`src/browser/webapi/element/Html.zig` @ 2ad38773, line
//! references below are to that file):
//! - getter: `getInnerText` (335-359), `InnerTextState` (683-742),
//!   `collectInnerText` (746-810), `handleChildElement` (814-881),
//!   `enterBlock` (884-891), `innerTextDisplay` (893-915),
//!   `isInnerTextBlockTag` (917-925, with `Element.Tag.isBlock`,
//!   webapi/Element.zig:2594-2615), `writeText` / `writeRenderedText`
//!   (936-998) with `CData.render` (webapi/CData.zig:247-302) and the
//!   length-preserving case maps `applyTextTransform` / `upperCodepoint` /
//!   `lowerCodepoint` (1000-1054);
//! - setters: `setInnerText` (361-364), `setOuterText` (366-405),
//!   `mergeTextNodes` (1056-1072), `renderedTextFragment` (1074-1096).
//!
//! Display decisions come from [`StyleEngine`]: an element's own
//! display:none, inherited `visibility: hidden`, own `text-transform`. As
//! upstream, `capitalize` is reported by getComputedStyle but not applied.
//! `white-space` comes from the computed style of each element.

use markup5ever::ns;

use crate::dom::Dom;
use crate::error::{DomError, DomErrorKind, DomResult};
use crate::node::{NodeId, NodeKind, NIL};
use crate::style::{CheckVisibilityOptions, StyleEngine, TextTransform, WhiteSpace};

#[derive(Copy, Clone, PartialEq, Eq)]
enum TextDisplay {
    Skip,
    Replaced,
    Inline,
    Block,
    BlockP,
    Transparent,
    TableCell,
    TableRow,
}

/// innerTextDisplay (Html.zig:893-915), by HTML local name.
fn inner_text_display(tag: &str) -> TextDisplay {
    use TextDisplay::*;
    match tag {
        "br" => Inline, // handled before this point
        "p" => BlockP,
        "td" | "th" => TableCell,
        "tr" => TableRow,
        "caption" => Block,
        // Row groups / column groups are transparent: no break of their own.
        "thead" | "tbody" | "tfoot" => Transparent,
        "col" | "colgroup" => Skip,
        "option" | "optgroup" => Block,
        // Replaced / embedded content whose subtree is not part of innerText.
        "img" | "canvas" | "iframe" | "embed" | "object" | "audio" | "video" | "input" | "textarea" => Replaced,
        // Never-rendered elements (UA display:none / not parsed as content).
        "script" | "style" | "template" | "head" | "base" | "link" | "meta" | "title" | "source" | "track"
        | "param" | "area" | "datalist" | "noscript" => Skip,
        t if is_inner_text_block_tag(t) => Block,
        _ => Inline,
    }
}

/// isInnerTextBlockTag (Html.zig:917-925) + Tag.isBlock (Element.zig:2594-2615).
fn is_inner_text_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "article"
            | "aside"
            | "footer"
            | "header"
            | "main"
            | "nav"
            | "section"
            | "address"
            | "div"
            | "fieldset"
            | "figure"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "dl"
            | "ol"
            | "ul"
            | "blockquote"
            | "pre"
            | "table"
            | "hr"
            | "summary"
            | "details"
            | "dialog"
            | "menu"
            | "dd"
            | "dt"
            | "form"
            | "li"
    )
}

/// InnerTextState (Html.zig:683-742).
struct State<'a> {
    out: String,
    style: &'a StyleEngine,
    /// Line breaks accumulated for the block, emitted lazily (max wins).
    pending_breaks: u8,
    /// Suppresses leading line breaks while nothing has been written.
    wrote_any: bool,
    /// A collapsible space is pending from a previous text run.
    pre_w: bool,
    /// Trim leading whitespace of the next text run.
    trim_left: bool,
    /// Computed white-space of the element whose text is being written.
    white_space: WhiteSpace,
    transform: TextTransform,
}

impl State<'_> {
    fn require_breaks(&mut self, n: u8) {
        if n > self.pending_breaks {
            self.pending_breaks = n;
        }
    }

    fn flush_breaks(&mut self) {
        if self.pending_breaks == 0 {
            return;
        }
        if self.wrote_any {
            for _ in 0..self.pending_breaks {
                self.out.push('\n');
            }
        }
        self.pending_breaks = 0;
        // A block boundary trims the whitespace on either side of it.
        self.pre_w = false;
        self.trim_left = true;
    }

    /// A literal separator (table cell tab / row newline).
    fn write_separator(&mut self, ch: char) {
        self.flush_breaks();
        self.out.push(ch);
        self.wrote_any = true;
        self.pre_w = false;
        self.trim_left = true;
    }
}

fn is_ascii_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_all_ascii_whitespace(s: &str) -> bool {
    s.bytes().all(is_ascii_ws)
}

/// `CData.render` (CData.zig:254-302) with `trim_right = true`: collapses
/// whitespace runs to one space; returns whether `s` ends with a trimmed
/// whitespace run.
fn render_collapsed(s: &str, trim_left: bool, out: &mut String) -> bool {
    let b = s.as_bytes();
    let mut start = 0;
    let mut prev_w: Option<bool> = None;
    let mut is_w = false;
    for (i, &c) in b.iter().enumerate() {
        is_w = is_ascii_ws(c);
        let p = *prev_w.get_or_insert(is_w);
        if p == is_w {
            continue;
        }
        if is_w {
            out.push_str(&s[start..i]);
        } else if start > 0 || !trim_left {
            out.push(' ');
        }
        prev_w = Some(is_w);
        start = i;
    }
    if is_w {
        return true;
    }
    out.push_str(&s[start..]);
    false
}

/// upperCodepoint (Html.zig:1027-1040): length-preserving simple mappings.
fn upper_codepoint(cp: u32) -> u32 {
    match cp {
        0x61..=0x7a => cp - 0x20,
        0xE0..=0xF6 | 0xF8..=0xFE => cp - 0x20,
        0xFF => 0x178,
        0x101..=0x12F | 0x133..=0x137 | 0x14B..=0x177 => {
            if cp & 1 == 1 {
                cp - 1
            } else {
                cp
            }
        }
        0x13A..=0x148 | 0x17A..=0x17E => {
            if cp & 1 == 0 {
                cp - 1
            } else {
                cp
            }
        }
        0x3B1..=0x3C1 | 0x3C3..=0x3C9 => cp - 0x20,
        0x3C2 => 0x3A3, // final sigma
        0x430..=0x44F => cp - 0x20,
        0x450..=0x45F => cp - 0x50,
        _ => cp,
    }
}

/// lowerCodepoint (Html.zig:1042-1054).
fn lower_codepoint(cp: u32) -> u32 {
    match cp {
        0x41..=0x5a => cp + 0x20,
        0xC0..=0xD6 | 0xD8..=0xDE => cp + 0x20,
        0x178 => 0xFF,
        0x100..=0x12E | 0x132..=0x136 | 0x14A..=0x176 => {
            if cp & 1 == 0 {
                cp + 1
            } else {
                cp
            }
        }
        0x139..=0x147 | 0x179..=0x17D => {
            if cp & 1 == 1 {
                cp + 1
            } else {
                cp
            }
        }
        0x391..=0x3A1 | 0x3A3..=0x3A9 => cp + 0x20,
        0x410..=0x42F => cp + 0x20,
        0x400..=0x40F => cp + 0x50,
        _ => cp,
    }
}

/// applyTextTransform (Html.zig:1001-1022).
fn apply_text_transform(text: &str, transform: TextTransform) -> String {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            let mapped = match transform {
                TextTransform::Uppercase => upper_codepoint(cp),
                TextTransform::Lowercase => lower_codepoint(cp),
                _ => cp,
            };
            char::from_u32(mapped).unwrap_or(c)
        })
        .collect()
}

impl Dom {
    /// `HTMLElement.innerText` (and `outerText`) getter: getInnerText
    /// (Html.zig:335-359). An element that isn't being rendered (no owner
    /// frame, disconnected, or in a display:none subtree) returns its
    /// `textContent`.
    pub fn inner_text(&self, style: &StyleEngine, el: NodeId) -> DomResult<String> {
        let idx = self.check(el)?;
        let Some(e) = self.n(idx).element() else { return Err(DomError::Type("not an element")) };
        let text_content = || self.text_content(el).map(Option::unwrap_or_default);
        if self.n(idx).owner_doc != style.document().idx {
            return text_content();
        }
        if !self.is_connected(el)? || style.is_hidden(self, el, CheckVisibilityOptions::default()) {
            return text_content();
        }
        let tag: &str = &e.name.local;
        if matches!(inner_text_display(tag), TextDisplay::Skip | TextDisplay::Replaced) {
            return Ok(String::new());
        }
        let mut state = State {
            out: String::new(),
            style,
            pending_breaks: 0,
            wrote_any: false,
            pre_w: false,
            trim_left: true,
            white_space: style.computed(self, el).map_or(WhiteSpace::Collapse, |c| c.white_space),
            transform: style.text_transform(self, el),
        };
        self.collect_inner_text(idx, &mut state);
        Ok(state.out)
    }

    /// collectInnerText (Html.zig:746-810).
    fn collect_inner_text(&self, idx: u32, state: &mut State<'_>) {
        let e = self.n(idx).element().expect("element");
        let self_tag: &str = &e.name.local;
        #[derive(PartialEq)]
        enum ChildFilter {
            None,
            Select,
            Optgroup,
        }
        let child_filter = match self_tag {
            "select" => ChildFilter::Select,
            "optgroup" => ChildFilter::Optgroup,
            _ => ChildFilter::None,
        };
        // Inside table containers, inter-cell/row whitespace text is not rendered.
        let table_ctx = matches!(self_tag, "table" | "tbody" | "thead" | "tfoot" | "tr");
        let mut saw_row = false;
        let mut saw_cell = false;
        // The text of an invisible element isn't included, but its children
        // are still walked: they can make themselves visible again.
        let text_hidden = state.style.has_visibility_hidden_inherited(self, self.id_of(idx));

        let mut c = self.n(idx).first_child;
        while c != NIL {
            match &self.n(c).kind {
                NodeKind::Element(ce) => {
                    if ce.name.ns != ns!(svg) {
                        let tag: &str = &ce.name.local;
                        let keep = match child_filter {
                            ChildFilter::None => true,
                            ChildFilter::Select => tag == "option" || tag == "optgroup",
                            ChildFilter::Optgroup => tag == "option",
                        };
                        if keep {
                            self.handle_child_element(c, tag, state, &mut saw_cell, &mut saw_row);
                        }
                    }
                }
                NodeKind::Text(s) => {
                    // Text directly inside <select>/<optgroup> is skipped, as
                    // is visibility:hidden text.
                    let skip =
                        child_filter != ChildFilter::None || text_hidden || (table_ctx && is_all_ascii_whitespace(s));
                    if !skip {
                        write_text(s, state);
                    }
                }
                _ => {}
            }
            c = self.n(c).next;
        }
    }

    /// handleChildElement (Html.zig:814-881). Only the child's own
    /// display:none matters: the root's ancestors were checked already.
    fn handle_child_element(
        &self,
        idx: u32,
        tag: &str,
        state: &mut State<'_>,
        saw_cell: &mut bool,
        saw_row: &mut bool,
    ) {
        let id = self.id_of(idx);
        // The layout engine's UA sheet hides a select's options (Blitz paints
        // the control itself); Chromium lays them out as blocks.
        let parent = self.n(idx).parent;
        let in_select = matches!(tag, "option" | "optgroup")
            && self.n(parent).element().is_some_and(|p| matches!(&*p.name.local, "select" | "optgroup"));
        if !in_select && state.style.has_display_none(self, id) {
            return;
        }
        if tag == "br" {
            state.flush_breaks();
            state.out.push('\n');
            state.wrote_any = true;
            state.pre_w = false;
            state.trim_left = true;
            return;
        }
        let display = inner_text_display(tag);
        if display == TextDisplay::Skip {
            return;
        }
        let computed = state.style.computed(self, id);
        // An atomic inline box (replaced element, inline-block and the like)
        // keeps the collapsible spaces on both of its sides, as Chrome's
        // rendered text does.
        let atomic_inline =
            display == TextDisplay::Replaced || computed.is_some_and(|c| c.author_display && c.atomic_inline);
        if atomic_inline {
            // The space before the box renders; its contents start afresh.
            // A replaced element writes nothing itself, so the line breaks
            // pending before it stay pending (and are dropped at the end).
            if display != TextDisplay::Replaced {
                state.flush_breaks();
            }
            if state.pre_w {
                state.out.push(' ');
                state.pre_w = false;
            }
            state.trim_left = true;
        }
        if display == TextDisplay::Replaced {
            state.trim_left = false;
            return;
        }
        // text-transform inherits, so a child's own value covers its subtree only.
        let parent_transform = state.transform;
        let parent_white_space = state.white_space;
        let own = computed.map_or(TextTransform::Inherit, |c| c.text_transform);
        if own != TextTransform::Inherit {
            state.transform = own;
        }
        state.white_space = computed.map_or(parent_white_space, |c| c.white_space);
        match display {
            TextDisplay::Skip | TextDisplay::Replaced => unreachable!(),
            TextDisplay::Inline | TextDisplay::Transparent if atomic_inline => {
                self.collect_inner_text(idx, state);
                // A trailing space inside the box collapses; the one after it renders.
                state.pre_w = false;
                state.trim_left = false;
            }
            TextDisplay::Inline | TextDisplay::Transparent => self.collect_inner_text(idx, state),
            TextDisplay::Block => {
                state.require_breaks(1);
                self.collect_inner_text(idx, state);
                state.require_breaks(1);
            }
            TextDisplay::BlockP => {
                state.require_breaks(2);
                self.collect_inner_text(idx, state);
                state.require_breaks(2);
            }
            TextDisplay::TableCell => {
                if *saw_cell {
                    state.write_separator('\t');
                }
                *saw_cell = true;
                self.collect_inner_text(idx, state);
            }
            TextDisplay::TableRow => {
                if *saw_row {
                    state.write_separator('\n');
                }
                *saw_row = true;
                self.collect_inner_text(idx, state);
            }
        }
        state.transform = parent_transform;
        state.white_space = parent_white_space;
    }

    /// renderedTextFragment (Html.zig:1074-1096): a fragment of Text nodes
    /// with one `<br>` per line break (a CR/LF pair counts once).
    pub fn rendered_text_fragment(&mut self, doc: NodeId, text: &str) -> DomResult<NodeId> {
        let frag = self.create_fragment(doc)?;
        let mut rest = text;
        loop {
            let text_end = rest.find(['\r', '\n']).unwrap_or(rest.len());
            if text_end > 0 {
                let t = self.create_text(doc, &rest[..text_end])?;
                self.append_child(frag, t)?;
            }
            rest = &rest[text_end..];
            if rest.is_empty() {
                return Ok(frag);
            }
            let break_len = if rest.starts_with("\r\n") { 2 } else { 1 };
            let br = self.create_element_ns(doc, Some("http://www.w3.org/1999/xhtml"), "br")?;
            self.append_child(frag, br)?;
            rest = &rest[break_len..];
        }
    }

    /// `innerText = text` (Html.zig:361-364).
    pub fn set_inner_text(&mut self, el: NodeId, text: &str) -> DomResult<()> {
        let doc = self.node_document(el)?;
        let frag = self.rendered_text_fragment(doc, text)?;
        self.replace_all(el, Some(frag))
    }

    /// `outerText = text` (Html.zig:366-405): replaces the element with the
    /// rendered fragment and merges it with the neighbouring Text nodes.
    pub fn set_outer_text(&mut self, el: NodeId, text: &str) -> DomResult<()> {
        let idx = self.check(el)?;
        if !self.n(idx).element().is_some_and(|e| e.is_html()) {
            return Ok(());
        }
        let p = self.n(idx).parent;
        if p == NIL {
            return Err(DomError::new(DomErrorKind::NoModificationAllowed, "element has no parent"));
        }
        let parent = self.id_of(p);
        let prev = self.opt_id(self.n(idx).prev);
        let next = self.opt_id(self.n(idx).next);
        let doc = self.node_document(el)?;
        let frag = self.rendered_text_fragment(doc, text)?;
        if self.first_child(frag)?.is_none() {
            // A fragment with no node still replaces the element with an
            // empty Text node so surrounding text can merge with it.
            let t = self.create_text(doc, "")?;
            self.append_child(frag, t)?;
        }
        self.replace_child(parent, frag, el)?;

        let first = match prev {
            Some(pv) => self.next_sibling(pv)?,
            None => self.first_child(parent)?,
        };
        let mut last = match next {
            Some(nx) => self.prev_sibling(nx)?,
            None => self.last_child(parent)?,
        };
        if let (Some(pv), Some(f)) = (prev, first) {
            if self.merge_text_nodes(pv, f)? && Some(f) == last {
                last = Some(pv);
            }
        }
        if let (Some(nx), Some(l)) = (next, last) {
            self.merge_text_nodes(l, nx)?;
        }
        Ok(())
    }

    /// mergeTextNodes (Html.zig:1056-1072).
    fn merge_text_nodes(&mut self, left: NodeId, right: NodeId) -> DomResult<bool> {
        let (l, r) = (self.check(left)?, self.check(right)?);
        let (NodeKind::Text(a), NodeKind::Text(b)) = (&self.n(l).kind, &self.n(r).kind) else { return Ok(false) };
        let merged = format!("{a}{b}");
        self.set_char_data(left, &merged)?;
        self.remove_node(right)?;
        Ok(true)
    }
}

/// writeText (Html.zig:936-956).
fn write_text(s: &str, state: &mut State<'_>) {
    // `capitalize` is recognized but not applied: it needs word-boundary
    // tracking across text runs.
    if state.transform != TextTransform::Uppercase && state.transform != TextTransform::Lowercase {
        write_rendered_text(s, state);
        return;
    }
    let mark = state.out.len();
    write_rendered_text(s, state);
    let mapped = apply_text_transform(&state.out[mark..], state.transform);
    state.out.truncate(mark);
    state.out.push_str(&mapped);
}

/// writeRenderedText (Html.zig:958-998).
fn write_rendered_text(s: &str, state: &mut State<'_>) {
    if state.white_space == WhiteSpace::PreserveBreaks && s.contains(['\n', '\r']) {
        // Each line collapses on its own; spaces around a newline are dropped.
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in normalized.split('\n').enumerate() {
            if i > 0 {
                state.flush_breaks();
                state.out.push('\n');
                state.wrote_any = true;
                state.pre_w = false;
                state.trim_left = true;
            }
            if !line.is_empty() {
                write_collapsed(line, state);
            }
        }
        return;
    }
    if state.white_space == WhiteSpace::Preserve {
        if s.is_empty() {
            return;
        }
        state.flush_breaks();
        // A collapsible space from a previous run still renders before this one.
        if state.pre_w {
            state.out.push(' ');
        }
        // Preserve whitespace verbatim, normalising CR / CRLF to LF.
        let mut chars = s.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                state.out.push('\n');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            } else {
                state.out.push(ch);
            }
        }
        state.wrote_any = true;
        state.pre_w = false;
        state.trim_left = false;
        return;
    }
    write_collapsed(s, state);
}

fn write_collapsed(s: &str, state: &mut State<'_>) {
    if is_all_ascii_whitespace(s) {
        // A pending space stays pending across whitespace-only runs.
        state.pre_w |= state.wrote_any && !state.trim_left;
        return;
    }
    state.flush_breaks();
    // The pending space and this run's leading whitespace collapse into one.
    let trim_left = state.trim_left || state.pre_w;
    if state.pre_w {
        state.out.push(' ');
    }
    state.pre_w = render_collapsed(s, trim_left, &mut state.out);
    state.wrote_any = true;
    state.trim_left = state.pre_w;
}
