//! pyro: CSS counters and quotes in generated content (CSS Lists 3 4, CSS 2.1 12.3-12.4).
//!
//! Before construction, a walk over the document in tree order applies each box's
//! `counter-reset`, `counter-increment` and `counter-set` (in that order, as Gecko does; a list
//! item increments `list-item` too) and evaluates the `content` of every `::before` and `::after`
//! that generates a box: its strings, `counter()` and `counters()` in their counter styles, and
//! `open-quote`/`close-quote` at the document's quote depth. The texts land in
//! `BaseDocument::generated_text`, which pseudo-element construction reads
//! (`construct::flush_pseudo_elements`); an owner whose texts changed is damaged so that its box
//! is built again.
//!
//! A counter is visible to the element that instantiates it, its descendants and its following
//! siblings with theirs, so it ends with the parent of the element that made it. The walk runs
//! only once a style traversal kept a `::before`/`::after` whose content reads counters or quotes
//! (`BaseDocument::generated_counters`, sticky), and only for a resolve with damage somewhere.

use blitz_traits::node_id::NodeId;
use std::collections::HashMap;
use style::{
    counter_style::{CounterStyle, Symbol, SymbolsType},
    properties::{style_structs, ComputedValues},
    selector_parser::PseudoElement,
    servo_arc::Arc,
    url::ComputedUrl,
    values::computed::{Content, ContentItem, Display, Image},
    values::specified::list::Quotes,
    Atom,
};
use stylo_taffy::CopyStyleGetters;

use crate::{
    net::{ImageHandler, ResourceHandler},
    node::SpecialElementData,
    util::ImageType,
    BaseDocument, NodeData,
};

/// What counters and quotes read of an element's styles: its own and its `::before`/`::after`
/// styles' counter properties and `content`, `display` and `quotes`. An element whose inputs a
/// restyle left equal changes no counter or quote, so the next evaluation can be skipped
/// (`BaseDocument::counters_dirty`).
pub(crate) struct CounterInputs([Option<(Arc<style_structs::Counters>, Display, Arc<style_structs::List>)>; 3]);

impl CounterInputs {
    pub(crate) fn of(styles: &style::data::ElementStyles) -> Self {
        let inputs = |style: Option<&Arc<ComputedValues>>| {
            style.map(|style| (style.clone_counters(), style.clone_display(), style.clone_list()))
        };
        let pseudo = |pseudo: PseudoElement| match styles.pseudos.is_empty() {
            true => None,
            false => inputs(styles.pseudos.get(&pseudo)),
        };
        Self([inputs(styles.primary.as_ref()), pseudo(PseudoElement::Before), pseudo(PseudoElement::After)])
    }
}

impl PartialEq for CounterInputs {
    fn eq(&self, other: &Self) -> bool {
        self.0.iter().zip(&other.0).all(|pair| match pair {
            (None, None) => true,
            (Some(a), Some(b)) => {
                (Arc::ptr_eq(&a.0, &b.0) || a.0 == b.0)
                    && a.1 == b.1
                    && (Arc::ptr_eq(&a.2, &b.2) || a.2.quotes == b.2.quotes)
            }
            _ => false,
        })
    }
}

/// A piece of generated content: text, or an image (its absolute URL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    /// An image's URL and its pixels per CSS px (an `image-set()` option's resolution, as the
    /// bits of an `f32`: pieces are compared).
    Image(String, u32),
}

/// The generated content of an element's `::after` (0) and `::before` (1), the pseudo-element
/// indices of `Node::pe_by_index`.
pub(crate) type GeneratedText = [Option<Vec<Piece>>; 2];

/// Whether `content` in this style reads a counter or a quote.
pub(crate) fn reads_counters(style: &ComputedValues) -> bool {
    match &style.get_counters().content {
        Content::Items(items) => items.items.iter().any(|item| {
            matches!(
                item,
                ContentItem::Counter(..)
                    | ContentItem::Counters(..)
                    | ContentItem::OpenQuote
                    | ContentItem::CloseQuote
                    | ContentItem::NoOpenQuote
                    | ContentItem::NoCloseQuote
            )
        }),
        _ => false,
    }
}

