use blitz_traits::node_id::NodeId;
use markup5ever::local_name;
use parley::FontFamily;
use style::computed_values::list_style_type::T as ListStyleType;
use style::{
    computed_values::list_style_position::T as ListStylePosition, counter_style::CounterStyle,
};
use stylo_taffy::CopyStyleGetters;

use crate::{
    BaseDocument,
    node::{ListItemLayout, ListItemLayoutPosition, Marker},
    stylo_to_parley,
};

pub(super) const BULLET_FONT_FAMILY: &str = "Bullet, monospace, sans-serif";

pub(super) fn collect_list_item_children(
    doc: &mut BaseDocument,
    index: &mut usize,
    reversed: bool,
    node_id: NodeId,
) {
    let mut children = doc.nodes[node_id].children.clone();
    // pyro: a `::before` or `::after` box is a list item too (`display: list-item`).
    if let Some(before) = doc.nodes[node_id].before() {
        children.insert(0, before);
    }
    if let Some(after) = doc.nodes[node_id].after() {
        children.push(after);
    }
    if reversed {
        children.reverse();
    }
    for child in children.into_iter() {
        // pyro: the item's number is its `list-item` counter value (`layout::counters`: `value`,
        // `start`, `reversed` and the counter properties); its position in its list without one.
        let number = {
            let node = &doc.nodes[node_id];
            let numbers = &doc.list_item_numbers;
            let pseudo = if node.before() == Some(child) {
                numbers.1.get(&(node_id, true))
            } else if node.after() == Some(child) {
                numbers.1.get(&(node_id, false))
            } else {
                numbers.0.get(&child)
            };
            pseudo.copied().unwrap_or(*index as i32 + 1)
        };
        if let Some(layout) = node_list_item_child(doc, child, number) {
            let node = &mut doc.nodes[child];
            let element_data = node.element_data_mut().unwrap();
            let inside = matches!(layout.position, ListItemLayoutPosition::Inside);
            // (A marker styled by `::marker` is built again: its style may have changed.)
            let same = layout.style_node.is_none()
                && element_data.list_item_data.as_ref().is_some_and(|old| {
                    old.style_node.is_none()
                        && old.marker == layout.marker
                        && matches!(old.position, ListItemLayoutPosition::Inside) == inside
                });
            element_data.list_item_data = Some(Box::new(layout));
            if !same {
                marker_changed(node);
            }
            *index += 1;
            crate::util::ensure_stack(|| collect_list_item_children(doc, index, reversed, child));
        } else {
            // Unset marker in case it was previously set
            let node = &mut doc.nodes[child];
            if let Some(element_data) = node.element_data_mut() {
                if element_data.list_item_data.take().is_some() {
                    marker_changed(node);
                }
            }
            // pyro: the list items of a `display: contents` child (a slot's assigned items) are
            // this list's, numbered on (WPT css-lists `counter-list-item-slot-order`); a list
            // element numbers its own (the numbers come from `layout::counters` anyway).
            // pyro: so are those of an inline box (an `ol` with `display: inline`), which its
            // inline formatting context lays out and no construction of its own numbers (WPT
            // css-lists `inline-block-list`).
            if doc.nodes[child].primary_styles().is_some_and(|style| {
                let display = style.get_box().display;
                display.is_contents()
                    || (display.outside() == style::values::specified::box_::DisplayOutside::Inline
                        && display.inside() == style::values::specified::box_::DisplayInside::Flow)
            }) {
                match list_numbering(doc, child) {
                    Some((mut own, own_reversed)) => {
                        crate::util::ensure_stack(|| collect_list_item_children(doc, &mut own, own_reversed, child))
                    }
                    None => crate::util::ensure_stack(|| collect_list_item_children(doc, index, reversed, child)),
                }
            }
        }
    }
}

