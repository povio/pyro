//! pyro: the page's selection, a DOM range the embedder keeps (pyro-webapi's document
//! selection), painted as highlighted text. The embedder sets it before a paint
//! ([`BaseDocument::set_dom_selection`]); blitz-paint reads it as the inline roots' selected byte
//! ranges ([`BaseDocument::get_text_selection_ranges`]), as it reads blitz's own selection.

use blitz_traits::node_id::NodeId;
use std::collections::HashMap;

use crate::{BaseDocument, NodeData};

thread_local! {
    /// pyro: while set, `:link` does not match a visited link (its highlight styles are resolved
    /// as Chromium's visited style; `BaseDocument::selection_pseudo_style`).
    pub(crate) static LINKS_AS_VISITED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A selection's ends: text nodes, each with an offset in UTF-16 code units of its data, the
/// start first in tree order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomSelection {
    pub start: (NodeId, usize),
    pub end: (NodeId, usize),
}

/// pyro: a highlight painted under the selection: a custom highlight (`::highlight(<name>)`) or
/// the document's target text (`::target-text`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightName {
    Custom(String),
    TargetText,
}

impl HighlightName {
    /// The pseudo-element its rules are filed under, and the one a rule must name to match.
    fn pseudo(&self) -> (style::selector_parser::PseudoElement, style::selector_parser::PseudoElement) {
        use style::selector_parser::PseudoElement;
        match self {
            HighlightName::Custom(name) => (
                PseudoElement::Highlight(style::selector_parser::ANY_HIGHLIGHT),
                PseudoElement::Highlight(style::selector_parser::highlight_name_id(&style::Atom::from(name.as_str()))),
            ),
            HighlightName::TargetText => (PseudoElement::TargetText, PseudoElement::TargetText),
        }
    }
}

/// pyro: where the caret is in the layout ([`BaseDocument::caret_position`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretPosition {
    /// In the text of an inline root: its node and a byte offset into its text
    /// (`TextLayout::text`).
    Text { root: NodeId, byte: usize },
    /// In an element with no text (an empty editing host): the element.
    Empty(NodeId),
}

impl BaseDocument {
    /// pyro: the caret painted from now on (`None`: none): a text node and an offset in UTF-16
    /// code units of its data, or an element with no text in it (offset 0).
    pub fn set_dom_caret(&mut self, caret: Option<(NodeId, usize)>) {
        self.dom_caret = caret;
    }