/// Whether a pseudo-element with this style generates a box.
fn generates(style: &ComputedValues) -> bool {
    !matches!(style.get_counters().content, Content::Normal | Content::None) && style.clone_display() != Display::None
}

/// The content a pseudo-element generates, evaluated on its own (counters are 0 and quotes at
/// depth 0): what it is when no counter or quote is in the document.
pub(crate) fn plain_content(style: &ComputedValues, owner: NodeId) -> Option<Vec<Piece>> {
    Walk::default().content(style, owner)
}

/// A counter instance: its value, and the element whose subtree it ends with (the parent of the
/// element that instantiated it).
struct Instance {
    value: i32,
    scope: NodeId,
    /// pyro: whether the following siblings of the box that made it see it (its scope is then
    /// that box's parent; otherwise the box itself).
    sibling: bool,
}

/// pyro: the scope of counters a `::before` or `::after` makes for itself alone (an id no node
/// has).
fn pseudo_scope() -> NodeId {
    NodeId::from_u64(u64::MAX)
}

#[derive(Default)]
struct Walk {
    counters: HashMap<Atom, Vec<Instance>>,
    /// Each instance made, in order, by name: popped as their scopes end.
    made: Vec<(NodeId, Atom)>,
    quote_depth: usize,
    texts: HashMap<NodeId, GeneratedText>,
}

impl Walk {
    /// `counter-reset` on a box whose parent is `scope` (`own`: the box itself). CSS Lists 3
    /// §4.4.1 "inheriting counters", as Chromium and WebKit have it: the new counter replaces one
    /// a previous sibling made; when the parent already has a counter of that name (from an
    /// ancestor), the siblings after the box keep that one and the new counter is the box's and
    /// its descendants' alone (it ends with the box); otherwise the siblings after it inherit it
    /// (it ends with the parent).
    ///
    /// `legacy`: the `list-item` counter an HTML list (`ol`, `ul`, `menu`, `dir`) resets is scoped
    /// as CSS 2.1 had it, its following siblings inheriting it whatever the parent has (WPT
    /// `counter-list-item-2`: items after a list nested in a list continue the nested one).
    fn reset(&mut self, name: &Atom, value: i32, scope: NodeId, own: NodeId, legacy: bool) {
        let stack = self.counters.entry(name.clone()).or_default();
        match stack.last_mut() {
            Some(innermost) if innermost.scope == scope && innermost.sibling => innermost.value = value,
            Some(_) if !legacy => {
                stack.push(Instance { value, scope: own, sibling: false });
                self.made.push((own, name.clone()));
            }
            _ => {
                stack.push(Instance { value, scope, sibling: true });
                self.made.push((scope, name.clone()));
            }
        }
    }

    /// The innermost counter `name`, instantiated at 0 on the box when there is none.
    fn innermost(&mut self, name: &Atom, scope: NodeId, own: NodeId) -> &mut Instance {
        if self.counters.get(name).is_none_or(|stack| stack.is_empty()) {
            self.reset(name, 0, scope, own, false);
        }
        self.counters.get_mut(name).unwrap().last_mut().unwrap()
    }

    /// The counter properties of a box (an element, `own`, or a pseudo-element) whose parent is
    /// `scope`; `list` for an HTML list element (see [`Walk::reset`]).
    fn apply(&mut self, style: &ComputedValues, scope: NodeId, own: NodeId, list_item: bool, list: bool) {
        let counters = style.get_counters();
        let list_item_name = Atom::from("list-item");
        for pair in counters.counter_reset.iter() {
            let legacy = list && pair.name.0 == list_item_name;
            self.reset(&pair.name.0, pair.value, scope, own, legacy);
        }
        // pyro: an increment past the integer range leaves the counter as it was (Blink's
        // `CalculateCounterValue`).
        let add = |value: i32, by: i32| value.checked_add(by).unwrap_or(value);
        let mut increments_list_item = false;
        for pair in counters.counter_increment.iter() {
            increments_list_item |= pair.name.0 == list_item_name;
            let instance = self.innermost(&pair.name.0, scope, own);
            instance.value = add(instance.value, pair.value);
        }
        if list_item && !increments_list_item {
            let instance = self.innermost(&list_item_name, scope, own);
            instance.value = add(instance.value, 1);
        }
        for pair in counters.counter_set.iter() {
            self.innermost(&pair.name.0, scope, own).value = pair.value;
        }
    }