/// pyro: the first index and direction of the numbering a list element (`ol`, `ul`, `menu`,
/// `dir`) starts for its items; `None` for other elements, whose items continue their parent's.
pub(super) fn list_numbering(doc: &BaseDocument, node_id: NodeId) -> Option<(usize, bool)> {
    let el = doc.nodes[node_id].element_data()?;
    match el.name.local {
        local_name!("ol") => Some((
            el.attr_parsed(local_name!("start")).map(|start: usize| start - 1).unwrap_or(0),
            el.attr_parsed(local_name!("reversed")).unwrap_or(false),
        )),
        local_name!("ul") | local_name!("menu") | local_name!("dir") => Some((0, false)),
        _ => None,
    }
}

/// pyro: a list item whose marker changed (its number, after an item was added, moved or
/// removed before it) is built and laid out again: an `inside` marker is text of its inline layout,
/// and an `outside` one sizes its line.
fn marker_changed(node: &mut crate::Node) {
    node.insert_damage(super::damage::ALL_DAMAGE);
    node.clear_layout_cache();
}

// Return a child node which is of display: list-item
fn node_list_item_child(
    doc: &mut BaseDocument,
    child_id: NodeId,
    number: i32,
) -> Option<ListItemLayout> {
    let node = &doc.nodes[child_id];

    // We only care about elements with display: list-item (li's have this automatically)
    if !node
        .primary_styles()
        .is_some_and(|style| style.get_box().display.is_list_item())
    {
        return None;
    }

    //Break on container elements when already in a list
    if node
        .element_data()
        .map(|element_data| {
            matches!(
                element_data.name.local,
                local_name!("ol") | local_name!("ul"),
            )
        })
        .unwrap_or(false)
    {
        return None;
    };

    let styles = style::servo_arc::Arc::clone(&*node.primary_styles().unwrap());
    let list_style_type = styles.get_list_style_type();
    let list_style_position = styles.clone_list_style_position();
    // pyro: an outside item's loaded `list-style-image` is its marker; while it loads, when it
    // fails and inside the item (not supported), the `list-style-type` marker is.
    // pyro: inside the item too (a box at the start of its first line, `inline::build`), and
    // at the resolution of an `image-set()` option.
    let resolution = crate::util::image_resolution(&styles.get_list().list_style_image);
    let image = node.element_data().and_then(|element| image_marker(element, resolution));
    // pyro: the item's `::marker` style, on a node of its own (the marker text's brush), and
    // the marker's `content`: `none` leaves no marker, strings replace it (CSS Lists 3 § 3.2).
    let (marker_style, rule_style) = item_marker_style(doc, child_id, &styles);
    // pyro: content that reads other counters is evaluated with the document's counters
    // (`layout::counters`); the first such marker turns that evaluation on.
    let content = match rule_style.as_ref() {
        Some(style) if marker_reads_counters(style) => match doc.marker_texts.get(&child_id) {
            Some(text) => Some(text.clone()),
            None => {
                if !doc.marker_counters {
                    doc.marker_counters = true;
                    doc.marker_counters_pending = true;
                    doc.generated_counters = true;
                    doc.counters_dirty = true;
                }
                None
            }
        },
        Some(style) => marker_content(style, number),
        None => None,
    };
    let from_content = content.is_some();
    let style_node = marker_style_node(doc, child_id, marker_style.clone());
    let mut marker = match (content, image) {
        (Some(text), _) => Marker::String(text),
        (None, Some(image)) => image,
        (None, None) => match custom_counter_marker(doc, child_id, list_style_type, number) {
            Some(marker) => marker,
            None => marker_for_style(list_style_type, number)?,
        },
    };
    // pyro: a `::marker` rule's `text-transform` transforms the marker's string; the item's
    // does not reach it (Blink's UA sheet has `::marker { text-transform: none }`).
    if let (Marker::String(text), Some(style)) = (&marker, rule_style.as_ref())
        && doc.style_declares(style, &[style::properties::LonghandId::TextTransform])
    {
        let transform = crate::layout::text_transform::CaseTransform::from_style(style);
        let text = crate::layout::text_transform::TextTransformer::default().transform_alone(text, &transform);
        marker = Marker::String(text);
    }
    // pyro: digits are tabular unless a `::marker` rule declares `font-variant-numeric`.
    let tabular_nums = !rule_style
        .as_ref()
        .is_some_and(|style| doc.style_declares(style, &[style::properties::LonghandId::FontVariantNumeric]));
    let isolate = !rule_style.as_ref().is_some_and(|style| {
        style.clone_unicode_bidi() == style::computed_values::unicode_bidi::T::Normal
            && doc.style_declares(style, &[style::properties::LonghandId::UnicodeBidi])
    });
    // The marker's text has the pseudo-element's style.
    let text_style = marker_style.as_ref().unwrap_or(&styles);
    let brush = style_node.unwrap_or(child_id);

    let mut annotations = Vec::new();
    let position = match list_style_position {
        ListStylePosition::Inside => {
            // pyro: a symbol marker inside the item is drawn from the bundled bullet font, unless
            // a `::marker` rule sets its `font-family`: then it is text in that font, as
            // `counter(c, square)` is (WPT CSS2 `content-counter-004`). Other `::marker` styles
            // keep the symbol (WPT css-pseudo `marker-word-spacing`, in Ahem).
            if let Marker::Char(symbol) = marker
                && rule_style
                    .as_ref()
                    .is_some_and(|style| doc.style_declares(style, &[style::properties::LonghandId::FontFamily]))
            {
                marker = Marker::String(format!("{symbol} "));
            }
            ListItemLayoutPosition::Inside
        }
        ListStylePosition::Outside => {
            let mut parley_style = stylo_to_parley::style(brush, text_style);
            // The marker's suffix space takes room (`white-space: pre` in the UA sheet), as a
            // no-break space of a `content` string does.
            parley_style.white_space_collapse = parley::WhiteSpaceCollapse::Preserve;
            if tabular_nums {
                parley_style.font_features = parley::FontFeatures::List(std::borrow::Cow::Owned(
                    stylo_to_parley::with_tabular_nums(&parley_style.font_features),
                ));
            }

            // pyro: blitz-paint paints a disc, circle or square as a shape sized from the item's
            // font (Blink's `ListMarker::RelativeSymbolMarkerRect`), so their layout is a line of
            // that font.
            let symbol = matches!(marker, Marker::Char('•' | '◦' | '▪') | Marker::Image { .. });
            // (A `content` string is text in the marker's own font.)
            if let Some(font_family) = font_for_bullet_style(list_style_type).filter(|_| !symbol && !from_content) {
                parley_style.font_family = font_family;
            }

            // Create a parley tree builder
            let mut font_ctx = doc.font_ctx.lock().unwrap();
            let mut builder = doc.layout_ctx.tree_builder(
                &mut font_ctx,
                doc.viewport.scale(),
                true,
                &parley_style,
            );
            // pyro: the marker's text is a paragraph of the item's direction (an isolate).
            builder.set_base_direction(if text_style.clone_direction() == style::computed_values::direction::T::Rtl {
                parley::BaseDirection::Rtl
            } else {
                parley::BaseDirection::Ltr
            });

            match &marker {
                Marker::Char(_) | Marker::Image { .. } if symbol => builder.push_text("x"),
                Marker::Char(char) => {
                    let mut buf = [0u8; 4];
                    builder.push_text(char.encode_utf8(&mut buf));
                }
                Marker::String(str) => builder.push_text(str),
                Marker::Image { .. } => {}
            };

            let mut layout = builder.build().0;
            let width = layout.calculate_content_widths().max;
            layout.break_all_lines(Some(width));
            // pyro: its emphasis marks (`text-emphasis`), drawn as over the item's text.
            annotations.extend(super::inline::build::marker_emphasis(
                &doc.nodes,
                &mut doc.layout_ctx,
                &mut font_ctx,
                doc.viewport.scale(),
                brush,
            ));

            ListItemLayoutPosition::Outside(Box::new(layout))
        }
    };

    Some(ListItemLayout {
        marker,
        style_node,
        position,
        tabular_nums,
        isolate,
        content_box_subpixel_offset: taffy::Point::ZERO,
        annotations,
    })
}

