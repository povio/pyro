//! Value-based inline editing commands and their collapsed-selection overrides.

use crate::{
    editing::{self, Point},
    page::PageDom,
};
use pyro_dom::{stylo::declarations::DeclarationBlock, Dom, NodeId, NodeType};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Family,
    Size,
    Foreground,
    Background,
}

impl Kind {
    pub(super) fn command(s: &str) -> Option<Self> {
        Some(match s {
            "fontname" => Self::Family,
            "fontsize" => Self::Size,
            "forecolor" => Self::Foreground,
            "backcolor" | "hilitecolor" => Self::Background,
            _ => return None,
        })
    }
    fn property(self) -> &'static str {
        match self {
            Self::Family => "font-family",
            Self::Size => "font-size",
            Self::Foreground => "color",
            Self::Background => "background-color",
        }
    }
    fn input_type(self) -> &'static str {
        match self {
            Self::Family => "formatFontName",
            Self::Size => "",
            Self::Foreground => "formatFontColor",
            Self::Background => "formatBackColor",
        }
    }
    fn html_attr(self) -> Option<&'static str> {
        match self {
            Self::Family => Some("face"),
            Self::Size => Some("size"),
            Self::Foreground => Some("color"),
            Self::Background => None,
        }
    }
}

/// The size numbers used by the HTML font element and the editing commands.
const SIZES: [&str; 7] = ["x-small", "small", "medium", "large", "x-large", "xx-large", "xxx-large"];

fn font_size(input: &str) -> Option<i32> {
    let s = input.trim_matches(|c: char| c.is_ascii_whitespace());
    pyro_dom::forms::parse_float(s.strip_prefix('+').unwrap_or(s))?;
    let digits = s
        .trim_start_matches(['+', '-'])
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |v, c| (v * 10 + i32::from(c - b'0')).min(10));
    let number = match s.as_bytes().first() {
        Some(b'+') => 3 + digits,
        Some(b'-') => 3 - digits,
        _ => digits,
    };
    Some(number.clamp(1, 7))
}

#[derive(Clone)]
struct Format {
    kind: Kind,
    css: String,
    legacy: Option<String>,
}

impl Format {
    fn parse(kind: Kind, value: &str) -> Option<Self> {
        let (css, legacy) = match kind {
            Kind::Size => {
                let n = font_size(value)?;
                (SIZES[(n - 1) as usize].into(), Some(n.to_string()))
            }
            Kind::Family => {
                let mut decls = DeclarationBlock::parse("");
                decls.set("font-family", value, false)?;
                (decls.value("font-family"), Some(value.to_owned()))
            }
            Kind::Foreground | Kind::Background => {
                let (color, html) = pyro_dom::color_well::editing_color(value)
                    .or_else(|| pyro_dom::color_well::editing_color(&format!("#{value}")))?;
                (color, if kind == Kind::Foreground { html } else { None })
            }
        };
        Some(Self { kind, css, legacy })
    }
    fn query(&self) -> String {
        if self.kind == Kind::Size {
            self.legacy.clone().unwrap_or_default()
        } else {
            self.css.clone()
        }
    }
    fn legacy(&self, use_css: bool) -> Option<(&'static str, &str)> {
        if use_css && !(self.kind == Kind::Size && self.legacy.as_deref() == Some("7")) {
            return None;
        }
        Some((self.kind.html_attr()?, self.legacy.as_deref()?))
    }
}