    /// The counters the children of `parent` made end with it.
    fn leave(&mut self, parent: NodeId) {
        while self.made.last().is_some_and(|(scope, _)| *scope == parent) {
            let (_, name) = self.made.pop().unwrap();
            if let Some(stack) = self.counters.get_mut(&name) {
                stack.pop();
            }
        }
    }

    /// The content of a pseudo-element whose originating element is `owner`: its text (strings,
    /// counters and quotes, joined) and images, in order.
    fn content(&mut self, style: &ComputedValues, owner: NodeId) -> Option<Vec<Piece>> {
        let Content::Items(items) = &style.get_counters().content else {
            return None;
        };
        let mut pieces: Vec<Piece> = Vec::new();
        fn push(pieces: &mut Vec<Piece>, text: &str) {
            match pieces.last_mut() {
                Some(Piece::Text(last)) => last.push_str(text),
                _ => pieces.push(Piece::Text(text.to_string())),
            }
        }
        for item in &items.items[0..items.alt_start] {
            match item {
                ContentItem::String(string) => push(&mut pieces, string),
                ContentItem::Counter(name, counter_style) => {
                    let value = self.innermost(&name.0, owner, pseudo_scope()).value;
                    push(&mut pieces, &format(value, counter_style));
                }
                ContentItem::Counters(name, separator, counter_style) => {
                    self.innermost(&name.0, owner, pseudo_scope());
                    let mut text = String::new();
                    for (index, instance) in self.counters[&name.0].iter().enumerate() {
                        if index > 0 {
                            text.push_str(separator);
                        }
                        text.push_str(&format(instance.value, counter_style));
                    }
                    push(&mut pieces, &text);
                }
                ContentItem::OpenQuote => {
                    let open = quote(style, self.quote_depth).map(|(open, _)| open).unwrap_or_default();
                    push(&mut pieces, &open);
                    self.quote_depth += 1;
                }
                ContentItem::CloseQuote => {
                    let mut close = String::new();
                    if self.quote_depth > 0 {
                        self.quote_depth -= 1;
                        close = quote(style, self.quote_depth).map(|(_, close)| close).unwrap_or_default();
                    }
                    push(&mut pieces, &close);
                }
                ContentItem::NoOpenQuote => {
                    self.quote_depth += 1;
                    push(&mut pieces, "");
                }
                ContentItem::NoCloseQuote => {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                    push(&mut pieces, "");
                }
                ContentItem::Image(image) => {
                    // pyro: an `image-set()` is the option it selected.
                    if let Image::Url(ComputedUrl::Valid(url)) = crate::util::unfiltered_image(image) {
                        pieces.push(Piece::Image(url.as_str().to_string(), crate::util::image_resolution(image).to_bits()));
                    }
                }
            }
        }
        (!pieces.is_empty()).then_some(pieces)
    }
}

/// The quotes at `depth` (the last pair beyond the list's end; none for `quotes: none`). `auto`
/// is English's (Chromium's default).
pub(super) fn quote(style: &ComputedValues, depth: usize) -> Option<(String, String)> {
    match &style.get_list().quotes {
        Quotes::QuoteList(list) => {
            let pairs = &list.0;
            let pair = pairs.get(depth).or(pairs.last())?;
            Some((pair.opening.to_string(), pair.closing.to_string()))
        }
        Quotes::Auto => Some(match depth {
            0 => ("\u{201C}".into(), "\u{201D}".into()),
            _ => ("\u{2018}".into(), "\u{2019}".into()),
        }),
    }
}