/// pyro: the marker an item's `list-style-image` makes once it has loaded: its natural size.
fn image_marker(element: &crate::node::ElementData, resolution: f32) -> Option<Marker> {
    use crate::node::{ImageData, Status};
    let image = element.list_style_images.first()?.as_ref()?;
    if !matches!(image.status, Status::Ok) {
        return None;
    }
    let (width, height) = match &image.image {
        ImageData::Raster(raster) => (raster.width as f32 / resolution, raster.height as f32 / resolution),
        #[cfg(feature = "svg")]
        ImageData::Svg(svg) => (svg.tree.size().width(), svg.tree.size().height()),
        _ => return None,
    };
    Some(Marker::Image { width, height })
}

/// pyro: the style of the marker of the list item `node_id` (style `item_style`): its `::marker`
/// style with the values of the animations on the marker (`Element.animate` with `pseudoElement:
/// '::marker'`, `ElementData::sampled_marker`) cascaded on it, none without either; and the
/// `::marker` style alone.
fn item_marker_style(
    doc: &BaseDocument,
    node_id: NodeId,
    item_style: &style::servo_arc::Arc<style::properties::ComputedValues>,
) -> (
    Option<style::servo_arc::Arc<style::properties::ComputedValues>>,
    Option<style::servo_arc::Arc<style::properties::ComputedValues>>,
) {
    let rule_style = marker_pseudo_style(doc, node_id, item_style);
    let style = match doc.nodes[node_id].element_data().and_then(|element| element.sampled_marker.clone()) {
        Some(block) => {
            let read_guard = doc.guard.read();
            let guards = style::shared_lock::StylesheetGuards::same(&read_guard);
            let base = rule_style.as_ref().unwrap_or(item_style);
            Some(doc.stylist.compute_for_declarations::<&crate::Node>(&guards, base, block))
        }
        None => rule_style.clone(),
    };
    (style, rule_style)
}

