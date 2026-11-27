//! pyro: the spacing around a MathML operator in a row (MathML Core § 3.2.4, "Operator, fence,
//! separator or accent"): its `lspace`/`rspace` attributes, else the operator dictionary's values
//! for its text and form. The spacing is the inline-start and inline-end margins of the
//! operator's *embellished operator* (§ 3.2.4.3: the outermost element above the `<mo>` that is
//! made only of it, scripts on it, and space-like siblings), a presentational hint
//! (`build_presentational_hints`), so it lies outside that box as in Chrome. An embellished
//! operator that is not a child of a row (a fraction's or a script's) takes none: fractions and
//! scripts are laid out elsewhere. The dictionary also carries the `stretchy`, `symmetric`,
//! `largeop`, `movablelimits` and `horizontal` properties (`Entry`) for the layout that stretches
//! operators.

use crate::node::{ElementData, Node, NodeData};
use blitz_traits::node_id::NodeId;
use markup5ever::{local_name, ns};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Form {
    Infix,
    Prefix,
    Postfix,
}

/// The default spacing of an operator the dictionary lacks: 5/18 em (thickmathspace) each side.
const DEFAULT_SPACING: (u8, u8) = (5, 5);

/// An operator's dictionary entry: its spacing in 1/18 em and its boolean properties.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Entry {
    pub lspace: u8,
    pub rspace: u8,
    flags: u8,
}

/// The dictionary's boolean properties, as bits of `Entry::flags`.
struct F;
impl F {
    const STRETCHY: u8 = 1;
    const SYMMETRIC: u8 = 2;
    const LARGEOP: u8 = 4;
    const MOVABLELIMITS: u8 = 8;
    /// Stretches along the inline axis (over and under bars, arrows, braces) instead of the block axis.
    const HORIZONTAL: u8 = 16;
}

// Read by the operator-stretching layout (not in this module); the dictionary data is complete.
#[allow(dead_code)]
impl Entry {
    pub(crate) fn stretchy(self) -> bool {
        self.flags & F::STRETCHY != 0
    }
    pub(crate) fn symmetric(self) -> bool {
        self.flags & F::SYMMETRIC != 0
    }
    pub(crate) fn largeop(self) -> bool {
        self.flags & F::LARGEOP != 0
    }
    pub(crate) fn movablelimits(self) -> bool {
        self.flags & F::MOVABLELIMITS != 0
    }
    pub(crate) fn horizontal(self) -> bool {
        self.flags & F::HORIZONTAL != 0
    }
}

/// An `<mo>`'s inline-start and inline-end spacing (`spacing`, `None` when the operator takes no
/// row spacing).
pub(crate) struct Spacing {
    pub start: Side,
    pub end: Side,
}

pub(crate) enum Side {
    /// The dictionary's value, in 1/18 em.
    Eighteenths(u8),
    /// An `lspace`/`rspace` attribute's value as written, and the dictionary's value for when it
    /// does not parse as a `<length>`.
    Explicit(String, u8),
}

/// How a MathML element takes part in rows, embellished operators and space-likeness.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `<mo>`.
    Operator,
    /// `mi`, `mn`, `ms` (never space-like), `mtext`, `mspace` (space-like).
    Token { space_like: bool },
    /// `mrow`, `mstyle`, `mphantom`, `merror`, `mpadded`, `menclose`, `a`, an unknown element and a
    /// script, fraction or radical with the wrong number of children (laid out as an `mrow`,
    /// MathML Core § 3.1): a row that is embellished or space-like by its in-flow children.
    Row,
    /// `math`, `msqrt` and `mtd`: a row for its children, but never itself an embellished operator
    /// or space-like.
    Container,
    /// A valid `msub`, `msup`, `msubsup`, `mmultiscripts`, `munder`, `mover`, `munderover` or
    /// `mfrac`: embellished when its first in-flow child is, never space-like, not a row.
    Scripts,
    /// `semantics` and `maction`: a row of which only the first child is rendered; embellished and
    /// space-like as that child (space-like also without one).
    Semantic,
    /// Everything else with a layout of its own (a valid `mroot`, tables, annotations).
    Other,
}

/// The deepest chain of embellishing elements examined: a formula nests a few levels, and a bound
/// keeps a hostile one linear.
const MAX_DEPTH: u32 = 64;

fn mathml(node: &Node) -> Option<&ElementData> {
    node.data.downcast_element().filter(|e| e.name.ns == ns!(mathml))
}

/// The element children that take part in layout: all of them, but a `semantics` or `maction`
/// renders only its first (the UA sheet hides the others).
fn in_flow<'a>(node: &'a Node, elem: &ElementData) -> Vec<&'a Node> {
    let mut elements = node.children.iter().map(|c| node.with(*c)).filter(|c| matches!(c.data, NodeData::Element(_)));
    if matches!(&*elem.name.local, "semantics" | "maction") {
        elements.next().into_iter().collect()
    } else {
        elements.collect()
    }
}

fn kind(node: &Node, elem: &ElementData) -> Kind {
    let count = || node.children.iter().filter(|c| matches!(node.with(**c).data, NodeData::Element(_))).count();
    match &*elem.name.local {
        "mo" => Kind::Operator,
        "mtext" | "mspace" => Kind::Token { space_like: true },
        "mi" | "mn" | "ms" => Kind::Token { space_like: false },
        "math" | "msqrt" | "mtd" => Kind::Container,
        "semantics" | "maction" => Kind::Semantic,
        "msub" | "msup" | "munder" | "mover" => if count() == 2 { Kind::Scripts } else { Kind::Row },
        "msubsup" | "munderover" => if count() == 3 { Kind::Scripts } else { Kind::Row },
        "mmultiscripts" => if count() >= 1 { Kind::Scripts } else { Kind::Row },
        // A fraction is embellished through its numerator (MathML Core § 3.2.4.3).
        "mfrac" => if count() == 2 { Kind::Scripts } else { Kind::Row },
        "mroot" => if count() == 2 { Kind::Other } else { Kind::Row },
        "mtable" | "mtr" | "mlabeledtr" | "annotation" | "annotation-xml" => Kind::Other,
        _ => Kind::Row,
    }
}

/// Whether `kind` lays its children out as a row, so that an operator among them is spaced.
fn lays_out_row(kind: Kind) -> bool {
    matches!(kind, Kind::Row | Kind::Container | Kind::Semantic)
}

/// MathML Core § 3.2.4.4: whether `node` is space-like (`mtext`, `mspace`, and rows or
/// `semantics`/`maction` made of space-like elements, an empty one included).
fn space_like(node: &Node, depth: u32) -> bool {
    let Some(elem) = mathml(node) else { return false };
    if depth > MAX_DEPTH {
        return false;
    }
    crate::util::ensure_stack(|| match kind(node, elem) {
        Kind::Token { space_like: yes } => yes,
        Kind::Row => in_flow(node, elem).into_iter().all(|c| space_like(c, depth + 1)),
        Kind::Semantic => in_flow(node, elem).first().is_none_or(|c| space_like(c, depth + 1)),
        _ => false,
    })
}

/// MathML Core § 3.2.4.3: the `<mo>` that is the core of the embellished operator `node` (itself
/// when it is an `<mo>`), if it is one.
fn core_operator(node: &Node, depth: u32) -> Option<&Node> {
    let elem = mathml(node)?;
    if depth > MAX_DEPTH {
        return None;
    }
    crate::util::ensure_stack(|| match kind(node, elem) {
        Kind::Operator => Some(node),
        Kind::Scripts | Kind::Semantic => in_flow(node, elem).first().copied().and_then(|c| core_operator(c, depth + 1)),
        Kind::Row => {
            let mut core = None;
            for child in in_flow(node, elem) {
                if space_like(child, depth + 1) {
                    continue;
                }
                if core.is_some() {
                    return None;
                }
                core = Some(core_operator(child, depth + 1)?);
            }
            core
        }
        _ => None,
    })
}

/// Whether `parent` is an embellished operator because its child `child` (an embellished operator)
/// is: the first child of a script or `semantics`, or the only child that is not space-like.
fn continues_embellishment(parent: &Node, child: &Node) -> bool {
    let Some(elem) = mathml(parent) else { return false };
    match kind(parent, elem) {
        Kind::Scripts | Kind::Semantic => in_flow(parent, elem).first().is_some_and(|c| c.id == child.id),
        Kind::Row => in_flow(parent, elem).into_iter().all(|c| c.id == child.id || space_like(c, 1)),
        _ => false,
    }
}

/// Whether `node` can be part of an embellished operator above its `<mo>`.
fn can_embellish(node: &Node) -> bool {
    mathml(node).is_some_and(|e| matches!(kind(node, e), Kind::Operator | Kind::Row | Kind::Scripts | Kind::Semantic))
}