pub(crate) struct TypingValues {
    range: (Point, Point),
    values: Vec<Format>,
    decoration: Option<super::decoration::Lines>,
    toggles: Vec<(&'static str, bool)>,
}

fn pending_at(slot: &mut Option<TypingValues>, range: (Point, Point)) -> &mut TypingValues {
    if slot.as_ref().is_none_or(|v| v.range != range) {
        *slot = Some(TypingValues { range, values: Vec::new(), decoration: None, toggles: Vec::new() });
    }
    slot.as_mut().unwrap()
}

pub(super) fn toggle_state(page: &PageDom, range: (Point, Point), tag: &str) -> Option<bool> {
    let slot = page.typing_values.borrow();
    slot.as_ref().filter(|v| v.range == range)?.toggles.iter().find(|(t, _)| *t == tag).map(|(_, on)| *on)
}

pub(super) fn decoration_state(page: &PageDom, range: (Point, Point), tag: &str) -> Option<bool> {
    let slot = page.typing_values.borrow();
    let pending = slot.as_ref().filter(|p| p.range == range)?.decoration?;
    let authored = super::decoration::Lines::at(page, &page.dom.borrow(), range.0.0);
    Some(pending.reported(authored).has(tag))
}

pub(super) fn toggle_decoration(page: &PageDom, range: (Point, Point), tag: &str) {
    let authored = super::decoration::Lines::at(page, &page.dom.borrow(), range.0.0);
    let mut slot = page.typing_values.borrow_mut();
    let pending = pending_at(&mut slot, range);
    let mut lines = pending.decoration.map_or(authored, |d| d.reported(authored));
    lines.toggle(tag);
    pending.decoration = Some(lines);
}

pub(super) fn set_toggle(page: &PageDom, range: (Point, Point), tag: &'static str, enabled: bool) {
    let mut slot = page.typing_values.borrow_mut();
    let pending = pending_at(&mut slot, range);
    if matches!(tag, "sub" | "sup") {
        pending.toggles.retain(|(t, _)| !matches!(*t, "sub" | "sup"));
        pending.toggles.push((if tag == "sub" { "sup" } else { "sub" }, false));
    } else {
        pending.toggles.retain(|(t, _)| *t != tag);
    }
    pending.toggles.push((tag, enabled));
}

/// Structural commands keep the formatting values attached to the selection
/// while its live endpoints move with the edited paragraphs.
pub(super) fn reselect(page: &Rc<PageDom>, original: (Point, Point), points: [Point; 2]) {
    let pending = page.typing_values.take().filter(|v| v.range == original);
    editing::retarget_selection(page, points[0], points[1]);
    if let (Some(mut pending), Some(range)) = (pending, editing::selection_range(page)) {
        pending.range = range;
        page.typing_values.replace(Some(pending));
    }
}

/// Fully selected text may lose its formatting element on Enter. Preserve
/// its color and font emphasis for the next insertion at the break caret.
pub(crate) fn retain_replacement_style(
    page: &PageDom, dom: &Dom, range: (Point, Point), text: NodeId, wrapper: NodeId,
) {
    let Some(parent) = dom.parent_element(wrapper).ok().flatten() else { return };
    let Some(element) = dom.parent_element(text).ok().flatten() else { return };
    for kind in [Kind::Foreground, Kind::Background] {
        let color = effective(page, dom, kind, text);
        if color == effective(page, dom, kind, parent) { continue; }
        if let Some(format) = Format::parse(kind, &color) {
            let mut slot = page.typing_values.borrow_mut();
            let pending = pending_at(&mut slot, range);
            if !pending.values.iter().any(|f| f.kind == kind) { pending.values.push(format); }
        }
    }
    let layout = page.layout();
    for (tag, property) in [("b", "font-weight"), ("i", "font-style")] {
        let enabled = |node| {
            let value = layout.computed_style_value(dom, node, property);
            if tag == "b" { super::numeric_bold_weight(&value) }
            else { value.starts_with("italic") || value.starts_with("oblique") }
        };
        let active = enabled(element);
        if active != enabled(parent) && toggle_state(page, range, tag).is_none() {
            set_toggle(page, range, tag, active);
        }
    }
}

/// A paragraph or soft break continues typing at the new caret.
pub(crate) fn set_break_caret(page: &Rc<PageDom>, original: (Point, Point), caret: Point) {
    let pending = page.typing_values.take().filter(|v| v.range == original);
    editing::set_selection(page, caret, caret);
    if let Some(mut pending) = pending {
        pending.range = (caret, caret);
        page.typing_values.replace(Some(pending));
    }
}

fn effective(page: &PageDom, dom: &Dom, kind: Kind, node: NodeId) -> String {
    let mut el = if dom.node_type(node).ok() == Some(NodeType::Element) {
        Some(node)
    } else {
        dom.parent_element(node).ok().flatten()
    };
    let layout = page.layout();
    while let Some(n) = el {
        let value = layout.computed_style_value(dom, n, kind.property());
        let parent = dom.parent_element(n).ok().flatten();
        if kind == Kind::Background
            && parent.is_some()
            && (value.is_empty() || pyro_dom::color_well::is_fully_transparent(&value))
        {
            el = parent;
        } else {
            return value;
        }
    }
    String::new()
}

fn formattable_nodes(page: &PageDom, dom: &Dom, scope: NodeId, start: Point, end: Point) -> Vec<NodeId> {
    if start == end {
        return Vec::new();
    }
    let mut nodes = Vec::new();
    for node in dom.descendants(scope).into_iter().flatten() {
        let (begin, finish) = if editing::is_text(dom, node) {
            let len = editing::len(dom, node);
            if len == 0 {
                continue;
            }
            ((node, 0), (node, len))
        } else if matches!(editing::local(dom, node).as_str(), "br" | "img") {
            let (Ok(Some(p)), Ok(index)) = (dom.parent(node), dom.index_in_parent(node)) else {
                continue;
            };
            ((p, index), (p, index + 1))
        } else {
            continue;
        };
        if crate::range::compare_boundary_points_dom(dom, finish, start) > 0
            && crate::range::compare_boundary_points_dom(dom, begin, end) < 0
            && super::rich_node(dom, node)
            && !editing::invisible_formatting_space(page, dom, scope, node)
            && super::alignment::visible(page, dom, node)
        {
            nodes.push(node);
        }
    }
    nodes
}
fn selected_values(page: &PageDom, kind: Kind) -> Vec<String> {
    let Some((start, end)) = editing::selection_range(page) else { return Vec::new() };
    let dom = page.dom.borrow();
    let mut nodes =
        editing::editing_host(&dom, start.0).map(|h| formattable_nodes(page, &dom, h, start, end)).unwrap_or_default();
    if nodes.is_empty() {
        nodes.push(start.0);
    }
    nodes.into_iter().map(|n| effective(page, &dom, kind, n)).collect()
}

// Stylo's absolute size keywords at this browser's default medium size.
const FONT_PX: [f32; 7] = [10., 13., 16., 18., 24., 32., 48.];

fn equivalent(fmt: &Format, computed: &str) -> bool {
    if fmt.css == computed {
        return true;
    }
    if fmt.kind == Kind::Size {
        return SIZES
            .iter()
            .position(|s| *s == fmt.css)
            .zip(computed.strip_suffix("px").and_then(|s| s.parse::<f32>().ok()))
            .is_some_and(|(n, px)| px == FONT_PX[n]);
    }
    false
}

pub(super) fn query(page: &PageDom, kind: Kind) -> String {
    if let Some(range) = editing::selection_range(page) {
        if let Some(v) = page
            .typing_values
            .borrow()
            .as_ref()
            .filter(|v| v.range == range)
            .and_then(|v| v.values.iter().find(|v| v.kind == kind))
        {
            return v.query();
        }
    }
    let Some(value) = selected_values(page, kind).into_iter().next() else { return String::new() };
    if kind != Kind::Size {
        return value;
    }
    let Some(px) = value.strip_suffix("px").and_then(|v| v.parse::<f32>().ok()) else { return String::new() };
    (1 + FONT_PX.windows(2).take_while(|pair| px >= (pair[0] + pair[1]) / 2.).count()).to_string()
}

pub(super) fn mixed(page: &PageDom, kind: Kind) -> bool {
    let values = selected_values(page, kind);
    values.first().is_some_and(|a| values.iter().any(|b| b != a))
}

fn apply_to(dom: &mut Dom, node: NodeId, fmt: &Format, legacy: Option<(&str, &str)>) {
    if let Some((attr, value)) = legacy {
        // A font's presentation attribute has lower cascade priority than
        // its inline declarations. Clear only the property being edited.
        if let Ok(Some(style)) = dom.get_attribute(node, "style") {
            let mut declarations = DeclarationBlock::parse(style);
            if declarations.remove(fmt.kind.property()) {
                if declarations.is_empty() {
                    let _ = dom.remove_attribute(node, "style");
                } else {
                    let _ = dom.set_attribute(node, "style", &declarations.css_text());
                }
            }
        }
        let _ = dom.set_attribute(node, attr, value);
    } else {
        let mut decl = DeclarationBlock::parse(dom.get_attribute(node, "style").ok().flatten().unwrap_or(""));
        decl.set(fmt.kind.property(), &fmt.css, false);
        let _ = dom.set_attribute(node, "style", &decl.css_text());
    }
}

/// A transparent child cannot cover an ancestor's painted background.
/// Split the inline paint-bearing containers at the selected leaf and
/// remove the color only from the selected portion.
fn clear_highlight(page: &PageDom, dom: &mut Dom, node: NodeId) -> bool {
    let Some(host) = editing::editing_host(dom, node) else { return false };
    let mut changed = false;
    let mut cur = dom.parent_element(node).ok().flatten();
    while let Some(el) = cur {
        if el == host {
            break;
        }
        let layout = page.layout();
        let display = layout.computed_style_value(dom, el, "display");
        if display != "inline" && display != "contents" {
            break;
        }
        let parent = dom.parent_element(el).ok().flatten();
        let own = layout.computed_style_value(dom, el, "background-color");
        if !own.is_empty() && !pyro_dom::color_well::is_fully_transparent(&own) {
            let Some(outer) = parent else {
                break;
            };
            let mut edges = [(node, 0), (node, editing::len(dom, node))];
            let Some(mid) = super::alignment::split_edge(dom, outer, node, false, &mut edges) else {
                break;
            };
            if super::alignment::split_edge(dom, outer, node, true, &mut edges).is_none() {
                break;
            }
            let mut style = DeclarationBlock::parse(dom.get_attribute(mid, "style").ok().flatten().unwrap_or(""));
            style.remove("background-color");
            if style.is_empty() {
                let _ = dom.remove_attribute(mid, "style");
            } else {
                let _ = dom.set_attribute(mid, "style", &style.css_text());
            }
            let inherited_rule = layout.computed_style_value(dom, mid, "background-color");
            if !pyro_dom::color_well::is_fully_transparent(&inherited_rule) {
                style.set("background-color", "transparent", false);
                let _ = dom.set_attribute(mid, "style", &style.css_text());
            }
            changed = true;
        }
        cur = parent;
    }
    changed
}

/// Carries formatting on a native element without parsing user input as HTML.
fn wrap(dom: &mut Dom, node: NodeId, fmt: &Format) -> Option<NodeId> {
    let host = editing::editing_host(dom, node)?;
    let doc = dom.node_document(node).ok()?;
    let legacy = fmt.legacy(dom.document_data(doc).ok()?.style_with_css);
    let tag = if legacy.is_some() { "font" } else { "span" };
    let parent = dom.parent_element(node).ok()??;
    if parent != host && editing::local(dom, parent) == tag && dom.child_count(parent).ok()? == 1 {
        apply_to(dom, parent, fmt, legacy);
        return Some(parent);
    }
    let el = dom.create_element(doc, tag).ok()?;
    apply_to(dom, el, fmt, legacy);
    dom.insert_before(parent, el, Some(node)).ok()?;
    dom.append_child(el, node).ok()?;
    Some(el)
}

/// Replace a selected portion of a single-purpose FONT or SPAN. Keep
/// the original value on either side of the range, and leave carriers
/// with unrelated attributes or declarations intact.
fn format_selected(page: &PageDom, dom: &mut Dom, host: NodeId, node: NodeId, range: (Point, Point), fmt: &Format) -> Option<()> {
    let parent = dom.parent_element(node).ok()??;
    if parent != host && dom.element(parent).ok()?.is_html() {
        let tag = editing::local(dom, parent);
        let attrs = dom.attributes(parent).ok()?;
        let carrier = match attrs {
            [attr] if attr.name.ns.is_empty() && tag == "font" => {
                fmt.kind.html_attr() == Some(&*attr.name.local)
            }
            [attr] if tag == "span" && attr.name.local == markup5ever::local_name!("style") => {
                matches!(crate::css::parse_declarations(&attr.value).as_slice(), [decl] if decl.name == fmt.kind.property())
            }
            _ => false,
        };
        if carrier {
            let outer = dom.parent_element(parent).ok()??;
            let inherited = if fmt.kind == Kind::Size {
                // A coincident pixel size is not an inherited HTML size.
                // Stylo retains keyword provenance through the cascade.
                page.layout().style().stylo_style(dom, outer).is_some_and(|style| {
                    let info = style.get_font().font_size.keyword_info;
                    let size = info.kw.html_size().checked_sub(1).and_then(|i| SIZES.get(i as usize));
                    info.factor == 1.0 && info.offset.px() == 0.0 && size == Some(&fmt.css.as_str())
                })
            } else {
                equivalent(fmt, &effective(page, dom, fmt.kind, outer))
            };
            if fmt.kind == Kind::Background && !inherited {
                // Across paint boundaries each existing background remains
                // in place. Within one run the selected paint can be replaced.
                let in_run = |point: Point| point.0 == parent || dom.parent(point.0).ok().flatten() == Some(parent);
                if !in_run(range.0) || !in_run(range.1) {
                    return wrap(dom, node, fmt).map(|_| ());
                }
            }
            let mut edges = [(node, 0), (node, editing::len(dom, node))];
            let middle = super::alignment::split_edge(dom, outer, node, false, &mut edges)?;
            super::alignment::split_edge(dom, outer, node, true, &mut edges)?;
            let doc = dom.node_document(node).ok()?;
            let legacy = fmt.legacy(dom.document_data(doc).ok()?.style_with_css);
            if !inherited && tag == if legacy.is_some() { "font" } else { "span" } {
                apply_to(dom, middle, fmt, legacy);
                return Some(());
            }
            super::indentation::unwrap(dom, middle, &mut edges)?;
            if inherited { return Some(()); }
        }
    }
    wrap(dom, node, fmt).map(|_| ())
}

pub(super) fn apply(page: &Rc<PageDom>, host: NodeId, kind: Kind, value: &str) -> bool {
    let Some(fmt) = Format::parse(kind, value) else { return false };
    let Some((start, end)) = editing::selection_range(page) else { return false };
    let mut nodes = formattable_nodes(page, &page.dom.borrow(), host, start, end);
    if nodes.is_empty() {
        let mut slot = page.typing_values.borrow_mut();
        let pending = pending_at(&mut slot, (start, end));
        pending.values.retain(|v| v.kind != kind);
        pending.values.push(fmt);
        return true;
    }
    let reversed = editing::selection(page).is_some_and(|(a, _)| a == end);
    let needs: Vec<_> = {
        let dom = page.dom.borrow();
        nodes.iter().map(|&n| !equivalent(&fmt, &effective(page, &dom, kind, n))).collect()
    };
    if !needs.contains(&true) {
        return true;
    }
    super::record_host_edit(page, host, kind.input_type());
    let mut dom = page.dom.borrow_mut();
    if let Some(&last) = nodes.last() {
        if last == end.0 && editing::is_text(&dom, last) && end.1 < editing::len(&dom, last) {
            let _ = dom.split_text(last, end.1);
        }
    }
    if let Some(first) = nodes.first_mut() {
        if *first == start.0 && editing::is_text(&dom, *first) && start.1 > 0 {
            if let Ok(right) = dom.split_text(*first, start.1) {
                *first = right;
            }
        }
    }
    let mut changed = false;
    for (&n, needs_format) in nodes.iter().zip(needs) {
        if needs_format {
            if kind == Kind::Background && pyro_dom::color_well::is_fully_transparent(&fmt.css) {
                changed |= clear_highlight(page, &mut dom, n);
            } else {
                changed |= format_selected(page, &mut dom, host, n, (start, end), &fmt).is_some();
            }
        }
    }
    // Preserve element-offset endpoints when their container survives.
    // Moving a selected child into a wrapper must not anchor the selection
    // in a text node which a later innerHTML assignment will remove.
    let point = |original: Point, node: NodeId, after: bool| -> Option<Point> {
        if !editing::is_text(&dom, original.0) && dom.contains(original.0, node).ok()? {
            let mut child = node;
            while let Some(p) = dom.parent(child).ok()? {
                if p == original.0 {
                    return Some((p, dom.index_in_parent(child).ok()? + u32::from(after)));
                }
                child = p;
            }
        }
        if editing::is_text(&dom, node) {
            Some((node, if after { editing::len(&dom, node) } else { 0 }))
        } else {
            Some((dom.parent(node).ok()??, dom.index_in_parent(node).ok()? + u32::from(after)))
        }
    };
    let selection =
        nodes.first().zip(nodes.last()).and_then(|(&f, &l)| Some((point(start, f, false)?, point(end, l, true)?)));
    drop(dom);
    if let Some((a, b)) = selection {
        if reversed {
            editing::set_selection(page, b, a);
        } else {
            editing::set_selection(page, a, b);
        }
    }
    super::note_host_edited(page, host);
    if changed {
        let data = if kind == Kind::Family { value } else { &fmt.css };
        editing::input_event(page, host, "input", kind.input_type(), Some(data), false);
    }
    true
}

/// Locate the element providing the caret's superscript or subscript.
fn script_carrier(page: &PageDom, dom: &Dom, node: NodeId, host: NodeId) -> Option<(NodeId, &'static str)> {
    let layout = page.layout();
    let mut cur = dom.parent_element(node).ok().flatten();
    while let Some(el) = cur {
        let value = layout.computed_style_value(dom, el, "vertical-align");
        match value.as_str() {
            "sub" => return Some((el, "sub")),
            "super" => return Some((el, "sup")),
            _ => {}
        }
        if el == host { break; }
        cur = dom.parent_element(el).ok().flatten();
    }
    None
}

/// A typing override affects the insertion style on the immediate text
/// container. A script higher in the inline tree remains observable there
/// until typing actually splits the carrier.
pub(super) fn caret_script_state(page: &PageDom, point: Point, tag: &str) -> Option<bool> {
    let dom = page.dom.borrow();
    if !editing::is_text(&dom, point.0) { return None; }
    let host = editing::editing_host(&dom, point.0)?;
    let container = dom.parent_element(point.0).ok()??;
    let carrier = script_carrier(page, &dom, point.0, host);
    let own = carrier.is_some_and(|(_, script)| script == tag);
    Some(match toggle_state(page, (point, point), tag) {
        Some(enabled) => enabled || carrier.is_some_and(|(node, script)| node != container && script == tag),
        None => own,
    })
}

/// Split just the script-bearing inline at the inserted text. Preserve
/// unrelated CSS and attributes on that portion when removing the script.
fn clear_script(page: &PageDom, dom: &mut Dom, typed: NodeId, host: NodeId) -> Option<bool> {
    let Some((carrier, _)) = script_carrier(page, dom, typed, host) else { return Some(true) };
    let tag = editing::local(dom, carrier);
    let authored = DeclarationBlock::parse(dom.get_attribute(carrier, "style").ok()?.unwrap_or(""));
    // Typing can remove a semantic tag or an inline declaration. A class
    // rule continues to style the inserted text (including the other side
    // of a split), unless another script is explicitly applied inside it.
    if !matches!(tag.as_str(), "sub" | "sup") && authored.value("vertical-align").is_empty() {
        return Some(true);
    }
    if carrier == host || !matches!(tag.as_str(), "sub" | "sup" | "span") {
        return Some(false);
    }
    let parent = dom.parent_element(carrier).ok()??;
    let mut edges = [(typed, 0), (typed, editing::len(dom, typed))];
    let middle = super::alignment::split_edge(dom, parent, typed, false, &mut edges)?;
    super::alignment::split_edge(dom, parent, typed, true, &mut edges)?;
    let mut declarations = DeclarationBlock::parse(dom.get_attribute(middle, "style").ok()?.unwrap_or(""));
    if declarations.remove("vertical-align") {
        if declarations.is_empty() {
            dom.remove_attribute(middle, "style").ok()?;
        } else {
            dom.set_attribute(middle, "style", &declarations.css_text()).ok()?;
        }
    }
    if dom.attributes(middle).ok()?.is_empty() {
        super::indentation::unwrap(dom, middle, &mut edges)?;
    } else {
        if editing::local(dom, middle) != "span" {
            super::indentation::rename(dom, middle, "span", &mut edges)?;
        }
    }
    Some(true)
}

/// Isolate only the newly inserted UTF-16 text and carry the pending values
/// to it. Later typing inherits its inline style from that wrapper.
pub(crate) fn format_typed(
    page: &PageDom,
    dom: &mut Dom,
    range: (Point, Point),
    end: Point,
    text: &str,
) -> Option<Point> {
    let mut slot = page.typing_values.borrow_mut();
    let pending = slot.as_mut().filter(|v| v.range == range)?;
    pending.values.retain(|fmt| !equivalent(fmt, &effective(page, dom, fmt.kind, end.0)));
    let host = editing::editing_host(dom, end.0)?;
    let parent = dom.parent_element(end.0).ok()??;
    let layout = page.layout();
    pending.toggles.retain(|&(tag, enabled)| {
        let active = if matches!(tag, "sub" | "sup") {
            script_carrier(page, dom, end.0, host).is_some_and(|(_, active)| active == tag)
        } else if tag == "b" {
            super::numeric_bold_weight(&layout.computed_style_value(dom, parent, "font-weight"))
        } else {
            let style = layout.computed_style_value(dom, parent, "font-style");
            style.starts_with("italic") || style.starts_with("oblique")
        };
        enabled != active
    });
    let decoration = pending.decoration.filter(|&lines| lines != super::decoration::Lines::at(page, dom, end.0));
    if pending.values.is_empty() && pending.toggles.is_empty() && decoration.is_none() {
        *slot = None;
        return Some(end);
    }
    let units = pyro_dom::dom::utf16_len(text);
    if units == 0 || units > end.1 || !editing::is_text(dom, end.0) {
        return None;
    }
    if end.1 < editing::len(dom, end.0) {
        dom.split_text(end.0, end.1).ok()?;
    }
    let typed = if end.1 > units { dom.split_text(end.0, end.1 - units).ok()? } else { end.0 };
    let mut value_wrapper = None;
    for fmt in &pending.values {
        if fmt.kind == Kind::Background && pyro_dom::color_well::is_fully_transparent(&fmt.css) {
            clear_highlight(page, dom, typed);
        } else {
            value_wrapper = Some(wrap(dom, typed, fmt)?);
        }
    }
    if let Some(lines) = decoration { super::decoration::apply(page, dom, host, typed, lines)?; }
    for &(tag, enabled) in &pending.toggles {
        let (property, value) = match tag {
            "sub" => ("vertical-align", "sub"),
            "sup" => ("vertical-align", "super"),
            _ => super::inline_css(tag)?,
        };
        let carriers: &[&str] = if tag == "b" { &["b", "strong"] } else { &["i", "em"] };
        if !enabled && matches!(tag, "sub" | "sup") {
            if clear_script(page, dom, typed, host)? { continue; }
        } else if !enabled {
            while let Some(ancestor) = super::ancestor_format(dom, typed, host, carriers, true) {
                super::hoist(dom, typed, ancestor);
                if dom.contains(ancestor, typed).unwrap_or(false) {
                    break;
                }
            }
            let font = if tag == "b" { super::FontToggle::Bold } else { super::FontToggle::Italic };
            super::clear_remaining_font(page, dom, font, host, typed, (typed, 0), (typed, units))?;
            continue;
        }
        let doc = dom.node_document(typed).ok()?;
        let css = !enabled || dom.document_data(doc).ok()?.style_with_css;
        if css && enabled {
            if let Some(span) = value_wrapper.filter(|&el| editing::local(dom, el) == "span"
                && dom.parent_element(typed).ok().flatten() == Some(el)) {
                let mut style = DeclarationBlock::parse(dom.get_attribute(span, "style").ok()?.unwrap_or(""));
                style.set(property, value, false)?;
                dom.set_attribute(span, "style", &style.css_text()).ok()?;
                continue;
            }
        }
        let wrapper = dom.create_element(doc, if css { "span" } else { tag }).ok()?;
        if css {
            let value = if enabled { value } else if property == "vertical-align" { "baseline" } else { "normal" };
            dom.set_attribute(wrapper, "style", &format!("{property}: {value};")).ok()?;
        }
        let parent = dom.parent_element(typed).ok()??;
        dom.insert_before(parent, wrapper, Some(typed)).ok()?;
        dom.append_child(wrapper, typed).ok()?;
    }
    *slot = None;
    Some((typed, units))
}