/// pyro: a list item with a `::marker` style node was restyled: the node takes the marker's
/// style as it is now (it inherits from the item's), so paint reads the new colours; a marker
/// that lost its style, or whose `content` changed, is built again with its list.
pub(crate) fn item_restyled(doc: &mut BaseDocument, item: NodeId) {
    let Some(node) = doc.nodes.get(item) else { return };
    let Some(style_node) = node.element_data().and_then(|element| element.marker_style) else { return };
    let Some(styles) = node.primary_styles().map(|style| style::servo_arc::Arc::clone(&*style)) else { return };
    let (style, _) = item_marker_style(doc, item, &styles);
    let content = |style: Option<&style::servo_arc::Arc<style::properties::ComputedValues>>| {
        style.map(|style| style.get_counters().content.clone())
    };
    let old = doc.nodes.get(style_node).and_then(|node| node.primary_styles().map(|style| style::servo_arc::Arc::clone(&*style)));
    let rebuild = style.is_none() || content(old.as_ref()) != content(style.as_ref());
    if style.is_some() {
        marker_style_node(doc, item, style);
    }
    if rebuild {
        let list = doc.nodes[item].parent.unwrap_or(item);
        for target in [item, list] {
            if let Some(node) = doc.nodes.get_mut(target) {
                node.insert_damage(super::damage::ALL_DAMAGE);
                node.mark_damaged();
            }
        }
    }
}