/// The elements whose spacing hints must be built again after `node`'s attributes, text or
/// children changed: the element itself, its element children (their position in a row decides
/// their form) and the embellished operators above it.
pub(crate) fn respace_targets(node: &Node) -> Vec<NodeId> {
    if mathml(node).is_none() {
        return Vec::new();
    }
    let mut targets = vec![node.id];
    fn add(targets: &mut Vec<NodeId>, id: NodeId) {
        if !targets.contains(&id) {
            targets.push(id);
        }
    }
    for &c in node.children.iter().filter(|c| mathml(node.with(**c)).is_some()) {
        add(&mut targets, c);
    }
    let mut current = node;
    while can_embellish(current) {
        let Some(parent) = current.parent.map(|id| node.with(id)) else { break };
        if mathml(parent).is_none() {
            break;
        }
        // The parent's other children: whether the parent is embellished through them changes.
        add(&mut targets, parent.id);
        for &c in parent.children.iter().filter(|c| mathml(node.with(**c)).is_some()) {
            add(&mut targets, c);
        }
        current = parent;
    }
    targets
}

/// The spacing hints of `node`: those of the embellished operator it is the outermost element of,
/// when its parent lays its children out as a row.
pub(crate) fn spacing(node: &Node, elem: &ElementData) -> Option<Spacing> {
    if elem.name.ns != ns!(mathml) || !matches!(kind(node, elem), Kind::Operator | Kind::Row | Kind::Scripts | Kind::Semantic) {
        return None;
    }
    let core = core_operator(node, 0)?;
    let parent = node.parent.map(|id| node.with(id))?;
    let parent_elem = mathml(parent)?;
    if !lays_out_row(kind(parent, parent_elem)) || continues_embellishment(parent, node) {
        return None;
    }
    let core_elem = core.data.downcast_element()?;
    let form = explicit_form(core_elem).unwrap_or_else(|| row_form(parent, parent_elem, node.id));
    let text = core.text_content();
    let found = entry(text.trim_matches(|c: char| c.is_ascii_whitespace()), form);
    let side = |attr: &str, default: u8| {
        core_elem
            .attrs()
            .iter()
            .find(|a| a.name.ns == ns!() && &*a.name.local == attr)
            .map(|a| a.value.trim())
            .filter(|v| valid_space(v))
            .map_or(Side::Eighteenths(default), |v| Side::Explicit(v.to_string(), default))
    };
    Some(Spacing { start: side("lspace", found.lspace), end: side("rspace", found.rspace) })
}

fn explicit_form(elem: &ElementData) -> Option<Form> {
    let value = elem.attrs().iter().find(|a| a.name.ns == ns!() && a.name.local == local_name!("form"))?;
    match value.value.to_ascii_lowercase().as_str() {
        "infix" => Some(Form::Infix),
        "prefix" => Some(Form::Prefix),
        "postfix" => Some(Form::Postfix),
        _ => None,
    }
}

/// MathML Core § 3.2.4.1: the first of several in-flow children of its row is a prefix, the last a
/// postfix, any other (or an only child) an infix operator.
fn row_form(parent: &Node, parent_elem: &ElementData, id: NodeId) -> Form {
    let children = in_flow(parent, parent_elem);
    match (children.first(), children.last()) {
        (Some(first), Some(last)) if children.len() > 1 => {
            if first.id == id {
                Form::Prefix
            } else if last.id == id {
                Form::Postfix
            } else {
                Form::Infix
            }
        }
        _ => Form::Infix,
    }
}

/// A `<length>` without a sign or a percentage: the values Chrome honours (anything else falls
/// back to the dictionary).
fn valid_space(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains('%')
}

/// MathML Core § 3.2.4.1, "Dictionary-based attributes": the entry for (content, form), else for
/// the content in the other forms in the order infix, postfix, prefix, else, for content that is a
/// character followed by a combining U+0338 or U+20D2, the same for that character alone, else the
/// default (5/18 em each side, no properties).
pub(crate) fn entry(content: &str, form: Form) -> Entry {
    let find = |content: &str, form: Form| {
        DICTIONARY
            .binary_search_by(|(c, f, ..)| c.as_bytes().cmp(content.as_bytes()).then(f.cmp(&form)))
            .ok()
            .map(|i| Entry { lspace: DICTIONARY[i].2, rspace: DICTIONARY[i].3, flags: DICTIONARY[i].4 })
    };
    let find_any_form = |content: &str| {
        find(content, form).or_else(|| [Form::Infix, Form::Postfix, Form::Prefix].into_iter().filter(|f| *f != form).find_map(|f| find(content, f)))
    };
    find_any_form(content)
        .or_else(|| {
            let mut chars = content.chars();
            chars.next_back().filter(|c| matches!(c, '\u{338}' | '\u{20D2}'))?;
            let base = chars.as_str();
            if base.chars().count() == 1 { find_any_form(base) } else { None }
        })
        .unwrap_or(Entry { lspace: DEFAULT_SPACING.0, rspace: DEFAULT_SPACING.1, flags: 0 })
}