impl BaseDocument {
    /// Brings generated content up to date after a style traversal, before damage propagates: a
    /// `::first-letter` style that changed restyles the letter's inline layout
    /// (`first_letter::style_changed`), and counters and quotes are evaluated (see the module).
    pub(crate) fn resolve_generated_content(&mut self) {
        for node_id in std::mem::take(&mut self.first_letter_changed) {
            super::first_letter::style_changed(self, node_id);
        }
        for node_id in std::mem::take(&mut self.marker_items_restyled) {
            super::list::item_restyled(self, node_id);
        }
        for node_id in std::mem::take(&mut self.first_line_changed) {
            super::first_line::style_changed(self, node_id);
        }
        if !self.generated_counters || !self.counters_dirty {
            return;
        }
        self.counters_dirty = false;
        let root_id = self.root_element().id;
        let document_id = self.root_node().id;
        let mut walk = Walk::default();
        walk_element(self, root_id, document_id, &mut walk);
        let texts = walk.texts;

        // An owner whose texts changed builds its box again (`flush_pseudo_elements`).
        let old = std::mem::take(&mut self.generated_text);
        let changed: Vec<NodeId> = texts
            .iter()
            .filter(|(id, text)| old.get(*id) != Some(*text))
            .map(|(id, _)| *id)
            .chain(old.keys().filter(|id| !texts.contains_key(*id)).copied())
            .collect();
        self.generated_text = texts;
        for node_id in changed {
            if let Some(node) = self.nodes.get_mut(node_id) {
                if node.flags.is_in_document() {
                    node.insert_damage(super::damage::ALL_DAMAGE);
                    node.mark_damaged();
                }
            }
        }
    }
}

impl BaseDocument {
    /// Fetches the image of a generated `img` box (`content: url()`), as an `<img src>` loads.
    pub(crate) fn load_generated_image(&mut self, node_id: NodeId, url: &str) {
        if let Some(image) = self.image_cache.get(url) {
            let image = Box::new(image.clone());
            if let Some(element) = self.nodes[node_id].element_data_mut() {
                element.special_data = SpecialElementData::Image(image);
            }
            return;
        }
        if let Some(waiting) = self.pending_images.get_mut(url) {
            waiting.push((node_id, ImageType::Image));
            return;
        }
        let Ok(parsed) = url::Url::parse(url) else {
            return;
        };
        self.pending_images.insert(url.to_string(), vec![(node_id, ImageType::Image)]);
        let doc_id = self.id();
        self.net_provider.fetch(
            doc_id,
            self.build_request(parsed),
            ResourceHandler::boxed(
                self.tx.clone(),
                doc_id,
                None,
                self.shell_provider.clone(),
                ImageHandler::new(ImageType::Image),
            ),
        );
    }
}

fn walk_element(doc: &BaseDocument, node_id: NodeId, parent: NodeId, walk: &mut Walk) {
    let node = &doc.nodes[node_id];
    // Unstyled (inside `display: none`) or boxless: no counters here or below.
    let Some(style) = node.primary_styles() else { return };
    let display = style.clone_display();
    if display == Display::None {
        return;
    }
    // A `display: contents` element generates no box: its counter properties do nothing.
    let list = node.element_data().is_some_and(|element| {
        matches!(
            element.name.local,
            markup5ever::local_name!("ol")
                | markup5ever::local_name!("ul")
                | markup5ever::local_name!("menu")
                | markup5ever::local_name!("dir")
        )
    });
    if !display.is_contents() {
        walk.apply(&style, parent, node_id, display.is_list_item(), list);
    }
    drop(style);
    // pyro: its children and its `::before` and `::after` are then its parent's in the box tree,
    // which counters follow (Blink counts layout objects; WPT `counter-order-display-contents`).
    let scope = if display.is_contents() { parent } else { node_id };
    let pseudos: [Option<Arc<ComputedValues>>; 2] = match node.try_stylo_element_data().and_then(|data| data.get()) {
        Some(data) if !data.styles.pseudos.is_empty() => [
            data.styles.pseudos.get(&PseudoElement::After).cloned(),
            data.styles.pseudos.get(&PseudoElement::Before).cloned(),
        ],
        _ => [None, None],
    };
    let mut texts: GeneratedText = [None, None];
    if let Some(before) = pseudos[1].as_deref().filter(|style| generates(style)) {
        walk.apply(before, scope, pseudo_scope(), before.clone_display().is_list_item(), false);
        texts[1] = walk.content(before, scope);
        walk.leave(pseudo_scope());
    }
    for &child_id in node.children.iter() {
        if matches!(doc.nodes[child_id].data, NodeData::Element(_)) {
            crate::util::ensure_stack(|| walk_element(doc, child_id, scope, walk));
        }
    }
    if let Some(after) = pseudos[0].as_deref().filter(|style| generates(style)) {
        walk.apply(after, scope, pseudo_scope(), after.clone_display().is_list_item(), false);
        texts[0] = walk.content(after, scope);
        walk.leave(pseudo_scope());
    }
    walk.leave(node_id);
    if texts.iter().any(Option::is_some) {
        walk.texts.insert(node_id, texts);
    }
}