/// pyro: the `::marker` style of the list item `node_id`, when a rule matches its marker.
pub(super) fn marker_pseudo_style(
    doc: &BaseDocument,
    node_id: NodeId,
    item_style: &style::servo_arc::Arc<style::properties::ComputedValues>,
) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
    use style::selector_parser::PseudoElement;
    use style::shared_lock::StylesheetGuards;
    use style::stylist::RuleInclusion;
    // (Of an element, or of its `::before` or `::after` box: `::after::marker`.)
    let node = &doc.nodes[node_id];
    let pseudo_box = node.parent.is_some_and(|parent| {
        let parent = &doc.nodes[parent];
        parent.before() == Some(node_id) || parent.after() == Some(node_id)
    });
    if !matches!(node.data, crate::NodeData::Element(_)) && !pseudo_box {
        return None;
    }
    let read_guard = doc.guard.read();
    let guards = StylesheetGuards::same(&read_guard);
    doc.stylist.lazily_compute_pseudo_element_style::<&crate::Node>(
        &guards,
        &doc.nodes[node_id],
        &PseudoElement::Marker,
        RuleInclusion::All,
        item_style,
        true,
        None,
    )
}

/// pyro: whether a `::marker` style's `content` reads a counter other than `counter(list-item)`
/// (`marker_content` evaluates that one alone).
pub(super) fn marker_reads_counters(style: &style::servo_arc::Arc<style::properties::ComputedValues>) -> bool {
    use style::values::generics::counters::{Content, ContentItem};
    match &style.get_counters().content {
        Content::Items(items) => items.items.iter().any(|item| match item {
            ContentItem::Counter(name, _) => &*name.0 != "list-item",
            ContentItem::Counters(..) => true,
            _ => false,
        }),
        _ => false,
    }
}

/// pyro: the text a `::marker` style's `content` gives the marker of the item at `index`: none
/// for `normal` (the marker of `list-style`), empty for `none`, else its strings, the item's
/// number for `counter(list-item)` and the quotes of the style's `quotes`. Other counters and
/// images are not supported: with one, the marker stays that of `list-style`.
fn marker_content(style: &style::servo_arc::Arc<style::properties::ComputedValues>, number: i32) -> Option<String> {
    use style::values::generics::counters::{Content, ContentItem};
    match &style.get_counters().content {
        Content::Normal => None,
        Content::None => Some(String::new()),
        Content::Items(items) => {
            let mut text = String::new();
            let mut depth = 0usize;
            for item in items.items.iter() {
                match item {
                    ContentItem::String(string) => text.push_str(string),
                    ContentItem::Counter(name, counter_style) if &*name.0 == "list-item" => {
                        text.push_str(&super::counters::format(number, counter_style));
                    }
                    ContentItem::OpenQuote => {
                        if let Some((open, _)) = super::counters::quote(style, depth, None) {
                            text.push_str(&open);
                        }
                        depth += 1;
                    }
                    ContentItem::CloseQuote => {
                        depth = depth.saturating_sub(1);
                        if let Some((_, close)) = super::counters::quote(style, depth, None) {
                            text.push_str(&close);
                        }
                    }
                    ContentItem::NoOpenQuote => depth += 1,
                    ContentItem::NoCloseQuote => depth = depth.saturating_sub(1),
                    _ => return None,
                }
            }
            Some(text)
        }
    }
}

/// pyro: the node holding the list item's `::marker` style `style` (created when the item has
/// none, with the item as its parent), or none, the item's node dropped, without a style.
fn marker_style_node(
    doc: &mut BaseDocument,
    item: NodeId,
    style: Option<style::servo_arc::Arc<style::properties::ComputedValues>>,
) -> Option<NodeId> {
    use markup5ever::{QualName, ns};
    const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("span") };
    let existing = doc.nodes[item]
        .element_data()
        .and_then(|element| element.marker_style)
        .filter(|&id| doc.nodes.contains_key(id));
    let Some(style) = style else {
        if let Some(element) = doc.nodes[item].element_data_mut() {
            element.marker_style = None;
        }
        if let Some(id) = existing {
            doc.remove_node_from_tree(id);
        }
        return None;
    };
    let node_id = match existing {
        Some(node_id) => node_id,
        None => {
            let node_id = doc.create_node(crate::NodeData::AnonymousBlock(Box::new(crate::ElementData::new(NAME, Vec::new()))));
            doc.nodes[item].element_data_mut()?.marker_style = Some(node_id);
            node_id
        }
    };
    let node = &mut doc.nodes[node_id];
    node.parent = Some(item);
    let mut data = style::data::ElementData::default();
    data.styles.primary = Some(style);
    data.set_restyled();
    *node.stylo_element_data_mut().ensure_init_mut() = data;
    node.update_layout_styles();
    Some(node_id)
}