    /// pyro: the width of `0` in the first available font of `element` (CSS px), the width of a
    /// block caret with no character after it.
    pub fn zero_advance(&self, element: NodeId) -> f32 {
        let Some(style) = self.nodes.get(element).and_then(|node| node.primary_styles()) else { return 0.0 };
        let font = style.get_font();
        let size = font.font_size.computed_size();
        let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: self.font_ctx.clone() };
        provider.metrics_for(font, size, ' ').zero_advance_measure.map_or(size.px() * 0.5, |m| m.px())
    }

    /// pyro: where the caret ([`BaseDocument::set_dom_caret`]) is in the layout, and the element
    /// whose style it takes (`caret-color`, `caret-shape`).
    pub fn caret_position(&self) -> Option<(CaretPosition, NodeId)> {
        let (node_id, offset) = self.dom_caret?;
        let node = self.nodes.get(node_id)?;
        if !node.flags.is_in_document() {
            return None;
        }
        let NodeData::Text(data) = &node.data else {
            return Some((CaretPosition::Empty(node_id), node_id));
        };
        let element = node.parent?;
        for (root_id, root) in self.nodes.iter() {
            let Some(layout) = root.element_data().and_then(|element| element.inline_layout_data.as_deref()) else {
                continue;
            };
            if !root.flags.is_in_document() {
                continue;
            }
            for (text_id, bytes) in &layout.text_nodes {
                if *text_id != node_id {
                    continue;
                }
                let Some(produced) = layout.text.get(bytes.clone()) else { continue };
                let byte = bytes.start + produced_offset(&data.content, produced, offset);
                return Some((CaretPosition::Text { root: root_id, byte }, element));
            }
        }
        None
    }

    /// pyro: the highlights painted from now on, bottom first (custom highlights by priority,
    /// then the target text), each with its ranges.
    pub fn set_dom_highlights(&mut self, highlights: Vec<(HighlightName, Vec<DomSelection>)>) {
        self.dom_highlights = highlights;
    }

    /// pyro: the highlights (`set_dom_highlights`), bottom first, each with the inline roots
    /// holding its text and the bytes of their text it covers.
    pub fn highlight_layer_ranges(&self) -> Vec<(HighlightName, Vec<(NodeId, usize, usize)>)> {
        self.dom_highlights
            .iter()
            .map(|(name, ranges)| {
                (name.clone(), ranges.iter().flat_map(|range| self.dom_selection_ranges(*range)).collect())
            })
            .collect()
    }

    /// pyro: the style of a highlight pseudo-element of the element `node_id` when a rule gives
    /// it one (`None`: no rule does).
    pub fn highlight_pseudo_style(
        &self,
        node_id: NodeId,
        name: &HighlightName,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        use style::shared_lock::StylesheetGuards;
        use style::stylist::RuleInclusion;
        let node = self.nodes.get(node_id)?;
        let primary = node.primary_styles()?;
        let (key, wanted) = name.pseudo();
        let matches = move |pe: &style::selector_parser::PseudoElement| *pe == wanted;
        let read_guard = self.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        self.stylist.lazily_compute_pseudo_element_style::<&crate::Node>(
            &guards,
            node,
            &key,
            RuleInclusion::All,
            &primary,
            true,
            Some(&matches),
        )
    }

    /// pyro: the `::selection` style of `node_id` computed now (its eager one resolved container
    /// units before layout sized the containers).
    pub fn selection_pseudo_style(
        &self,
        node_id: NodeId,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        use style::shared_lock::StylesheetGuards;
        use style::stylist::RuleInclusion;
        let node = self.nodes.get(node_id)?;
        let primary = node.primary_styles()?;
        let read_guard = self.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        // pyro: a visited link's style matches `:link` as not (`LINKS_AS_VISITED`).
        let visited = self.is_visited_link(node_id);
        let outer = LINKS_AS_VISITED.replace(visited);
        let style = self.stylist.lazily_compute_pseudo_element_style::<&crate::Node>(
            &guards,
            node,
            &style::selector_parser::PseudoElement::Selection,
            RuleInclusion::All,
            &primary,
            true,
            None,
        );
        LINKS_AS_VISITED.set(outer);
        style
    }

    /// pyro: whether `node_id` is a visited link (`ElementData::flush_link_visited`).
    pub fn is_visited_link(&self, node_id: NodeId) -> bool {
        self.nodes
            .get(node_id)
            .is_some_and(|node| node.element_state().contains(style_dom::ElementState::VISITED))
    }

    /// pyro: whether the author declaration of `longhand` that `style` uses is `currentcolor`
    /// (`color: currentcolor` computes to the inherited colour, which a highlight replaces with
    /// the colour of the highlight below).
    pub fn style_declares_current_color(
        &self,
        style: &style::properties::ComputedValues,
        longhand: style::properties::LonghandId,
    ) -> bool {
        use style::properties::{CSSWideKeyword, PropertyDeclaration, PropertyDeclarationId};
        use style::stylesheets::Origin;
        use style::values::specified::Color;
        let guard = self.guard.read();
        for node in style.rules().self_and_ancestors() {
            if node.cascade_level().origin().origin() == Origin::UserAgent {
                continue;
            }
            let Some(source) = node.style_source() else { continue };
            let Some((declaration, _)) = source.read(&guard).get(PropertyDeclarationId::Longhand(longhand)) else {
                continue;
            };
            return match declaration {
                PropertyDeclaration::Color(value) => matches!(value.0, Color::CurrentColor),
                PropertyDeclaration::BackgroundColor(value) => matches!(value, Color::CurrentColor),
                PropertyDeclaration::CSSWideKeyword(wide) => match wide.keyword {
                    CSSWideKeyword::RevertLayer | CSSWideKeyword::RevertRule => continue,
                    _ => false,
                },
                _ => false,
            };
        }
        false
    }

    /// pyro: the CSS text of the author or user declaration of `longhand` that `style`'s rules
    /// give (the highest priority one), `None` without one or for a CSS-wide keyword.
    pub(crate) fn declared_css(
        &self,
        style: &style::properties::ComputedValues,
        longhand: style::properties::LonghandId,
    ) -> Option<String> {
        use style::properties::{PropertyDeclaration, PropertyDeclarationId};
        use style::stylesheets::Origin;
        let guard = self.guard.read();
        for node in style.rules().self_and_ancestors() {
            if node.cascade_level().origin().origin() == Origin::UserAgent {
                continue;
            }
            let Some(source) = node.style_source() else { continue };
            let Some((declaration, _)) = source.read(&guard).get(PropertyDeclarationId::Longhand(longhand)) else {
                continue;
            };
            if matches!(declaration, PropertyDeclaration::CSSWideKeyword(_)) {
                return None;
            }
            let mut css = String::new();
            declaration.to_css(&mut css).ok()?;
            return Some(css);
        }
        None
    }

    /// pyro: whether author or user rules (style sheets, style attributes, animations) give
    /// `style` a declaration of some of `longhands`. A `revert` at the highest priority declares
    /// nothing (it rolls back to the user agent's, which declares none of these for a highlight
    /// pseudo-element); `revert-layer` defers to the declarations below it. Highlight painting asks
    /// it of a `::selection` style: one without its own `color` or `background-color` is drawn in
    /// the system's highlight colours (CSS Pseudo-Elements 4 § 3.4, as Chromium).
    pub fn style_declares(
        &self,
        style: &style::properties::ComputedValues,
        longhands: &[style::properties::LonghandId],
    ) -> bool {
        use style::computed_value_flags::ComputedValueFlags;
        use style::properties::LonghandId as L;
        let guard = self.guard.read();
        // pyro: a visited link's visited-dependent colours come from its visited style, whose
        // rules (`:visited` ones) declare them too.
        let visited = style
            .visited_style()
            .filter(|_| style.flags.contains(ComputedValueFlags::IS_RELEVANT_LINK_VISITED));
        longhands.iter().any(|&longhand| {
            let dependent = matches!(
                longhand,
                L::Color | L::BackgroundColor | L::TextDecorationColor | L::TextEmphasisColor | L::OutlineColor
                    | L::CaretColor | L::Fill | L::Stroke
            );
            let rule_sets = std::iter::once(style).chain(visited.filter(|_| dependent));
            rule_sets.into_iter().any(|style| self.rules_declare(&guard, style, longhand))
        })
    }

    fn rules_declare(
        &self,
        guard: &style::shared_lock::SharedRwLockReadGuard,
        style: &style::properties::ComputedValues,
        longhand: style::properties::LonghandId,
    ) -> bool {
        use style::properties::{CSSWideKeyword, PropertyDeclaration, PropertyDeclarationId};
        use style::stylesheets::Origin;
        {
            for node in style.rules().self_and_ancestors() {
                if node.cascade_level().origin().origin() == Origin::UserAgent {
                    continue;
                }
                let Some(source) = node.style_source() else { continue };
                let Some((declaration, _)) = source.read(guard).get(PropertyDeclarationId::Longhand(longhand)) else {
                    continue;
                };
                return match declaration {
                    PropertyDeclaration::CSSWideKeyword(wide) => match wide.keyword {
                        CSSWideKeyword::Revert => false,
                        CSSWideKeyword::RevertLayer | CSSWideKeyword::RevertRule => continue,
                        _ => true,
                    },
                    _ => true,
                };
            }
            false
        }
    }

    /// pyro: the replaced elements the selection holds, painted with its tint from now on.
    pub fn set_selected_replaced(&mut self, nodes: Vec<NodeId>) {
        self.selected_replaced = nodes;
    }

    /// pyro: whether the selection holds the replaced element `node_id`.
    pub fn is_selected_replaced(&self, node_id: NodeId) -> bool {
        self.selected_replaced.contains(&node_id)
    }

    /// pyro: the selection painted from now on (`None`: none), in place of blitz's own.
    pub fn set_dom_selection(&mut self, selection: Option<DomSelection>) {
        self.dom_selection = selection;
    }

    /// pyro: the inline roots holding text of `selection`, each with the bytes of its text
    /// (`TextLayout::text`) the selection covers.
    pub(crate) fn dom_selection_ranges(&self, selection: DomSelection) -> Vec<(NodeId, usize, usize)> {
        let selected = self.selected_text_nodes(selection);
        if selected.is_empty() {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for (root_id, node) in self.nodes.iter() {
            let Some(layout) = node.element_data().and_then(|element| element.inline_layout_data.as_deref()) else {
                continue;
            };
            if !node.flags.is_in_document() {
                continue;
            }
            let (mut start, mut end) = (usize::MAX, 0);
            for (text_id, bytes) in &layout.text_nodes {
                let Some(&(from, to)) = selected.get(text_id) else { continue };
                let Some(NodeData::Text(data)) = self.nodes.get(*text_id).map(|node| &node.data) else { continue };
                let Some(produced) = layout.text.get(bytes.clone()) else { continue };
                let from = bytes.start + produced_offset(&data.content, produced, from);
                let to = bytes.start + to.map_or(produced.len(), |to| produced_offset(&data.content, produced, to));
                if from < to {
                    start = start.min(from);
                    end = end.max(to);
                }
            }
            if start < end {
                ranges.push((root_id, start, end));
            }
        }
        ranges
    }

    /// pyro: the text nodes from the start of `selection` to its end, in tree order, each with the
    /// UTF-16 range of its data selected (`None`: to its end).
    pub(crate) fn selected_text_nodes(&self, selection: DomSelection) -> HashMap<NodeId, (usize, Option<usize>)> {
        let mut selected: HashMap<NodeId, (usize, Option<usize>)> = HashMap::new();
        let mut stack = vec![self.root_node().id];
        let mut inside = false;
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get(id) else { continue };
            if let NodeData::Text(_) = node.data {
                let start = if id == selection.start.0 {
                    inside = true;
                    selection.start.1
                } else {
                    0
                };
                if !inside {
                    continue;
                }
                if id == selection.end.0 {
                    selected.insert(id, (start, Some(selection.end.1)));
                    break;
                }
                selected.insert(id, (start, None));
                continue;
            }
            stack.extend(node.children.iter().rev().copied());
        }
        selected
    }

    /// pyro: an inline `<svg>` serialized again with its selected text in its `::selection` fill and
    /// stroke (CSS Pseudo 4 § 3.2: `fill`, `stroke-color`, `stroke-width` apply to selected SVG
    /// text) and casting its `text-shadow`, for a content box of `width` x `height` CSS px; `None` with no selected text that
    /// such a style restyles. Not kept: made for each paint with a selection.
    #[cfg(feature = "svg")]
    pub fn inline_svg_with_selection(&self, node_id: NodeId, width: f32, height: f32) -> Option<std::sync::Arc<crate::node::SvgImageData>> {
        use style::properties::LonghandId;
        let selection = self.dom_selection?;
        let selected = self.selected_text_nodes(selection);
        let mut overrides: HashMap<NodeId, crate::node::svg_css::SelectedSvgText> = HashMap::new();
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get(id) else { continue };
            if let (NodeData::Text(_), Some(&(from, to))) = (&node.data, selected.get(&id)) {
                let Some(parent) = node.parent else { continue };
                let Some(style) = self.selection_pseudo_style(parent) else { continue };
                // (Stylo's cascade keeps only the colours of a highlight pseudo-element: the
                // declared values are read from its rules.)
                let mut attrs = String::new();
                for (longhand, name) in [
                    (LonghandId::Fill, "fill"),
                    (LonghandId::Stroke, "stroke"),
                    (LonghandId::StrokeWidth, "stroke-width"),
                ] {
                    if let Some(value) = self.declared_css(&style, longhand) {
                        let mut escaped = String::new();
                        let _ = html_escape::encode_double_quoted_attribute_to_string(&value, &mut escaped);
                        attrs.push_str(&format!(" {name}=\"{escaped}\""));
                    }
                }
                // pyro: and its `text-shadow`, cast by the selected text under the text's own.
                let shadows = if self.declared_css(&style, LonghandId::TextShadow).is_some() {
                    crate::node::svg_css::text_shadows(&style, id)
                } else {
                    Vec::new()
                };
                if !attrs.is_empty() || !shadows.is_empty() {
                    overrides.insert(id, crate::node::svg_css::SelectedSvgText { from, to, attrs, shadows });
                }
                continue;
            }
            stack.extend(node.children.iter().copied());
        }
        if overrides.is_empty() {
            return None;
        }
        let node = self.get_node(node_id)?;
        crate::node::svg_css::SELECTED_SVG_TEXT.with(|cell| *cell.borrow_mut() = overrides);
        let (source, _) = node.svg_source_for_viewport(Some((width as f64, height as f64)));
        crate::node::svg_css::SELECTED_SVG_TEXT.with(|cell| cell.borrow_mut().clear());
        let source = self.with_cross_svg_references(self.with_svg_sprites(source));
        crate::util::parse_svg_resources(source.as_bytes(), Some(&self.svg_images), &self.raster_budget).ok().map(|mut svg| {
            if svg.intrinsic_dimensions.view_box.is_none() {
                svg.inline_source = crate::util::SvgSource::copy(&source, &self.raster_budget);
            }
            std::sync::Arc::new(svg)
        })
    }
}