/// `value` in a counter style (CSS Counter Styles 3: the predefined styles; an unknown name is
/// `decimal`).
pub(crate) fn format(value: i32, style: &CounterStyle) -> String {
    match style {
        CounterStyle::None => String::new(),
        CounterStyle::String(string) => string.as_ref().to_string(),
        CounterStyle::Name(name) => predefined(value, &name.0),
        CounterStyle::Symbols { ty, symbols } => {
            let symbols: Vec<String> = symbols
                .0
                .iter()
                .map(|symbol| match symbol {
                    Symbol::String(string) => string.to_string(),
                    Symbol::Ident(ident) => ident.0.to_string(),
                })
                .collect();
            let symbols: Vec<&str> = symbols.iter().map(String::as_str).collect();
            let text = match ty {
                SymbolsType::Cyclic => cyclic(value, &symbols),
                SymbolsType::Numeric => numeric(value, &symbols),
                SymbolsType::Alphabetic => alphabetic(value, &symbols),
                SymbolsType::Symbolic => symbolic(value, &symbols),
                SymbolsType::Fixed => fixed(value, &symbols),
            };
            text.unwrap_or_else(|| value.to_string())
        }
    }
}

const LOWER_LATIN: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w",
    "x", "y", "z",
];
const UPPER_LATIN: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W",
    "X", "Y", "Z",
];
const LOWER_GREEK: [&str; 24] = [
    "\u{3B1}", "\u{3B2}", "\u{3B3}", "\u{3B4}", "\u{3B5}", "\u{3B6}", "\u{3B7}", "\u{3B8}", "\u{3B9}", "\u{3BA}",
    "\u{3BB}", "\u{3BC}", "\u{3BD}", "\u{3BE}", "\u{3BF}", "\u{3C0}", "\u{3C1}", "\u{3C3}", "\u{3C4}", "\u{3C5}",
    "\u{3C6}", "\u{3C7}", "\u{3C8}", "\u{3C9}",
];
const ROMAN: [(u32, &str); 13] = [
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
/// Upper-case Armenian by weight, 9000 down to 1 (U+0531..U+0554).
const ARMENIAN: [(u32, u32); 36] = [
    (9000, 0x554),
    (8000, 0x553),
    (7000, 0x552),
    (6000, 0x551),
    (5000, 0x550),
    (4000, 0x54F),
    (3000, 0x54E),
    (2000, 0x54D),
    (1000, 0x54C),
    (900, 0x54B),
    (800, 0x54A),
    (700, 0x549),
    (600, 0x548),
    (500, 0x547),
    (400, 0x546),
    (300, 0x545),
    (200, 0x544),
    (100, 0x543),
    (90, 0x542),
    (80, 0x541),
    (70, 0x540),
    (60, 0x53F),
    (50, 0x53E),
    (40, 0x53D),
    (30, 0x53C),
    (20, 0x53B),
    (10, 0x53A),
    (9, 0x539),
    (8, 0x538),
    (7, 0x537),
    (6, 0x536),
    (5, 0x535),
    (4, 0x534),
    (3, 0x533),
    (2, 0x532),
    (1, 0x531),
];
/// Georgian by weight, 10000 down to 1.
const GEORGIAN: [(u32, u32); 37] = [
    (10000, 0x10F5),
    (9000, 0x10F0),
    (8000, 0x10EF),
    (7000, 0x10F4),
    (6000, 0x10EE),
    (5000, 0x10ED),
    (4000, 0x10EC),
    (3000, 0x10EB),
    (2000, 0x10EA),
    (1000, 0x10E9),
    (900, 0x10E8),
    (800, 0x10E7),
    (700, 0x10E6),
    (600, 0x10E5),
    (500, 0x10E4),
    (400, 0x10F3),
    (300, 0x10E2),
    (200, 0x10E1),
    (100, 0x10E0),
    (90, 0x10DF),
    (80, 0x10DE),
    (70, 0x10DD),
    (60, 0x10F2),
    (50, 0x10DC),
    (40, 0x10DB),
    (30, 0x10DA),
    (20, 0x10D9),
    (10, 0x10D8),
    (9, 0x10D7),
    (8, 0x10F1),
    (7, 0x10D6),
    (6, 0x10D5),
    (5, 0x10D4),
    (4, 0x10D3),
    (3, 0x10D2),
    (2, 0x10D1),
    (1, 0x10D0),
];

fn predefined(value: i32, name: &Atom) -> String {
    let in_range = |max: i32| (1..=max).contains(&value);
    let by_weight = |table: &[(u32, u32)], lower: bool| {
        let symbols: Vec<(u32, String)> = table
            .iter()
            .filter_map(|&(weight, code)| {
                // Lower-case Armenian is 0x30 above upper-case.
                let code = if lower { code + 0x30 } else { code };
                Some((weight, char::from_u32(code)?.to_string()))
            })
            .collect();
        let symbols: Vec<(u32, &str)> = symbols.iter().map(|(weight, symbol)| (*weight, symbol.as_str())).collect();
        additive(value, &symbols)
    };
    let text = match &**name {
        "decimal" => None,
        "none" => Some(String::new()),
        "decimal-leading-zero" => Some(match value {
            0..=9 => format!("0{value}"),
            -9..=-1 => format!("-0{}", -value),
            _ => value.to_string(),
        }),
        "lower-roman" if in_range(3999) => additive(value, &ROMAN).map(|text| text.to_ascii_lowercase()),
        "upper-roman" if in_range(3999) => additive(value, &ROMAN),
        "lower-alpha" | "lower-latin" => alphabetic(value, &LOWER_LATIN),
        "upper-alpha" | "upper-latin" => alphabetic(value, &UPPER_LATIN),
        "lower-greek" => alphabetic(value, &LOWER_GREEK),
        "armenian" | "upper-armenian" if in_range(9999) => by_weight(&ARMENIAN, false),
        "lower-armenian" if in_range(9999) => by_weight(&ARMENIAN, true),
        "georgian" if in_range(19999) => by_weight(&GEORGIAN, false),
        "disc" => Some("\u{2022}".into()),
        "circle" => Some("\u{25E6}".into()),
        "square" => Some("\u{25AA}".into()),
        "disclosure-open" => Some("\u{25BE}".into()),
        "disclosure-closed" => Some("\u{25B8}".into()),
        "cjk-decimal" => numeric(
            value,
            &[
                "\u{3007}", "\u{4E00}", "\u{4E8C}", "\u{4E09}", "\u{56DB}", "\u{4E94}", "\u{516D}", "\u{4E03}",
                "\u{516B}", "\u{4E5D}",
            ],
        ),
        other => digits_from(other).and_then(|zero| {
            let digits: Vec<String> = (0..10).filter_map(|d| char::from_u32(zero + d)).map(String::from).collect();
            let digits: Vec<&str> = digits.iter().map(String::as_str).collect();
            numeric(value, &digits)
        }),
    };
    text.unwrap_or_else(|| value.to_string())
}

/// The zero digit of a predefined numeric style with ten consecutive digits.
fn digits_from(name: &str) -> Option<u32> {
    Some(match name {
        "arabic-indic" => 0x0660,
        "persian" => 0x06F0,
        "devanagari" => 0x0966,
        "bengali" => 0x09E6,
        "gurmukhi" => 0x0A66,
        "gujarati" => 0x0AE6,
        "oriya" => 0x0B66,
        "tamil" => 0x0BE6,
        "telugu" => 0x0C66,
        "kannada" => 0x0CE6,
        "malayalam" => 0x0D66,
        "thai" => 0x0E50,
        "lao" => 0x0ED0,
        "tibetan" => 0x0F20,
        "myanmar" => 0x1040,
        "khmer" | "cambodian" => 0x17E0,
        "mongolian" => 0x1810,
        _ => return None,
    })
}

fn cyclic(value: i32, symbols: &[&str]) -> Option<String> {
    let n = symbols.len() as i64;
    (n > 0).then(|| symbols[(i64::from(value) - 1).rem_euclid(n) as usize].to_string())
}

fn fixed(value: i32, symbols: &[&str]) -> Option<String> {
    let index = usize::try_from(i64::from(value) - 1).ok()?;
    symbols.get(index).map(|symbol| symbol.to_string())
}

fn symbolic(value: i32, symbols: &[&str]) -> Option<String> {
    let n = symbols.len() as u32;
    if value < 1 || n == 0 {
        return None;
    }
    let value = value as u32;
    let symbol = symbols[((value - 1) % n) as usize];
    Some(symbol.repeat(value.div_ceil(n) as usize))
}

fn alphabetic(value: i32, symbols: &[&str]) -> Option<String> {
    let n = symbols.len() as i64;
    if value < 1 || n < 2 {
        return None;
    }
    let mut value = i64::from(value);
    let mut parts = Vec::new();
    while value > 0 {
        value -= 1;
        parts.push(symbols[(value % n) as usize]);
        value /= n;
    }
    Some(parts.into_iter().rev().collect())
}

fn numeric(value: i32, symbols: &[&str]) -> Option<String> {
    let n = symbols.len() as i64;
    if n < 2 {
        return None;
    }
    let mut magnitude = i64::from(value).abs();
    let mut parts = Vec::new();
    loop {
        parts.push(symbols[(magnitude % n) as usize]);
        magnitude /= n;
        if magnitude == 0 {
            break;
        }
    }
    let digits: String = parts.into_iter().rev().collect();
    Some(if value < 0 { format!("-{digits}") } else { digits })
}

fn additive(value: i32, symbols: &[(u32, &str)]) -> Option<String> {
    let mut value = u32::try_from(value).ok().filter(|&value| value > 0)?;
    let mut text = String::new();
    for &(weight, symbol) in symbols {
        while weight > 0 && value >= weight {
            text.push_str(symbol);
            value -= weight;
        }
    }
    (value == 0).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::format;
    use style::{counter_style::CounterStyle, values::CustomIdent, Atom};

    fn named(value: i32, name: &str) -> String {
        format(value, &CounterStyle::Name(CustomIdent(Atom::from(name))))
    }

    #[test]
    fn predefined_styles() {
        assert_eq!(named(-3, "decimal"), "-3");
        assert_eq!(named(7, "decimal-leading-zero"), "07");
        assert_eq!(named(1994, "upper-roman"), "MCMXCIV");
        assert_eq!(named(4, "lower-roman"), "iv");
        assert_eq!(named(0, "upper-roman"), "0");
        assert_eq!(named(4000, "lower-roman"), "4000");
        assert_eq!(named(28, "lower-alpha"), "ab");
        assert_eq!(named(0, "upper-latin"), "0");
        assert_eq!(named(25, "lower-greek"), "\u{3B1}\u{3B1}");
        assert_eq!(named(1, "armenian"), "\u{531}");
        assert_eq!(named(1, "lower-armenian"), "\u{561}");
        assert_eq!(named(19, "georgian"), "\u{10D8}\u{10D7}");
        assert_eq!(named(5, "disc"), "\u{2022}");
        assert_eq!(named(3, "none-such"), "3");
        assert_eq!(format(3, &CounterStyle::None), "");
        assert_eq!(named(3, "none"), "");
    }
}