/// pyro: the marker of an item whose `list-style-type` names an `@counter-style` rule that extends
/// another style (`system: extends <name>`): the item's number in the extended style, with the
/// rule's `prefix` and `suffix` (the extended style's when it gives none). It is text in the
/// item's font, also for an extended `disc` (CSS Counter Styles 3: only the predefined bullets
/// may be drawn otherwise). Rules of the other systems format the number with their own symbols.
fn custom_counter_marker(doc: &BaseDocument, item: NodeId, list_style_type: &ListStyleType, number: i32) -> Option<Marker> {
    use style::counter_style::{Symbol, System};
    let CounterStyle::Name(name) = &list_style_type.0 else {
        return None;
    };
    // The styles a rule cannot redefine.
    if matches!(
        &*name.0,
        "decimal" | "disc" | "circle" | "square" | "disclosure-open" | "disclosure-closed" | "none"
    ) {
        return None;
    }
    // pyro: the item's shadow trees first, inside out (tree-scoped names, CSS Scoping 1 §3.5;
    // WPT css-shadow scoped-reference-animation-002), then the document's origins.
    let scoped = || {
        let mut host = doc.nodes.get(item)?.containing_shadow_host_id();
        while let Some(id) = host {
            let node = &doc.nodes[id];
            if let Some(rule) = node.shadow_cascade_data().and_then(|data| data.counter_style(&name.0)) {
                return Some(rule.clone());
            }
            host = node.containing_shadow_host_id();
        }
        None
    };
    let rule = scoped().or_else(|| {
        doc.stylist
            .iter_extra_data_origins_rev()
            .find_map(|(data, _)| data.counter_styles.get(&name.0).cloned())
    })?;
    let guard = doc.guard.read();
    let rule = rule.read_with(&guard);
    let symbol = |symbol: &Symbol| match symbol {
        Symbol::String(text) => text.to_string(),
        Symbol::Ident(ident) => ident.0.to_string(),
    };
    let descriptors = rule.descriptors();
    let System::Extends(base) = rule.resolved_system() else {
        // pyro: the other systems format with the rule's own symbols (CSS Counter Styles 3 §3.1);
        // a value out of their range takes `decimal` (the default `fallback`). Overriding a
        // predefined name (`@counter-style lower-roman { ... }`) is allowed.
        use super::counters::{additive, alphabetic, cyclic, fixed, numeric, symbolic};
        let symbols: Vec<String> = descriptors.symbols.as_ref().map_or(Vec::new(), |s| s.0.iter().map(symbol).collect());
        let symbols: Vec<&str> = symbols.iter().map(String::as_str).collect();
        let text = match rule.resolved_system() {
            System::Cyclic => cyclic(number, &symbols),
            System::Numeric => numeric(number, &symbols),
            System::Alphabetic => alphabetic(number, &symbols),
            System::Symbolic => symbolic(number, &symbols),
            System::Fixed { first_symbol_value } => {
                let first = first_symbol_value.as_ref().and_then(|v| v.resolve()).unwrap_or(1);
                fixed(number.saturating_sub(first - 1), &symbols)
            }
            System::Additive => {
                let tuples: Vec<(u32, String)> = descriptors.additive_symbols.as_ref().map_or(Vec::new(), |list| {
                    list.0
                        .iter()
                        .filter_map(|t| Some((u32::try_from(t.weight.resolve()?).ok()?, symbol(&t.symbol))))
                        .collect()
                });
                let tuples: Vec<(u32, &str)> = tuples.iter().map(|(w, s)| (*w, s.as_str())).collect();
                additive(number, &tuples)
            }
            System::Extends(_) => None,
        };
        let text = text.unwrap_or_else(|| number.to_string());
        let prefix = descriptors.prefix.as_ref().map(symbol).unwrap_or_default();
        let suffix = descriptors.suffix.as_ref().map(symbol).unwrap_or_else(|| ". ".into());
        return Some(Marker::String(format!("{prefix}{text}{suffix}")));
    };
    let bullet = matches!(&*base.0, "disc" | "circle" | "square" | "disclosure-open" | "disclosure-closed");
    let prefix = descriptors.prefix.as_ref().map(symbol).unwrap_or_default();
    let suffix = descriptors
        .suffix
        .as_ref()
        .map(symbol)
        .unwrap_or_else(|| if bullet { " ".into() } else { ". ".into() });
    let text = super::counters::format(number, &CounterStyle::Name(base.clone()));
    Some(Marker::String(format!("{prefix}{text}{suffix}")))
}