/// Where `utf16` (an offset in UTF-16 code units of a text node's data, `content`) is in the
/// text the node produced in its inline layout (`produced`, its white space collapsed): in bytes
/// of `produced`. Collapsed white space maps to where it collapsed.
fn produced_offset(content: &str, produced: &str, utf16: usize) -> usize {
    if content == produced {
        let mut units = 0;
        for (index, c) in content.char_indices() {
            if units >= utf16 {
                return index;
            }
            units += c.len_utf16();
        }
        return content.len();
    }
    let mut out = produced.char_indices().peekable();
    // White space that the node before left to this node's text (its trailing collapsible white
    // space belongs to the next node, `TextLayout::text_nodes`) is not in this content.
    if content.chars().next().is_some_and(|c| !c.is_whitespace()) {
        while out.next_if(|&(_, p)| p.is_whitespace()).is_some() {}
    }
    let mut units = 0;
    for c in content.chars() {
        if units >= utf16 {
            break;
        }
        units += c.len_utf16();
        match out.peek() {
            Some(&(_, p)) if p == c || (p.is_whitespace() && c.is_whitespace()) => {
                out.next();
            }
            // Collapsed white space, or text the layout changed (`text-transform`).
            _ if c.is_whitespace() => {}
            // pyro: a combining mark `text-transform` removed (Greek uppercase drops the acute
            // accent and the tonos; WPT css-text `text-transform-upperlower-106`).
            Some(&(_, p)) if is_combining_mark(c) && !is_combining_mark(p) => {}
            Some(&(index, _)) => {
                // pyro: a character that `text-transform` mapped to several (`ß` to `SS`) produced
                // them all (WPT css-text `text-transform-upperlower-105`).
                let rest = &produced[index..];
                let expanded = [c.to_uppercase().collect::<String>(), c.to_lowercase().collect::<String>()]
                    .into_iter()
                    .find(|mapped| mapped.chars().count() > 1 && rest.starts_with(mapped.as_str()));
                for _ in 0..expanded.map_or(1, |mapped| mapped.chars().count()) {
                    out.next();
                }
            }
            None => break,
        }
    }
    out.peek().map_or(produced.len(), |&(index, _)| index)
}