/// MathML Core's operator dictionary, (content, form, lspace, rspace, properties) with the spacing
/// in 1/18 em, sorted by the content's UTF-8 bytes and then the form. Generated from WPT
/// `mathml/support/operator-dictionary.json` (sha256 ec331ebfcd13db2286572ef0a60a0059104b3c3cfb092cb581c4cdfeac090340,
/// WPT a421f2fea5eb); the U+00A0 entries, which have the default values, are left out.
#[rustfmt::skip]
static DICTIONARY: &[(&str, Form, u8, u8, u8)] = &[
    ("!", Form::Prefix, 0, 0, 0),
    ("!", Form::Postfix, 0, 0, 0),
    ("!!", Form::Postfix, 0, 0, 0),
    ("!=", Form::Infix, 5, 5, 0),
    ("\"", Form::Postfix, 0, 0, 0),
    ("%", Form::Infix, 3, 3, 0),
    ("%", Form::Postfix, 0, 0, 0),
    ("&", Form::Postfix, 0, 0, 0),
    ("&&", Form::Infix, 4, 4, 0),
    ("'", Form::Postfix, 0, 0, 0),
    ("(", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    (")", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("*", Form::Infix, 3, 3, 0),
    ("**", Form::Infix, 3, 3, 0),
    ("*=", Form::Infix, 5, 5, 0),
    ("+", Form::Infix, 4, 4, 0),
    ("+", Form::Prefix, 0, 0, 0),
    ("++", Form::Postfix, 0, 0, 0),
    ("+=", Form::Infix, 5, 5, 0),
    (",", Form::Infix, 0, 3, 0),
    ("-", Form::Infix, 4, 4, 0),
    ("-", Form::Prefix, 0, 0, 0),
    ("--", Form::Postfix, 0, 0, 0),
    ("-=", Form::Infix, 5, 5, 0),
    ("->", Form::Infix, 5, 5, 0),
    (".", Form::Infix, 3, 3, 0),
    ("/", Form::Infix, 4, 4, 0),
    ("//", Form::Infix, 5, 5, 0),
    ("/=", Form::Infix, 5, 5, 0),
    (":", Form::Infix, 0, 3, 0),
    (":=", Form::Infix, 5, 5, 0),
    (";", Form::Infix, 0, 3, 0),
    ("<", Form::Infix, 5, 5, 0),
    ("<=", Form::Infix, 5, 5, 0),
    ("<>", Form::Infix, 3, 3, 0),
    ("=", Form::Infix, 5, 5, F::HORIZONTAL),
    ("==", Form::Infix, 5, 5, 0),
    (">", Form::Infix, 5, 5, 0),
    (">=", Form::Infix, 5, 5, 0),
    ("?", Form::Infix, 3, 3, 0),
    ("@", Form::Infix, 3, 3, 0),
    ("[", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\\", Form::Infix, 0, 0, 0),
    ("]", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("^", Form::Infix, 3, 3, F::HORIZONTAL),
    ("^", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("_", Form::Infix, 0, 0, F::HORIZONTAL),
    ("_", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("`", Form::Postfix, 0, 0, 0),
    ("{", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("|", Form::Infix, 5, 5, 0),
    ("|", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("|", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("||", Form::Infix, 5, 5, 0),
    ("||", Form::Prefix, 0, 0, 0),
    ("||", Form::Postfix, 0, 0, 0),
    ("}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("~", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{A8}", Form::Postfix, 0, 0, 0),
    ("\u{AC}", Form::Prefix, 0, 0, 0),
    ("\u{AF}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{B0}", Form::Postfix, 0, 0, 0),
    ("\u{B1}", Form::Infix, 4, 4, 0),
    ("\u{B1}", Form::Prefix, 0, 0, 0),
    ("\u{B2}", Form::Postfix, 0, 0, 0),
    ("\u{B3}", Form::Postfix, 0, 0, 0),
    ("\u{B4}", Form::Postfix, 0, 0, 0),
    ("\u{B7}", Form::Infix, 3, 3, 0),
    ("\u{B8}", Form::Postfix, 0, 0, 0),
    ("\u{B9}", Form::Postfix, 0, 0, 0),
    ("\u{D7}", Form::Infix, 3, 3, 0),
    ("\u{F7}", Form::Infix, 4, 4, 0),
    ("\u{2C6}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2C7}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2C9}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2CA}", Form::Postfix, 0, 0, 0),
    ("\u{2CB}", Form::Postfix, 0, 0, 0),
    ("\u{2CD}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2D8}", Form::Postfix, 0, 0, 0),
    ("\u{2D9}", Form::Postfix, 0, 0, 0),
    ("\u{2DA}", Form::Postfix, 0, 0, 0),
    ("\u{2DC}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2DD}", Form::Postfix, 0, 0, 0),
    ("\u{2F7}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{302}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{311}", Form::Postfix, 0, 0, 0),
    ("\u{2016}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2016}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2018}", Form::Prefix, 0, 0, 0),
    ("\u{2019}", Form::Postfix, 0, 0, 0),
    ("\u{201A}", Form::Postfix, 0, 0, 0),
    ("\u{201B}", Form::Postfix, 0, 0, 0),
    ("\u{201C}", Form::Prefix, 0, 0, 0),
    ("\u{201D}", Form::Postfix, 0, 0, 0),
    ("\u{201E}", Form::Postfix, 0, 0, 0),
    ("\u{201F}", Form::Postfix, 0, 0, 0),
    ("\u{2022}", Form::Infix, 3, 3, 0),
    ("\u{2032}", Form::Postfix, 0, 0, 0),
    ("\u{2033}", Form::Postfix, 0, 0, 0),
    ("\u{2034}", Form::Postfix, 0, 0, 0),
    ("\u{2035}", Form::Postfix, 0, 0, 0),
    ("\u{2036}", Form::Postfix, 0, 0, 0),
    ("\u{2037}", Form::Postfix, 0, 0, 0),
    ("\u{203E}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2043}", Form::Infix, 3, 3, 0),
    ("\u{2044}", Form::Infix, 4, 4, 0),
    ("\u{2057}", Form::Postfix, 0, 0, 0),
    ("\u{2061}", Form::Infix, 0, 0, 0),
    ("\u{2062}", Form::Infix, 0, 0, 0),
    ("\u{2063}", Form::Infix, 0, 0, 0),
    ("\u{2064}", Form::Infix, 0, 0, 0),
    ("\u{20DB}", Form::Postfix, 0, 0, 0),
    ("\u{20DC}", Form::Postfix, 0, 0, 0),
    ("\u{2145}", Form::Prefix, 3, 0, 0),
    ("\u{2146}", Form::Prefix, 3, 0, 0),
    ("\u{2190}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2191}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2192}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2193}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2194}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2195}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2196}", Form::Infix, 5, 5, 0),
    ("\u{2197}", Form::Infix, 5, 5, 0),
    ("\u{2198}", Form::Infix, 5, 5, F::HORIZONTAL),
    ("\u{2199}", Form::Infix, 5, 5, F::HORIZONTAL),
    ("\u{219A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{219B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{219C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{219D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{219E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{219F}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21A0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21A1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21A2}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21A3}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21A4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21A5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21A6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21A7}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21A8}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21A9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21AF}", Form::Infix, 5, 5, 0),
    ("\u{21B0}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21B1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21B2}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21B3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21B4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21B5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21B6}", Form::Infix, 5, 5, 0),
    ("\u{21B7}", Form::Infix, 5, 5, 0),
    ("\u{21B8}", Form::Infix, 5, 5, 0),
    ("\u{21B9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21BA}", Form::Infix, 5, 5, 0),
    ("\u{21BB}", Form::Infix, 5, 5, 0),
    ("\u{21BC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21BD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21BE}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21BF}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21C0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21C1}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21C2}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21C3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21C4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21C5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21C6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21C7}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21C8}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21C9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21CA}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21CB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21CC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21CD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21CE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21CF}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21D0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21D1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21D2}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21D3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21D4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21D5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21D6}", Form::Infix, 5, 5, 0),
    ("\u{21D7}", Form::Infix, 5, 5, 0),
    ("\u{21D8}", Form::Infix, 5, 5, 0),
    ("\u{21D9}", Form::Infix, 5, 5, 0),
    ("\u{21DA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21DB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21DC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21DD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21DE}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21DF}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21E0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21E2}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21E4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E5}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E7}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21E8}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21E9}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21EA}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21EB}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21EC}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21ED}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21EE}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21EF}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21F0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21F1}", Form::Infix, 5, 5, 0),
    ("\u{21F2}", Form::Infix, 5, 5, 0),
    ("\u{21F3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21F4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21F5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{21F6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21F7}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21F8}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21F9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{21FF}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2200}", Form::Prefix, 0, 0, 0),
    ("\u{2201}", Form::Prefix, 0, 0, 0),
    ("\u{2202}", Form::Prefix, 3, 0, 0),
    ("\u{2203}", Form::Prefix, 0, 0, 0),
    ("\u{2204}", Form::Prefix, 0, 0, 0),
    ("\u{2206}", Form::Infix, 0, 0, 0),
    ("\u{2207}", Form::Prefix, 0, 0, 0),
    ("\u{2208}", Form::Infix, 5, 5, 0),
    ("\u{2209}", Form::Infix, 5, 5, 0),
    ("\u{220A}", Form::Infix, 5, 5, 0),
    ("\u{220B}", Form::Infix, 5, 5, 0),
    ("\u{220C}", Form::Infix, 5, 5, 0),
    ("\u{220D}", Form::Infix, 5, 5, 0),
    ("\u{220F}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2210}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2211}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2212}", Form::Infix, 4, 4, 0),
    ("\u{2212}", Form::Prefix, 0, 0, 0),
    ("\u{2213}", Form::Infix, 4, 4, 0),
    ("\u{2213}", Form::Prefix, 0, 0, 0),
    ("\u{2214}", Form::Infix, 4, 4, 0),
    ("\u{2215}", Form::Infix, 4, 4, 0),
    ("\u{2216}", Form::Infix, 4, 4, 0),
    ("\u{2217}", Form::Infix, 3, 3, 0),
    ("\u{2218}", Form::Infix, 3, 3, 0),
    ("\u{2219}", Form::Infix, 3, 3, 0),
    ("\u{221A}", Form::Prefix, 3, 0, 0),
    ("\u{221B}", Form::Prefix, 3, 0, 0),
    ("\u{221C}", Form::Prefix, 3, 0, 0),
    ("\u{221D}", Form::Infix, 5, 5, 0),
    ("\u{221F}", Form::Prefix, 0, 0, 0),
    ("\u{2220}", Form::Prefix, 0, 0, 0),
    ("\u{2221}", Form::Prefix, 0, 0, 0),
    ("\u{2222}", Form::Prefix, 0, 0, 0),
    ("\u{2223}", Form::Infix, 5, 5, 0),
    ("\u{2224}", Form::Infix, 5, 5, 0),
    ("\u{2225}", Form::Infix, 5, 5, 0),
    ("\u{2226}", Form::Infix, 5, 5, 0),
    ("\u{2227}", Form::Infix, 4, 4, 0),
    ("\u{2228}", Form::Infix, 4, 4, 0),
    ("\u{2229}", Form::Infix, 4, 4, 0),
    ("\u{222A}", Form::Infix, 4, 4, 0),
    ("\u{222B}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{222C}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{222D}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{222E}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{222F}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2230}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2231}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2232}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2233}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2234}", Form::Prefix, 0, 0, 0),
    ("\u{2235}", Form::Prefix, 0, 0, 0),
    ("\u{2236}", Form::Infix, 4, 4, 0),
    ("\u{2237}", Form::Infix, 5, 5, 0),
    ("\u{2238}", Form::Infix, 4, 4, 0),
    ("\u{2239}", Form::Infix, 5, 5, 0),
    ("\u{223A}", Form::Infix, 5, 5, 0),
    ("\u{223B}", Form::Infix, 5, 5, 0),
    ("\u{223C}", Form::Infix, 5, 5, 0),
    ("\u{223C}", Form::Prefix, 0, 0, 0),
    ("\u{223D}", Form::Infix, 5, 5, 0),
    ("\u{223E}", Form::Infix, 5, 5, 0),
    ("\u{2240}", Form::Infix, 3, 3, 0),
    ("\u{2241}", Form::Infix, 5, 5, 0),
    ("\u{2242}", Form::Infix, 5, 5, 0),
    ("\u{2243}", Form::Infix, 5, 5, 0),
    ("\u{2244}", Form::Infix, 5, 5, 0),
    ("\u{2245}", Form::Infix, 5, 5, 0),
    ("\u{2246}", Form::Infix, 5, 5, 0),
    ("\u{2247}", Form::Infix, 5, 5, 0),
    ("\u{2248}", Form::Infix, 5, 5, 0),
    ("\u{2249}", Form::Infix, 5, 5, 0),
    ("\u{224A}", Form::Infix, 5, 5, 0),
    ("\u{224B}", Form::Infix, 5, 5, 0),
    ("\u{224C}", Form::Infix, 5, 5, 0),
    ("\u{224D}", Form::Infix, 5, 5, 0),
    ("\u{224E}", Form::Infix, 5, 5, 0),
    ("\u{224F}", Form::Infix, 5, 5, 0),
    ("\u{2250}", Form::Infix, 5, 5, 0),
    ("\u{2251}", Form::Infix, 5, 5, 0),
    ("\u{2252}", Form::Infix, 5, 5, 0),
    ("\u{2253}", Form::Infix, 5, 5, 0),
    ("\u{2254}", Form::Infix, 5, 5, 0),
    ("\u{2255}", Form::Infix, 5, 5, 0),
    ("\u{2256}", Form::Infix, 5, 5, 0),
    ("\u{2257}", Form::Infix, 5, 5, 0),
    ("\u{2258}", Form::Infix, 5, 5, 0),
    ("\u{2259}", Form::Infix, 5, 5, 0),
    ("\u{225A}", Form::Infix, 5, 5, 0),
    ("\u{225B}", Form::Infix, 5, 5, 0),
    ("\u{225C}", Form::Infix, 5, 5, 0),
    ("\u{225D}", Form::Infix, 5, 5, 0),
    ("\u{225E}", Form::Infix, 5, 5, 0),
    ("\u{225F}", Form::Infix, 5, 5, 0),
    ("\u{2260}", Form::Infix, 5, 5, 0),
    ("\u{2261}", Form::Infix, 5, 5, 0),
    ("\u{2262}", Form::Infix, 5, 5, 0),
    ("\u{2263}", Form::Infix, 5, 5, 0),
    ("\u{2264}", Form::Infix, 5, 5, 0),
    ("\u{2265}", Form::Infix, 5, 5, 0),
    ("\u{2266}", Form::Infix, 5, 5, 0),
    ("\u{2267}", Form::Infix, 5, 5, 0),
    ("\u{2268}", Form::Infix, 5, 5, 0),
    ("\u{2269}", Form::Infix, 5, 5, 0),
    ("\u{226A}", Form::Infix, 5, 5, 0),
    ("\u{226B}", Form::Infix, 5, 5, 0),
    ("\u{226C}", Form::Infix, 5, 5, 0),
    ("\u{226D}", Form::Infix, 5, 5, 0),
    ("\u{226E}", Form::Infix, 5, 5, 0),
    ("\u{226F}", Form::Infix, 5, 5, 0),
    ("\u{2270}", Form::Infix, 5, 5, 0),
    ("\u{2271}", Form::Infix, 5, 5, 0),
    ("\u{2272}", Form::Infix, 5, 5, 0),
    ("\u{2273}", Form::Infix, 5, 5, 0),
    ("\u{2274}", Form::Infix, 5, 5, 0),
    ("\u{2275}", Form::Infix, 5, 5, 0),
    ("\u{2276}", Form::Infix, 5, 5, 0),
    ("\u{2277}", Form::Infix, 5, 5, 0),
    ("\u{2278}", Form::Infix, 5, 5, 0),
    ("\u{2279}", Form::Infix, 5, 5, 0),
    ("\u{227A}", Form::Infix, 5, 5, 0),
    ("\u{227B}", Form::Infix, 5, 5, 0),
    ("\u{227C}", Form::Infix, 5, 5, 0),
    ("\u{227D}", Form::Infix, 5, 5, 0),
    ("\u{227E}", Form::Infix, 5, 5, 0),
    ("\u{227F}", Form::Infix, 5, 5, 0),
    ("\u{2280}", Form::Infix, 5, 5, 0),
    ("\u{2281}", Form::Infix, 5, 5, 0),
    ("\u{2282}", Form::Infix, 5, 5, 0),
    ("\u{2283}", Form::Infix, 5, 5, 0),
    ("\u{2284}", Form::Infix, 5, 5, 0),
    ("\u{2285}", Form::Infix, 5, 5, 0),
    ("\u{2286}", Form::Infix, 5, 5, 0),
    ("\u{2287}", Form::Infix, 5, 5, 0),
    ("\u{2288}", Form::Infix, 5, 5, 0),
    ("\u{2289}", Form::Infix, 5, 5, 0),
    ("\u{228A}", Form::Infix, 5, 5, 0),
    ("\u{228B}", Form::Infix, 5, 5, 0),
    ("\u{228C}", Form::Infix, 4, 4, 0),
    ("\u{228D}", Form::Infix, 4, 4, 0),
    ("\u{228E}", Form::Infix, 4, 4, 0),
    ("\u{228F}", Form::Infix, 5, 5, 0),
    ("\u{2290}", Form::Infix, 5, 5, 0),
    ("\u{2291}", Form::Infix, 5, 5, 0),
    ("\u{2292}", Form::Infix, 5, 5, 0),
    ("\u{2293}", Form::Infix, 4, 4, 0),
    ("\u{2294}", Form::Infix, 4, 4, 0),
    ("\u{2295}", Form::Infix, 4, 4, 0),
    ("\u{2296}", Form::Infix, 4, 4, 0),
    ("\u{2297}", Form::Infix, 3, 3, 0),
    ("\u{2298}", Form::Infix, 4, 4, 0),
    ("\u{2299}", Form::Infix, 3, 3, 0),
    ("\u{229A}", Form::Infix, 3, 3, 0),
    ("\u{229B}", Form::Infix, 3, 3, 0),
    ("\u{229C}", Form::Infix, 5, 5, 0),
    ("\u{229D}", Form::Infix, 4, 4, 0),
    ("\u{229E}", Form::Infix, 4, 4, 0),
    ("\u{229F}", Form::Infix, 4, 4, 0),
    ("\u{22A0}", Form::Infix, 3, 3, 0),
    ("\u{22A1}", Form::Infix, 3, 3, 0),
    ("\u{22A2}", Form::Infix, 5, 5, 0),
    ("\u{22A3}", Form::Infix, 5, 5, 0),
    ("\u{22A6}", Form::Infix, 5, 5, 0),
    ("\u{22A7}", Form::Infix, 5, 5, 0),
    ("\u{22A8}", Form::Infix, 5, 5, 0),
    ("\u{22A9}", Form::Infix, 5, 5, 0),
    ("\u{22AA}", Form::Infix, 5, 5, 0),
    ("\u{22AB}", Form::Infix, 5, 5, 0),
    ("\u{22AC}", Form::Infix, 5, 5, 0),
    ("\u{22AD}", Form::Infix, 5, 5, 0),
    ("\u{22AE}", Form::Infix, 5, 5, 0),
    ("\u{22AF}", Form::Infix, 5, 5, 0),
    ("\u{22B0}", Form::Infix, 5, 5, 0),
    ("\u{22B1}", Form::Infix, 5, 5, 0),
    ("\u{22B2}", Form::Infix, 5, 5, 0),
    ("\u{22B3}", Form::Infix, 5, 5, 0),
    ("\u{22B4}", Form::Infix, 5, 5, 0),
    ("\u{22B5}", Form::Infix, 5, 5, 0),
    ("\u{22B6}", Form::Infix, 5, 5, 0),
    ("\u{22B7}", Form::Infix, 5, 5, 0),
    ("\u{22B8}", Form::Infix, 5, 5, 0),
    ("\u{22BA}", Form::Infix, 3, 3, 0),
    ("\u{22BB}", Form::Infix, 4, 4, 0),
    ("\u{22BC}", Form::Infix, 4, 4, 0),
    ("\u{22BD}", Form::Infix, 4, 4, 0),
    ("\u{22BE}", Form::Prefix, 0, 0, 0),
    ("\u{22BF}", Form::Prefix, 0, 0, 0),
    ("\u{22C0}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{22C1}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{22C2}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{22C3}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{22C4}", Form::Infix, 3, 3, 0),
    ("\u{22C5}", Form::Infix, 3, 3, 0),
    ("\u{22C6}", Form::Infix, 3, 3, 0),
    ("\u{22C7}", Form::Infix, 3, 3, 0),
    ("\u{22C8}", Form::Infix, 5, 5, 0),
    ("\u{22C9}", Form::Infix, 3, 3, 0),
    ("\u{22CA}", Form::Infix, 3, 3, 0),
    ("\u{22CB}", Form::Infix, 3, 3, 0),
    ("\u{22CC}", Form::Infix, 3, 3, 0),
    ("\u{22CD}", Form::Infix, 5, 5, 0),
    ("\u{22CE}", Form::Infix, 4, 4, 0),
    ("\u{22CF}", Form::Infix, 4, 4, 0),
    ("\u{22D0}", Form::Infix, 5, 5, 0),
    ("\u{22D1}", Form::Infix, 5, 5, 0),
    ("\u{22D2}", Form::Infix, 4, 4, 0),
    ("\u{22D3}", Form::Infix, 4, 4, 0),
    ("\u{22D4}", Form::Infix, 5, 5, 0),
    ("\u{22D5}", Form::Infix, 5, 5, 0),
    ("\u{22D6}", Form::Infix, 5, 5, 0),
    ("\u{22D7}", Form::Infix, 5, 5, 0),
    ("\u{22D8}", Form::Infix, 5, 5, 0),
    ("\u{22D9}", Form::Infix, 5, 5, 0),
    ("\u{22DA}", Form::Infix, 5, 5, 0),
    ("\u{22DB}", Form::Infix, 5, 5, 0),
    ("\u{22DC}", Form::Infix, 5, 5, 0),
    ("\u{22DD}", Form::Infix, 5, 5, 0),
    ("\u{22DE}", Form::Infix, 5, 5, 0),
    ("\u{22DF}", Form::Infix, 5, 5, 0),
    ("\u{22E0}", Form::Infix, 5, 5, 0),
    ("\u{22E1}", Form::Infix, 5, 5, 0),
    ("\u{22E2}", Form::Infix, 5, 5, 0),
    ("\u{22E3}", Form::Infix, 5, 5, 0),
    ("\u{22E4}", Form::Infix, 5, 5, 0),
    ("\u{22E5}", Form::Infix, 5, 5, 0),
    ("\u{22E6}", Form::Infix, 5, 5, 0),
    ("\u{22E7}", Form::Infix, 5, 5, 0),
    ("\u{22E8}", Form::Infix, 5, 5, 0),
    ("\u{22E9}", Form::Infix, 5, 5, 0),
    ("\u{22EA}", Form::Infix, 5, 5, 0),
    ("\u{22EB}", Form::Infix, 5, 5, 0),
    ("\u{22EC}", Form::Infix, 5, 5, 0),
    ("\u{22ED}", Form::Infix, 5, 5, 0),
    ("\u{22F2}", Form::Infix, 5, 5, 0),
    ("\u{22F3}", Form::Infix, 5, 5, 0),
    ("\u{22F4}", Form::Infix, 5, 5, 0),
    ("\u{22F5}", Form::Infix, 5, 5, 0),
    ("\u{22F6}", Form::Infix, 5, 5, 0),
    ("\u{22F7}", Form::Infix, 5, 5, 0),
    ("\u{22F8}", Form::Infix, 5, 5, 0),
    ("\u{22F9}", Form::Infix, 5, 5, 0),
    ("\u{22FA}", Form::Infix, 5, 5, 0),
    ("\u{22FB}", Form::Infix, 5, 5, 0),
    ("\u{22FC}", Form::Infix, 5, 5, 0),
    ("\u{22FD}", Form::Infix, 5, 5, 0),
    ("\u{22FE}", Form::Infix, 5, 5, 0),
    ("\u{22FF}", Form::Infix, 5, 5, 0),
    ("\u{2301}", Form::Infix, 5, 5, 0),
    ("\u{2305}", Form::Infix, 3, 3, 0),
    ("\u{2306}", Form::Infix, 3, 3, 0),
    ("\u{2308}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2309}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{230A}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{230B}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2310}", Form::Prefix, 0, 0, 0),
    ("\u{2319}", Form::Prefix, 0, 0, 0),
    ("\u{2322}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2323}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2329}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{232A}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{237C}", Form::Infix, 5, 5, 0),
    ("\u{238B}", Form::Infix, 5, 5, 0),
    ("\u{23B4}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23B5}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23CD}", Form::Postfix, 0, 0, 0),
    ("\u{23DC}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23DD}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23DE}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23DF}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23E0}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{23E1}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{2772}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2773}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2794}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2795}", Form::Infix, 4, 4, 0),
    ("\u{2795}", Form::Prefix, 0, 0, 0),
    ("\u{2796}", Form::Infix, 4, 4, 0),
    ("\u{2796}", Form::Prefix, 0, 0, 0),
    ("\u{2797}", Form::Infix, 4, 4, 0),
    ("\u{2798}", Form::Infix, 5, 5, 0),
    ("\u{2799}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{279A}", Form::Infix, 5, 5, 0),
    ("\u{279B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{279C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{279D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{279E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{279F}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A0}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A1}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A5}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A7}", Form::Infix, 5, 5, 0),
    ("\u{27A8}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27A9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27AF}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27B1}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27B2}", Form::Infix, 5, 5, 0),
    ("\u{27B3}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27B4}", Form::Infix, 5, 5, 0),
    ("\u{27B5}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27B6}", Form::Infix, 5, 5, 0),
    ("\u{27B7}", Form::Infix, 5, 5, 0),
    ("\u{27B8}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27B9}", Form::Infix, 5, 5, 0),
    ("\u{27BA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27BB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27BC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27BD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27BE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27C0}", Form::Prefix, 0, 0, 0),
    ("\u{27C2}", Form::Infix, 5, 5, 0),
    ("\u{27CB}", Form::Infix, 3, 3, 0),
    ("\u{27CD}", Form::Infix, 3, 3, 0),
    ("\u{27E6}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27E7}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27E8}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27E9}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27EA}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27EB}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27EC}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27ED}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27EE}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27EF}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{27F0}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{27F1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{27F2}", Form::Infix, 5, 5, 0),
    ("\u{27F3}", Form::Infix, 5, 5, 0),
    ("\u{27F4}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27F5}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27F6}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27F7}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27F8}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27F9}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FA}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FB}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FC}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FD}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FE}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{27FF}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2900}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2901}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2902}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2903}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2904}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2905}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2906}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2907}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2908}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2909}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{290A}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{290B}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{290C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{290D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{290E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{290F}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2910}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2911}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2912}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2913}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2914}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2915}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2916}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2917}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2918}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2919}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{291F}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2920}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2921}", Form::Infix, 5, 5, 0),
    ("\u{2922}", Form::Infix, 5, 5, 0),
    ("\u{2923}", Form::Infix, 5, 5, 0),
    ("\u{2924}", Form::Infix, 5, 5, 0),
    ("\u{2925}", Form::Infix, 5, 5, 0),
    ("\u{2926}", Form::Infix, 5, 5, 0),
    ("\u{2927}", Form::Infix, 5, 5, 0),
    ("\u{2928}", Form::Infix, 5, 5, 0),
    ("\u{2929}", Form::Infix, 5, 5, 0),
    ("\u{292A}", Form::Infix, 5, 5, 0),
    ("\u{292B}", Form::Infix, 5, 5, 0),
    ("\u{292C}", Form::Infix, 5, 5, 0),
    ("\u{292D}", Form::Infix, 5, 5, 0),
    ("\u{292E}", Form::Infix, 5, 5, 0),
    ("\u{292F}", Form::Infix, 5, 5, 0),
    ("\u{2930}", Form::Infix, 5, 5, 0),
    ("\u{2931}", Form::Infix, 5, 5, 0),
    ("\u{2932}", Form::Infix, 5, 5, 0),
    ("\u{2933}", Form::Infix, 5, 5, 0),
    ("\u{2934}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2935}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2936}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2937}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2938}", Form::Infix, 5, 5, 0),
    ("\u{2939}", Form::Infix, 5, 5, 0),
    ("\u{293A}", Form::Infix, 5, 5, 0),
    ("\u{293B}", Form::Infix, 5, 5, 0),
    ("\u{293C}", Form::Infix, 5, 5, 0),
    ("\u{293D}", Form::Infix, 5, 5, 0),
    ("\u{293E}", Form::Infix, 5, 5, 0),
    ("\u{293F}", Form::Infix, 5, 5, 0),
    ("\u{2940}", Form::Infix, 5, 5, 0),
    ("\u{2941}", Form::Infix, 5, 5, 0),
    ("\u{2942}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2943}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2944}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2945}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2946}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2947}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2948}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2949}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{294A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{294B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{294C}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{294D}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{294E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{294F}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2950}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2951}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2952}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2953}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2954}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2955}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2956}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2957}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2958}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2959}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{295A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{295B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{295C}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{295D}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{295E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{295F}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2960}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2961}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2962}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2963}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2964}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2965}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2966}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2967}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2968}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2969}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{296A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{296B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{296C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{296D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{296E}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{296F}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2970}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2971}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2972}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2973}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2974}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2975}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2976}", Form::Infix, 5, 5, 0),
    ("\u{2977}", Form::Infix, 5, 5, 0),
    ("\u{2978}", Form::Infix, 5, 5, 0),
    ("\u{2979}", Form::Infix, 5, 5, 0),
    ("\u{297A}", Form::Infix, 5, 5, 0),
    ("\u{297B}", Form::Infix, 5, 5, 0),
    ("\u{297C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{297D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{297E}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{297F}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2980}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2980}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2981}", Form::Infix, 5, 5, 0),
    ("\u{2982}", Form::Infix, 5, 5, 0),
    ("\u{2983}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2984}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2985}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2986}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2987}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2988}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2989}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298A}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298B}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298C}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298D}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298E}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{298F}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2990}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2991}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2992}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2993}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2994}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2995}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2996}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2997}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2998}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2999}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2999}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{299B}", Form::Prefix, 0, 0, 0),
    ("\u{299C}", Form::Prefix, 0, 0, 0),
    ("\u{299D}", Form::Prefix, 0, 0, 0),
    ("\u{299E}", Form::Prefix, 0, 0, 0),
    ("\u{299F}", Form::Prefix, 0, 0, 0),
    ("\u{29A0}", Form::Prefix, 0, 0, 0),
    ("\u{29A1}", Form::Prefix, 0, 0, 0),
    ("\u{29A2}", Form::Prefix, 0, 0, 0),
    ("\u{29A3}", Form::Prefix, 0, 0, 0),
    ("\u{29A4}", Form::Prefix, 0, 0, 0),
    ("\u{29A5}", Form::Prefix, 0, 0, 0),
    ("\u{29A6}", Form::Prefix, 0, 0, 0),
    ("\u{29A7}", Form::Prefix, 0, 0, 0),
    ("\u{29A8}", Form::Prefix, 0, 0, 0),
    ("\u{29A9}", Form::Prefix, 0, 0, 0),
    ("\u{29AA}", Form::Prefix, 0, 0, 0),
    ("\u{29AB}", Form::Prefix, 0, 0, 0),
    ("\u{29AC}", Form::Prefix, 0, 0, 0),
    ("\u{29AD}", Form::Prefix, 0, 0, 0),
    ("\u{29AE}", Form::Prefix, 0, 0, 0),
    ("\u{29AF}", Form::Prefix, 0, 0, 0),
    ("\u{29B6}", Form::Infix, 5, 5, 0),
    ("\u{29B7}", Form::Infix, 5, 5, 0),
    ("\u{29B8}", Form::Infix, 4, 4, 0),
    ("\u{29B9}", Form::Infix, 5, 5, 0),
    ("\u{29BC}", Form::Infix, 4, 4, 0),
    ("\u{29C0}", Form::Infix, 5, 5, 0),
    ("\u{29C1}", Form::Infix, 5, 5, 0),
    ("\u{29C4}", Form::Infix, 4, 4, 0),
    ("\u{29C5}", Form::Infix, 4, 4, 0),
    ("\u{29C6}", Form::Infix, 3, 3, 0),
    ("\u{29C7}", Form::Infix, 3, 3, 0),
    ("\u{29C8}", Form::Infix, 3, 3, 0),
    ("\u{29CE}", Form::Infix, 5, 5, 0),
    ("\u{29CF}", Form::Infix, 5, 5, 0),
    ("\u{29D0}", Form::Infix, 5, 5, 0),
    ("\u{29D1}", Form::Infix, 5, 5, 0),
    ("\u{29D2}", Form::Infix, 5, 5, 0),
    ("\u{29D3}", Form::Infix, 5, 5, 0),
    ("\u{29D4}", Form::Infix, 3, 3, 0),
    ("\u{29D5}", Form::Infix, 3, 3, 0),
    ("\u{29D6}", Form::Infix, 3, 3, 0),
    ("\u{29D7}", Form::Infix, 3, 3, 0),
    ("\u{29D8}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{29D9}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{29DA}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{29DB}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{29DF}", Form::Infix, 5, 5, 0),
    ("\u{29E1}", Form::Infix, 5, 5, 0),
    ("\u{29E2}", Form::Infix, 3, 3, 0),
    ("\u{29E3}", Form::Infix, 5, 5, 0),
    ("\u{29E4}", Form::Infix, 5, 5, 0),
    ("\u{29E5}", Form::Infix, 5, 5, 0),
    ("\u{29E6}", Form::Infix, 5, 5, 0),
    ("\u{29F4}", Form::Infix, 5, 5, 0),
    ("\u{29F5}", Form::Infix, 4, 4, 0),
    ("\u{29F6}", Form::Infix, 4, 4, 0),
    ("\u{29F7}", Form::Infix, 4, 4, 0),
    ("\u{29F8}", Form::Infix, 4, 4, 0),
    ("\u{29F9}", Form::Infix, 4, 4, 0),
    ("\u{29FA}", Form::Infix, 4, 4, 0),
    ("\u{29FB}", Form::Infix, 4, 4, 0),
    ("\u{29FC}", Form::Prefix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{29FD}", Form::Postfix, 0, 0, F::STRETCHY | F::SYMMETRIC),
    ("\u{2A00}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A01}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A02}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A03}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A04}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A05}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A06}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A07}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A08}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A09}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A0A}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A0B}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A0C}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A0D}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A0E}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A0F}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A10}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A11}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A12}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A13}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A14}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A15}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A16}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A17}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A18}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A19}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A1A}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A1B}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A1C}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP),
    ("\u{2A1D}", Form::Infix, 3, 3, 0),
    ("\u{2A1D}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A1E}", Form::Infix, 3, 3, 0),
    ("\u{2A1E}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2A1F}", Form::Infix, 4, 4, 0),
    ("\u{2A20}", Form::Infix, 4, 4, 0),
    ("\u{2A21}", Form::Infix, 4, 4, 0),
    ("\u{2A22}", Form::Infix, 4, 4, 0),
    ("\u{2A23}", Form::Infix, 4, 4, 0),
    ("\u{2A24}", Form::Infix, 4, 4, 0),
    ("\u{2A25}", Form::Infix, 4, 4, 0),
    ("\u{2A26}", Form::Infix, 4, 4, 0),
    ("\u{2A27}", Form::Infix, 4, 4, 0),
    ("\u{2A28}", Form::Infix, 4, 4, 0),
    ("\u{2A29}", Form::Infix, 4, 4, 0),
    ("\u{2A2A}", Form::Infix, 4, 4, 0),
    ("\u{2A2B}", Form::Infix, 4, 4, 0),
    ("\u{2A2C}", Form::Infix, 4, 4, 0),
    ("\u{2A2D}", Form::Infix, 4, 4, 0),
    ("\u{2A2E}", Form::Infix, 4, 4, 0),
    ("\u{2A2F}", Form::Infix, 3, 3, 0),
    ("\u{2A30}", Form::Infix, 3, 3, 0),
    ("\u{2A31}", Form::Infix, 3, 3, 0),
    ("\u{2A32}", Form::Infix, 3, 3, 0),
    ("\u{2A33}", Form::Infix, 3, 3, 0),
    ("\u{2A34}", Form::Infix, 3, 3, 0),
    ("\u{2A35}", Form::Infix, 3, 3, 0),
    ("\u{2A36}", Form::Infix, 3, 3, 0),
    ("\u{2A37}", Form::Infix, 3, 3, 0),
    ("\u{2A38}", Form::Infix, 4, 4, 0),
    ("\u{2A39}", Form::Infix, 4, 4, 0),
    ("\u{2A3A}", Form::Infix, 4, 4, 0),
    ("\u{2A3B}", Form::Infix, 3, 3, 0),
    ("\u{2A3C}", Form::Infix, 3, 3, 0),
    ("\u{2A3D}", Form::Infix, 3, 3, 0),
    ("\u{2A3E}", Form::Infix, 4, 4, 0),
    ("\u{2A3F}", Form::Infix, 3, 3, 0),
    ("\u{2A40}", Form::Infix, 4, 4, 0),
    ("\u{2A41}", Form::Infix, 4, 4, 0),
    ("\u{2A42}", Form::Infix, 4, 4, 0),
    ("\u{2A43}", Form::Infix, 4, 4, 0),
    ("\u{2A44}", Form::Infix, 4, 4, 0),
    ("\u{2A45}", Form::Infix, 4, 4, 0),
    ("\u{2A46}", Form::Infix, 4, 4, 0),
    ("\u{2A47}", Form::Infix, 4, 4, 0),
    ("\u{2A48}", Form::Infix, 4, 4, 0),
    ("\u{2A49}", Form::Infix, 4, 4, 0),
    ("\u{2A4A}", Form::Infix, 4, 4, 0),
    ("\u{2A4B}", Form::Infix, 4, 4, 0),
    ("\u{2A4C}", Form::Infix, 4, 4, 0),
    ("\u{2A4D}", Form::Infix, 4, 4, 0),
    ("\u{2A4E}", Form::Infix, 4, 4, 0),
    ("\u{2A4F}", Form::Infix, 4, 4, 0),
    ("\u{2A50}", Form::Infix, 3, 3, 0),
    ("\u{2A51}", Form::Infix, 4, 4, 0),
    ("\u{2A52}", Form::Infix, 4, 4, 0),
    ("\u{2A53}", Form::Infix, 4, 4, 0),
    ("\u{2A54}", Form::Infix, 4, 4, 0),
    ("\u{2A55}", Form::Infix, 4, 4, 0),
    ("\u{2A56}", Form::Infix, 4, 4, 0),
    ("\u{2A57}", Form::Infix, 4, 4, 0),
    ("\u{2A58}", Form::Infix, 4, 4, 0),
    ("\u{2A59}", Form::Infix, 4, 4, 0),
    ("\u{2A5A}", Form::Infix, 4, 4, 0),
    ("\u{2A5B}", Form::Infix, 4, 4, 0),
    ("\u{2A5C}", Form::Infix, 4, 4, 0),
    ("\u{2A5D}", Form::Infix, 4, 4, 0),
    ("\u{2A5E}", Form::Infix, 4, 4, 0),
    ("\u{2A5F}", Form::Infix, 4, 4, 0),
    ("\u{2A60}", Form::Infix, 4, 4, 0),
    ("\u{2A61}", Form::Infix, 4, 4, 0),
    ("\u{2A62}", Form::Infix, 4, 4, 0),
    ("\u{2A63}", Form::Infix, 4, 4, 0),
    ("\u{2A64}", Form::Infix, 3, 3, 0),
    ("\u{2A65}", Form::Infix, 3, 3, 0),
    ("\u{2A66}", Form::Infix, 5, 5, 0),
    ("\u{2A67}", Form::Infix, 5, 5, 0),
    ("\u{2A68}", Form::Infix, 5, 5, 0),
    ("\u{2A69}", Form::Infix, 5, 5, 0),
    ("\u{2A6A}", Form::Infix, 5, 5, 0),
    ("\u{2A6B}", Form::Infix, 5, 5, 0),
    ("\u{2A6C}", Form::Infix, 5, 5, 0),
    ("\u{2A6D}", Form::Infix, 5, 5, 0),
    ("\u{2A6E}", Form::Infix, 5, 5, 0),
    ("\u{2A6F}", Form::Infix, 5, 5, 0),
    ("\u{2A70}", Form::Infix, 5, 5, 0),
    ("\u{2A71}", Form::Infix, 5, 5, 0),
    ("\u{2A72}", Form::Infix, 5, 5, 0),
    ("\u{2A73}", Form::Infix, 5, 5, 0),
    ("\u{2A74}", Form::Infix, 5, 5, 0),
    ("\u{2A75}", Form::Infix, 5, 5, 0),
    ("\u{2A76}", Form::Infix, 5, 5, 0),
    ("\u{2A77}", Form::Infix, 5, 5, 0),
    ("\u{2A78}", Form::Infix, 5, 5, 0),
    ("\u{2A79}", Form::Infix, 5, 5, 0),
    ("\u{2A7A}", Form::Infix, 5, 5, 0),
    ("\u{2A7B}", Form::Infix, 5, 5, 0),
    ("\u{2A7C}", Form::Infix, 5, 5, 0),
    ("\u{2A7D}", Form::Infix, 5, 5, 0),
    ("\u{2A7E}", Form::Infix, 5, 5, 0),
    ("\u{2A7F}", Form::Infix, 5, 5, 0),
    ("\u{2A80}", Form::Infix, 5, 5, 0),
    ("\u{2A81}", Form::Infix, 5, 5, 0),
    ("\u{2A82}", Form::Infix, 5, 5, 0),
    ("\u{2A83}", Form::Infix, 5, 5, 0),
    ("\u{2A84}", Form::Infix, 5, 5, 0),
    ("\u{2A85}", Form::Infix, 5, 5, 0),
    ("\u{2A86}", Form::Infix, 5, 5, 0),
    ("\u{2A87}", Form::Infix, 5, 5, 0),
    ("\u{2A88}", Form::Infix, 5, 5, 0),
    ("\u{2A89}", Form::Infix, 5, 5, 0),
    ("\u{2A8A}", Form::Infix, 5, 5, 0),
    ("\u{2A8B}", Form::Infix, 5, 5, 0),
    ("\u{2A8C}", Form::Infix, 5, 5, 0),
    ("\u{2A8D}", Form::Infix, 5, 5, 0),
    ("\u{2A8E}", Form::Infix, 5, 5, 0),
    ("\u{2A8F}", Form::Infix, 5, 5, 0),
    ("\u{2A90}", Form::Infix, 5, 5, 0),
    ("\u{2A91}", Form::Infix, 5, 5, 0),
    ("\u{2A92}", Form::Infix, 5, 5, 0),
    ("\u{2A93}", Form::Infix, 5, 5, 0),
    ("\u{2A94}", Form::Infix, 5, 5, 0),
    ("\u{2A95}", Form::Infix, 5, 5, 0),
    ("\u{2A96}", Form::Infix, 5, 5, 0),
    ("\u{2A97}", Form::Infix, 5, 5, 0),
    ("\u{2A98}", Form::Infix, 5, 5, 0),
    ("\u{2A99}", Form::Infix, 5, 5, 0),
    ("\u{2A9A}", Form::Infix, 5, 5, 0),
    ("\u{2A9B}", Form::Infix, 5, 5, 0),
    ("\u{2A9C}", Form::Infix, 5, 5, 0),
    ("\u{2A9D}", Form::Infix, 5, 5, 0),
    ("\u{2A9E}", Form::Infix, 5, 5, 0),
    ("\u{2A9F}", Form::Infix, 5, 5, 0),
    ("\u{2AA0}", Form::Infix, 5, 5, 0),
    ("\u{2AA1}", Form::Infix, 5, 5, 0),
    ("\u{2AA2}", Form::Infix, 5, 5, 0),
    ("\u{2AA3}", Form::Infix, 5, 5, 0),
    ("\u{2AA4}", Form::Infix, 5, 5, 0),
    ("\u{2AA5}", Form::Infix, 5, 5, 0),
    ("\u{2AA6}", Form::Infix, 5, 5, 0),
    ("\u{2AA7}", Form::Infix, 5, 5, 0),
    ("\u{2AA8}", Form::Infix, 5, 5, 0),
    ("\u{2AA9}", Form::Infix, 5, 5, 0),
    ("\u{2AAA}", Form::Infix, 5, 5, 0),
    ("\u{2AAB}", Form::Infix, 5, 5, 0),
    ("\u{2AAC}", Form::Infix, 5, 5, 0),
    ("\u{2AAD}", Form::Infix, 5, 5, 0),
    ("\u{2AAE}", Form::Infix, 5, 5, 0),
    ("\u{2AAF}", Form::Infix, 5, 5, 0),
    ("\u{2AB0}", Form::Infix, 5, 5, 0),
    ("\u{2AB1}", Form::Infix, 5, 5, 0),
    ("\u{2AB2}", Form::Infix, 5, 5, 0),
    ("\u{2AB3}", Form::Infix, 5, 5, 0),
    ("\u{2AB4}", Form::Infix, 5, 5, 0),
    ("\u{2AB5}", Form::Infix, 5, 5, 0),
    ("\u{2AB6}", Form::Infix, 5, 5, 0),
    ("\u{2AB7}", Form::Infix, 5, 5, 0),
    ("\u{2AB8}", Form::Infix, 5, 5, 0),
    ("\u{2AB9}", Form::Infix, 5, 5, 0),
    ("\u{2ABA}", Form::Infix, 5, 5, 0),
    ("\u{2ABB}", Form::Infix, 5, 5, 0),
    ("\u{2ABC}", Form::Infix, 5, 5, 0),
    ("\u{2ABD}", Form::Infix, 5, 5, 0),
    ("\u{2ABE}", Form::Infix, 5, 5, 0),
    ("\u{2ABF}", Form::Infix, 5, 5, 0),
    ("\u{2AC0}", Form::Infix, 5, 5, 0),
    ("\u{2AC1}", Form::Infix, 5, 5, 0),
    ("\u{2AC2}", Form::Infix, 5, 5, 0),
    ("\u{2AC3}", Form::Infix, 5, 5, 0),
    ("\u{2AC4}", Form::Infix, 5, 5, 0),
    ("\u{2AC5}", Form::Infix, 5, 5, 0),
    ("\u{2AC6}", Form::Infix, 5, 5, 0),
    ("\u{2AC7}", Form::Infix, 5, 5, 0),
    ("\u{2AC8}", Form::Infix, 5, 5, 0),
    ("\u{2AC9}", Form::Infix, 5, 5, 0),
    ("\u{2ACA}", Form::Infix, 5, 5, 0),
    ("\u{2ACB}", Form::Infix, 5, 5, 0),
    ("\u{2ACC}", Form::Infix, 5, 5, 0),
    ("\u{2ACD}", Form::Infix, 5, 5, 0),
    ("\u{2ACE}", Form::Infix, 5, 5, 0),
    ("\u{2ACF}", Form::Infix, 5, 5, 0),
    ("\u{2AD0}", Form::Infix, 5, 5, 0),
    ("\u{2AD1}", Form::Infix, 5, 5, 0),
    ("\u{2AD2}", Form::Infix, 5, 5, 0),
    ("\u{2AD3}", Form::Infix, 5, 5, 0),
    ("\u{2AD4}", Form::Infix, 5, 5, 0),
    ("\u{2AD5}", Form::Infix, 5, 5, 0),
    ("\u{2AD6}", Form::Infix, 5, 5, 0),
    ("\u{2AD7}", Form::Infix, 5, 5, 0),
    ("\u{2AD8}", Form::Infix, 5, 5, 0),
    ("\u{2AD9}", Form::Infix, 5, 5, 0),
    ("\u{2ADA}", Form::Infix, 5, 5, 0),
    ("\u{2ADB}", Form::Infix, 4, 4, 0),
    ("\u{2ADC}", Form::Infix, 3, 3, 0),
    ("\u{2ADD}", Form::Infix, 3, 3, 0),
    ("\u{2ADE}", Form::Infix, 5, 5, 0),
    ("\u{2ADF}", Form::Infix, 5, 5, 0),
    ("\u{2AE0}", Form::Infix, 5, 5, 0),
    ("\u{2AE1}", Form::Infix, 5, 5, 0),
    ("\u{2AE2}", Form::Infix, 5, 5, 0),
    ("\u{2AE3}", Form::Infix, 5, 5, 0),
    ("\u{2AE4}", Form::Infix, 5, 5, 0),
    ("\u{2AE5}", Form::Infix, 5, 5, 0),
    ("\u{2AE6}", Form::Infix, 5, 5, 0),
    ("\u{2AE7}", Form::Infix, 5, 5, 0),
    ("\u{2AE8}", Form::Infix, 5, 5, 0),
    ("\u{2AE9}", Form::Infix, 5, 5, 0),
    ("\u{2AEA}", Form::Infix, 5, 5, 0),
    ("\u{2AEB}", Form::Infix, 5, 5, 0),
    ("\u{2AEC}", Form::Prefix, 0, 0, 0),
    ("\u{2AED}", Form::Prefix, 0, 0, 0),
    ("\u{2AEE}", Form::Infix, 5, 5, 0),
    ("\u{2AF2}", Form::Infix, 5, 5, 0),
    ("\u{2AF3}", Form::Infix, 5, 5, 0),
    ("\u{2AF4}", Form::Infix, 5, 5, 0),
    ("\u{2AF5}", Form::Infix, 5, 5, 0),
    ("\u{2AF6}", Form::Infix, 4, 4, 0),
    ("\u{2AF7}", Form::Infix, 5, 5, 0),
    ("\u{2AF8}", Form::Infix, 5, 5, 0),
    ("\u{2AF9}", Form::Infix, 5, 5, 0),
    ("\u{2AFA}", Form::Infix, 5, 5, 0),
    ("\u{2AFB}", Form::Infix, 4, 4, 0),
    ("\u{2AFC}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2AFD}", Form::Infix, 4, 4, 0),
    ("\u{2AFE}", Form::Infix, 3, 3, 0),
    ("\u{2AFF}", Form::Prefix, 3, 3, F::SYMMETRIC | F::LARGEOP | F::MOVABLELIMITS),
    ("\u{2B00}", Form::Infix, 5, 5, 0),
    ("\u{2B01}", Form::Infix, 5, 5, 0),
    ("\u{2B02}", Form::Infix, 5, 5, 0),
    ("\u{2B03}", Form::Infix, 5, 5, 0),
    ("\u{2B04}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B05}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B06}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B07}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B08}", Form::Infix, 5, 5, 0),
    ("\u{2B09}", Form::Infix, 5, 5, 0),
    ("\u{2B0A}", Form::Infix, 5, 5, 0),
    ("\u{2B0B}", Form::Infix, 5, 5, 0),
    ("\u{2B0C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B0D}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B0E}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B0F}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B10}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B11}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B30}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B31}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B32}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B33}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B34}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B35}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B36}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B37}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B38}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B39}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3D}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3E}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B3F}", Form::Infix, 5, 5, 0),
    ("\u{2B40}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B41}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B42}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B43}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B44}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B45}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B46}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B47}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B48}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B49}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B4A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B4B}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B4C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B4D}", Form::Infix, 5, 5, 0),
    ("\u{2B4E}", Form::Infix, 5, 5, 0),
    ("\u{2B4F}", Form::Infix, 5, 5, 0),
    ("\u{2B5A}", Form::Infix, 5, 5, 0),
    ("\u{2B5B}", Form::Infix, 5, 5, 0),
    ("\u{2B5C}", Form::Infix, 5, 5, 0),
    ("\u{2B5D}", Form::Infix, 5, 5, 0),
    ("\u{2B5E}", Form::Infix, 5, 5, 0),
    ("\u{2B5F}", Form::Infix, 5, 5, 0),
    ("\u{2B60}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B61}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B62}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B63}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B64}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B65}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B66}", Form::Infix, 5, 5, 0),
    ("\u{2B67}", Form::Infix, 5, 5, 0),
    ("\u{2B68}", Form::Infix, 5, 5, 0),
    ("\u{2B69}", Form::Infix, 5, 5, 0),
    ("\u{2B6A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B6B}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B6C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B6D}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B6E}", Form::Infix, 5, 5, 0),
    ("\u{2B6F}", Form::Infix, 5, 5, 0),
    ("\u{2B70}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B71}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B72}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B73}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B76}", Form::Infix, 5, 5, 0),
    ("\u{2B77}", Form::Infix, 5, 5, 0),
    ("\u{2B78}", Form::Infix, 5, 5, 0),
    ("\u{2B79}", Form::Infix, 5, 5, 0),
    ("\u{2B7A}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B7B}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B7C}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B7D}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B80}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B81}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B82}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B83}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B84}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B85}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B86}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2B87}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2B88}", Form::Infix, 5, 5, 0),
    ("\u{2B89}", Form::Infix, 5, 5, 0),
    ("\u{2B8A}", Form::Infix, 5, 5, 0),
    ("\u{2B8B}", Form::Infix, 5, 5, 0),
    ("\u{2B8C}", Form::Infix, 5, 5, 0),
    ("\u{2B8D}", Form::Infix, 5, 5, 0),
    ("\u{2B8E}", Form::Infix, 5, 5, 0),
    ("\u{2B8F}", Form::Infix, 5, 5, 0),
    ("\u{2B94}", Form::Infix, 5, 5, 0),
    ("\u{2B95}", Form::Infix, 5, 5, F::STRETCHY | F::HORIZONTAL),
    ("\u{2BA0}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA1}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA2}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA3}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA4}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA5}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA6}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA7}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA8}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BA9}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAA}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAB}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAC}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAD}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAE}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BAF}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BB0}", Form::Infix, 5, 5, 0),
    ("\u{2BB1}", Form::Infix, 5, 5, 0),
    ("\u{2BB2}", Form::Infix, 5, 5, 0),
    ("\u{2BB3}", Form::Infix, 5, 5, 0),
    ("\u{2BB4}", Form::Infix, 5, 5, 0),
    ("\u{2BB5}", Form::Infix, 5, 5, 0),
    ("\u{2BB6}", Form::Infix, 5, 5, 0),
    ("\u{2BB7}", Form::Infix, 5, 5, 0),
    ("\u{2BB8}", Form::Infix, 5, 5, F::STRETCHY),
    ("\u{2BD1}", Form::Infix, 5, 5, 0),
    ("\u{1EEF0}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
    ("\u{1EEF1}", Form::Postfix, 0, 0, F::STRETCHY | F::HORIZONTAL),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(content: &str, form: Form) -> (u8, u8) {
        let e = entry(content, form);
        (e.lspace, e.rspace)
    }

    #[test]
    fn dictionary_is_sorted_for_lookup() {
        assert!(DICTIONARY.windows(2).all(|w| (w[0].0.as_bytes(), w[0].1) < (w[1].0.as_bytes(), w[1].1)));
        // 1,180 entries in the JSON, minus the three U+00A0 ones that hold the defaults.
        assert_eq!(DICTIONARY.len(), 1177);
    }

    #[test]
    fn dictionary_spacing_of_common_operators() {
        assert_eq!(lookup("+", Form::Infix), (4, 4));
        assert_eq!(lookup("=", Form::Infix), (5, 5));
        assert_eq!(lookup(",", Form::Infix), (0, 3));
        assert_eq!(lookup("+", Form::Prefix), (0, 0));
        // `=` has no prefix entry: its infix one.
        assert_eq!(lookup("=", Form::Prefix), (5, 5));
        // Not in the dictionary: thickmathspace each side.
        assert_eq!(lookup("xyz", Form::Infix), (DEFAULT_SPACING.0, DEFAULT_SPACING.1));
    }

    #[test]
    fn dictionary_properties() {
        let sum = entry("\u{2211}", Form::Prefix);
        assert!(sum.largeop() && sum.movablelimits() && sum.symmetric() && !sum.stretchy());
        let paren = entry("(", Form::Prefix);
        assert!(paren.stretchy() && paren.symmetric() && !paren.largeop());
        assert!(entry("=", Form::Infix).horizontal());
        assert!(!entry("+", Form::Infix).stretchy());
        // The form decides: `|` is stretchy as a prefix or postfix, not as an infix operator.
        assert!(entry("|", Form::Prefix).stretchy() && !entry("|", Form::Infix).stretchy());
        // Not in the dictionary: no property.
        assert_eq!(entry("xyz", Form::Infix).flags, 0);
    }

    #[test]
    fn combining_negation_and_variation_marks_use_the_base_character() {
        // MathML Core: `∃` has a prefix entry only; `∃` followed by U+0338 or U+20D2 is spaced as `∃`.
        assert_eq!(entry("\u{2203}\u{338}", Form::Infix), entry("\u{2203}", Form::Infix));
        assert_eq!(entry("=\u{20D2}", Form::Infix), entry("=", Form::Infix));
        // An entry of the full string wins (`!=`), a longer base is not stripped.
        assert_eq!(entry("!=", Form::Infix), entry("!=", Form::Infix));
        assert_eq!(lookup("xy\u{338}", Form::Infix), (DEFAULT_SPACING.0, DEFAULT_SPACING.1));
    }
}