// Determine the marker to render for a given list style type
fn marker_for_style(list_style_type: &ListStyleType, number: i32) -> Option<Marker> {
    // pyro: numbers below 1 are out of the alphabetic and roman styles' ranges (their fallback,
    // `decimal`, formats them); `index` is the 0-based position of the others.
    let index = (number - 1).max(0) as usize;
    let positive = number >= 1;
    Some(match &list_style_type.0 {
        CounterStyle::Name(name)
            if !positive
                && matches!(
                    &*name.0,
                    "lower-alpha" | "upper-alpha" | "lower-latin" | "upper-latin" | "lower-roman" | "upper-roman"
                ) =>
        {
            Marker::String(format!("{number}. "))
        }
        CounterStyle::None => return None,
        CounterStyle::Name(name) => match &*name.0 {
            "lower-alpha" => {
                let mut marker = String::new();
                build_alpha_marker(index, &mut marker);
                Marker::String(format!("{marker}. "))
            }
            "upper-alpha" => {
                let mut marker = String::new();
                build_alpha_marker(index, &mut marker);
                Marker::String(format!("{}. ", marker.to_ascii_uppercase()))
            }
            "decimal" => Marker::String(format!("{number}. ")),
            "decimal-leading-zero" => Marker::String(format!(
                "{}. ",
                super::counters::format(number, &list_style_type.0)
            )),
            "lower-roman" => Marker::String(format!("{}. ", roman(index + 1).to_ascii_lowercase())),
            "upper-roman" => Marker::String(format!("{}. ", roman(index + 1))),
            "lower-latin" => {
                let mut marker = String::new();
                build_alpha_marker(index, &mut marker);
                Marker::String(format!("{marker}. "))
            }
            "upper-latin" => {
                let mut marker = String::new();
                build_alpha_marker(index, &mut marker);
                Marker::String(format!("{}. ", marker.to_ascii_uppercase()))
            }
            "disc" => Marker::Char('•'),
            "circle" => Marker::Char('◦'),
            "square" => Marker::Char('▪'),
            "disclosure-open" => Marker::Char('▾'),
            "disclosure-closed" => Marker::Char('▸'),
            // pyro: the other predefined styles (`armenian`, `georgian`, `lower-greek`, ...) as
            // `counter()` formats them, with their `. ` suffix; an unknown name is `decimal`.
            _ => Marker::String(format!(
                "{}. ",
                super::counters::format(number, &list_style_type.0)
            )),
        },
        CounterStyle::String(atom_string) => Marker::String(atom_string.as_ref().to_string()),
        CounterStyle::Symbols { symbols, .. } => {
            // Use symbols from counter-style definition
            let syms = &symbols.0;
            let len = syms.len();
            if len == 0 {
                Marker::Char('•')
            } else {
                // Cycle through symbols based on index: 1st, 2nd, 1st, 2nd, etc.
                let idx = if len > 1 { (number - 1).rem_euclid(len as i32) as usize } else { 0 };
                // Symbol is String or Ident variant, extract the string value
                let sym = &syms[idx];
                let marker_str = match sym {
                    style::counter_style::Symbol::String(s) => s.to_string(),
                    style::counter_style::Symbol::Ident(id) => id.0.to_string(),
                };
                Marker::String(marker_str)
            }
        }
    })
}