/// pyro: whether `c` is in a block of combining diacritical marks.
fn is_combining_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

#[cfg(test)]
mod tests {
    use super::produced_offset;

    #[test]
    fn offsets_follow_case_mappings_that_change_the_length() {
        // `ß` uppercased to "SS": one source character covers both.
        assert_eq!(produced_offset("\u{DF}", "SS", 1), 2);
        assert_eq!(produced_offset("a\u{DF}b", "ASSB", 2), 3);
        // Greek uppercase drops the acute: "Ϊ́Ρ" to "Ϊ̈Ρ"; the first three source characters are
        // the first letter.
        assert_eq!(produced_offset("\u{399}\u{308}\u{301}\u{3A1}", "\u{399}\u{308}\u{3A1}", 3), 4);
    }

    #[test]
    fn offsets_follow_collapsed_white_space() {
        assert_eq!(produced_offset("abc", "abc", 2), 2);
        // UTF-16: 'é' is one unit, two bytes; '𝄞' two units, four bytes.
        assert_eq!(produced_offset("é𝄞x", "é𝄞x", 3), 6);
        // "\n  Lorem  ipsum" collapsed to "Lorem ipsum".
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 3), 0);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 8), 5);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 10), 6);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 15), 11);
    }

    #[test]
    fn offsets_skip_white_space_of_the_node_before() {
        // "Text <span>sample</span>": the span's text produced " sample".
        assert_eq!(produced_offset("sample", " sample", 0), 1);
        assert_eq!(produced_offset("sample", " sample", 5), 6);
        assert_eq!(produced_offset("sample", " sample", 6), 7);
    }
}