// Override the font to our specific bullet font when rendering bullets
fn font_for_bullet_style(list_style_type: &ListStyleType) -> Option<FontFamily<'static>> {
    if list_style_type.0.is_bullet() {
        return Some(BULLET_FONT_FAMILY.into());
    }

    None
}

// A roman numeral (the `upper-roman` counter style, 1 to 3999, then decimal).
fn roman(n: usize) -> String {
    if n >= 4000 {
        return n.to_string();
    }
    const DIGITS: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let (mut n, mut out) = (n, String::new());
    for (value, digits) in DIGITS {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

const ALPHABET: [char; 26] = [
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z',
];

// Construct alphanumeric marker from index, appending characters when index exceeds powers of 26
fn build_alpha_marker(index: usize, str: &mut String) {
    let rem = index % 26;
    let sym = ALPHABET[rem];
    str.insert(0, sym);
    let rest = (index - rem) as i64 / 26 - 1;
    if rest >= 0 {
        build_alpha_marker(rest as usize, str);
    }
}

#[cfg(test)]
mod tests {
    use crate::node::Marker;
    use style::{
        Atom,
        counter_style::CounterStyle,
        values::{CustomIdent, computed::ListStyleType},
    };

    use super::marker_for_style;

    fn list_style(s: &str) -> ListStyleType {
        ListStyleType(CounterStyle::Name(CustomIdent(Atom::from(s))))
    }

    #[test]
    fn test_marker_for_disc() {
        let result = marker_for_style(&ListStyleType::disc(), 1);
        assert_eq!(result, Some(Marker::Char('•')));
    }

    #[test]
    fn test_marker_for_decimal() {
        let result_1 = marker_for_style(&list_style("decimal"), 1);
        let result_2 = marker_for_style(&list_style("decimal"), 2);
        assert_eq!(result_1, Some(Marker::String("1. ".to_string())));
        assert_eq!(result_2, Some(Marker::String("2. ".to_string())));
    }

    #[test]
    fn test_marker_for_lower_alpha() {
        let result_1 = marker_for_style(&list_style("lower-alpha"), 1);
        let result_2 = marker_for_style(&list_style("lower-alpha"), 2);
        let result_extended_1 = marker_for_style(&list_style("lower-alpha"), 27);
        let result_extended_2 = marker_for_style(&list_style("lower-alpha"), 28);
        assert_eq!(result_1, Some(Marker::String("a. ".to_string())));
        assert_eq!(result_2, Some(Marker::String("b. ".to_string())));
        assert_eq!(result_extended_1, Some(Marker::String("aa. ".to_string())));
        assert_eq!(result_extended_2, Some(Marker::String("ab. ".to_string())));
    }

    #[test]
    fn test_marker_for_upper_alpha() {
        let result_1 = marker_for_style(&list_style("upper-alpha"), 1);
        let result_2 = marker_for_style(&list_style("upper-alpha"), 2);
        let result_extended_1 = marker_for_style(&list_style("upper-alpha"), 27);
        let result_extended_2 = marker_for_style(&list_style("upper-alpha"), 28);
        assert_eq!(result_1, Some(Marker::String("A. ".to_string())));
        assert_eq!(result_2, Some(Marker::String("B. ".to_string())));
        assert_eq!(result_extended_1, Some(Marker::String("AA. ".to_string())));
        assert_eq!(result_extended_2, Some(Marker::String("AB. ".to_string())));
    }
}
