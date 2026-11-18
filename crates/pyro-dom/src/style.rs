//! Stylesheet model and cascade for the property subset upstream's renderless
//! layout acts on. Port of `src/browser/StyleManager.zig` (lightpanda-io/browser
//! @ 2ad38773); line references below are to that file unless noted.
//!
//! Tracked properties (StyleManager.zig:1423-1660): `display`, `visibility`,
//! `opacity` (only `0` matters), `pointer-events`, `overflow-x/-y` (and the
//! `overflow` shorthand), `overscroll-behavior-x/-y` (and shorthand), `width`,
//! `height`, `touch-action`, `text-transform`, `position`, `float`, plus
//! custom properties (`--*`, StyleManager.zig:1706-1735, 1918-1990) and
//! `font-size` resolved from inline styles only (StyleManager.zig:1992-2089).
//!
//! Cascade (StyleManager.zig:963-1057, 1761-1781): per property the
//! declaration with the highest packed priority wins, where priority is
//! `important:1 | layer_rank:11 | specificity:30 | doc_order:22`. Normal inline
//! declarations sit above every normal rule, `!important` rules above those,
//! `!important` inline above everything. `@layer` ranks follow css-cascade-5
//! (StyleManager.zig:361-454); `@media` blocks apply when the query matches
//! the current [`Viewport`] (StyleManager.zig:162-175). Nesting of `@media` /
//! `@layer` is capped at 32 levels (StyleManager.zig:115). Rules whose
//! rightmost compound has a dynamic pseudo-class (`:hover`, `:active`,
//! `:focus`, `:focus-within`, `:focus-visible`, `:visited`, `:target`) or a
//! pseudo-element are skipped (StyleManager.zig:1198-1235).
//!
//! In pyro the cascade itself is the layout engine's (Stylo in
//! Blitz, [`StyleEngine::stylo_style`]): pseudo-element rules apply, and
//! [`StyleEngine::pseudo_style`] reads `::before` / `::after` styles
//! ([`StyleEngine::css_content`] for their generated text).
//!
//! The UA stylesheet subset (`display: none` for unrendered tags, `[hidden]`,
//! `input[type=hidden i]`, closed `<dialog>`, closed popovers, closed
//! `<details>` content) is applied only when no author rule set `display`
//! (StyleManager.zig:1061-1099, 1494-1504).
//!
//! Inheritance: `visibility`, `pointer-events` and the display:none/opacity:0
//! "in hidden subtree" bits inherit along the flat tree (shadow content from
//! its host); `touch-action` blocking ORs with ancestors; `text-transform`
//! inherits along `parentElement`. The others are the element's own value.
//!
//! Memoization: computed values are cached per element and dropped whenever
//! `Dom::version()`, the engine's own generation (sheets, viewport, popover
//! state, [`StyleEngine::invalidate`]) or the document's focused/hovered
//! element changes (upstream's `Page.style_version`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cssparser::{ParseError, Parser, Token};
use markup5ever::local_name;
use rustc_hash::FxHashSet;

use crate::dom::Dom;
use crate::media::{self, Viewport};
use crate::node::{NodeId, NodeKind, NIL};

/// The font-size an element inherits when nothing declares one
/// (StyleManager.zig:1998).
pub const DEFAULT_FONT_SIZE: f64 = 16.0;

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// Only the `display` values the layout acts on (StyleManager.zig:1307-1319).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Display {
    None,
    Flex,
    Grid,
    #[default]
    Other,
}

/// The tree-abiding pseudo-elements the style engine computes styles for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PseudoElement {
    Before,
    After,
}

impl PseudoElement {
    /// `getComputedStyle`'s `pseudoElt` argument: `::before` / `::after`
    /// (and the legacy `:before` / `:after`), ASCII case-insensitively.
    pub fn parse(text: &str) -> Option<PseudoElement> {
        let name = text.strip_prefix("::").or_else(|| text.strip_prefix(':'))?;
        if name.eq_ignore_ascii_case("before") {
            Some(PseudoElement::Before)
        } else if name.eq_ignore_ascii_case("after") {
            Some(PseudoElement::After)
        } else {
            None
        }
    }
}

/// The case-changing part of `text-transform` (StyleManager.zig:1321-1376).
/// `Inherit` is both the `inherit`/`unset` keywords and "nothing declared".
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum TextTransform {
    #[default]
    Inherit,
    None,
    Capitalize,
    Uppercase,
    Lowercase,
}

impl TextTransform {
    /// `None` for an invalid value, which drops the declaration.
    fn parse(value: &str) -> Option<TextTransform> {
        let t = value.trim();
        let ci = |k: &str| t.eq_ignore_ascii_case(k);
        if ci("inherit") || ci("unset") || ci("revert") || ci("revert-layer") {
            return Some(TextTransform::Inherit);
        }
        if ci("initial") || ci("none") {
            return Some(TextTransform::None);
        }
        let mut result = None;
        let mut any = false;
        for token in t.split(is_ascii_ws_char).filter(|s| !s.is_empty()) {
            any = true;
            let case = if token.eq_ignore_ascii_case("uppercase") {
                TextTransform::Uppercase
            } else if token.eq_ignore_ascii_case("lowercase") {
                TextTransform::Lowercase
            } else if token.eq_ignore_ascii_case("capitalize") {
                TextTransform::Capitalize
            } else if token.eq_ignore_ascii_case("full-width")
                || token.eq_ignore_ascii_case("full-size-kana")
                || token.eq_ignore_ascii_case("math-auto")
            {
                continue;
            } else {
                return None;
            };
            if result.is_some() {
                return None;
            }
            result = Some(case);
        }
        if !any {
            return None;
        }
        Some(result.unwrap_or(TextTransform::None))
    }

    /// The computed-value keyword, as getComputedStyle reports it.
    pub fn keyword(self) -> &'static str {
        match self {
            TextTransform::Inherit | TextTransform::None => "none",
            TextTransform::Capitalize => "capitalize",
            TextTransform::Uppercase => "uppercase",
            TextTransform::Lowercase => "lowercase",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum LengthUnit {
    /// Declared but unsupported (auto, em, ...), or undeclared.
    #[default]
    Auto,
    /// Intrinsic sizes (min-/max-/fit-content).
    Content,
    /// Percentage; requires a definite containing size from layout.
    Percentage,
    Px,
    Vw,
    Vh,
}

/// A declared `width` / `height` (StyleManager.zig:1378-1421). The value is
/// kept as `f32` like upstream's packed struct.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    fn parse(text: &str) -> Length {
        if is_intrinsic(text) {
            return Length { value: 0.0, unit: LengthUnit::Content };
        }
        let Some((value, unit)) = parse_unit(text) else { return Length::default() };
        if value < 0.0 {
            return Length::default();
        }
        match unit {
            Unit::Percentage => Length { value: value as f32, unit: LengthUnit::Percentage },
            Unit::Vw => Length { value: value as f32, unit: LengthUnit::Vw },
            Unit::Vh => Length { value: value as f32, unit: LengthUnit::Vh },
            u => match absolute_length_factor(u) {
                Some(f) => Length { value: (value * f) as f32, unit: LengthUnit::Px },
                None => Length::default(),
            },
        }
    }

    /// Pixels, or `None` when it needs layout to resolve.
    pub fn resolve(&self, viewport: &Viewport) -> Option<f64> {
        let v = self.value as f64;
        match self.unit {
            LengthUnit::Auto | LengthUnit::Content | LengthUnit::Percentage => None,
            LengthUnit::Px => Some(v),
            LengthUnit::Vw => Some(v * viewport.width as f64 / 100.0),
            LengthUnit::Vh => Some(v * viewport.height as f64 / 100.0),
        }
    }
}

fn is_intrinsic(text: &str) -> bool {
    ["min-content", "max-content", "fit-content"].iter().any(|k| text.eq_ignore_ascii_case(k))
        || text.get(..12).is_some_and(|p| p.eq_ignore_ascii_case("fit-content("))
}

/// `src/browser/css/units.zig` Unit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    None,
    Percentage,
    Em,
    Rem,
    Ex,
    Px,
    Cm,
    Mm,
    In,
    Pt,
    Pc,
    Vh,
    Vw,
    Deg,
    Rad,
    Grad,
    Turn,
}

/// units.zig:62-79: longest suffixes first.
const UNIT_SUFFIXES: &[(&str, Unit)] = &[
    ("turn", Unit::Turn),
    ("grad", Unit::Grad),
    ("deg", Unit::Deg),
    ("rad", Unit::Rad),
    ("%", Unit::Percentage),
    ("rem", Unit::Rem),
    ("em", Unit::Em),
    ("ex", Unit::Ex),
    ("px", Unit::Px),
    ("cm", Unit::Cm),
    ("mm", Unit::Mm),
    ("in", Unit::In),
    ("pt", Unit::Pt),
    ("pc", Unit::Pc),
    ("vh", Unit::Vh),
    ("vw", Unit::Vw),
];

const CSS_WS: &[char] = &[' ', '\t', '\r', '\n', '\x0c'];

/// units.zig:81-98: lenient dimension lexer.
pub(crate) fn parse_unit(input: &str) -> Option<(f64, Unit)> {
    let value = input.trim_matches(CSS_WS);
    if value.is_empty() {
        return None;
    }
    for &(sfx, unit) in UNIT_SUFFIXES {
        if value.len() <= sfx.len() || !value.is_char_boundary(value.len() - sfx.len()) {
            continue;
        }
        if !value[value.len() - sfx.len()..].eq_ignore_ascii_case(sfx) {
            continue;
        }
        let number = value[..value.len() - sfx.len()].trim_matches(CSS_WS);
        return Some((parse_number(number)?, unit));
    }
    Some((parse_number(value)?, Unit::None))
}

fn parse_number(s: &str) -> Option<f64> {
    let n: f64 = s.parse().ok()?;
    n.is_finite().then_some(n)
}

/// units.zig:110-120: CSS px per absolute unit.
pub(crate) fn absolute_length_factor(unit: Unit) -> Option<f64> {
    Some(match unit {
        Unit::None | Unit::Px => 1.0,
        Unit::Cm => 96.0 / 2.54,
        Unit::Mm => 96.0 / 25.4,
        Unit::In => 96.0,
        Unit::Pt => 96.0 / 72.0,
        Unit::Pc => 16.0,
        _ => return None,
    })
}

fn is_ascii_ws_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Options of `Element.checkVisibility()` (StyleManager.zig:1770-1773).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckVisibilityOptions {
    pub check_visibility: bool,
    pub check_opacity: bool,
}

/// StyleManager.zig:633.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rendering {
    Visible,
    /// Hidden alone (visibility:hidden), a descendant can opt back in.
    Invisible,
    /// The element and its subtree are not rendered (display:none).
    None,
}

/// Scroll axes (upstream `Element.ScrollAxes`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScrollAxes {
    pub x: bool,
    pub y: bool,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// One `name: value [!important]` declaration, as written (value trimmed,
/// `!important` stripped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// Parses a declaration list (a style attribute or a rule body). Port of
/// upstream `css/Parser.zig` DeclarationsIterator (Parser.zig:131-240):
/// at-rules and invalid declarations are skipped, the value spans from its
/// first to its last significant token, and a trailing `! important` pair
/// sets `important`. Empty values are dropped.
pub fn parse_declarations(text: &str) -> Vec<Declaration> {
    let mut p = Parser::new(text);
    let mut out = Vec::new();
    parse_declaration_list(&mut p, text, &mut out);
    out
}

/// The next token (comments and whitespace included), detached from the
/// parser borrow.
fn next_tok<'i>(p: &mut Parser<'i>) -> Option<Token<'i>> {
    p.next_including_whitespace_and_comments().ok().cloned()
}

fn is_block_opener(t: &Token<'_>) -> bool {
    matches!(t, Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock)
}

/// Consumes the contents of the block `t` opened, if it opened one.
fn consume_block_of(p: &mut Parser<'_>, t: &Token<'_>) {
    if is_block_opener(t) {
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok(())
        });
    }
}

fn parse_declaration_list(p: &mut Parser<'_>, src: &str, out: &mut Vec<Declaration>) {
    while let Some(tok) = next_tok(p) {
        match tok {
            Token::WhiteSpace(_) | Token::Comment(_) | Token::Semicolon => {}
            Token::AtKeyword(_) => skip_at_rule(p),
            Token::Ident(ref name) => {
                if let Some(d) = consume_declaration(p, src, name) {
                    out.push(d);
                }
            }
            t => {
                consume_block_of(p, &t);
                skip_invalid_declaration(p);
            }
        }
    }
}

/// Up to `;` or past a `{}` block.
fn skip_at_rule(p: &mut Parser<'_>) {
    while let Some(t) = next_tok(p) {
        match t {
            Token::Semicolon => return,
            Token::CurlyBracketBlock => {
                consume_block_of(p, &t);
                return;
            }
            t => consume_block_of(p, &t),
        }
    }
}

/// Up to and including the next top-level `;`.
fn skip_invalid_declaration(p: &mut Parser<'_>) {
    while let Some(t) = next_tok(p) {
        if let Token::Semicolon = t {
            return;
        }
        consume_block_of(p, &t);
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    bang: bool,
    important_ident: bool,
}

/// Parser.zig:168-234.
fn consume_declaration(p: &mut Parser<'_>, src: &str, name: &str) -> Option<Declaration> {
    loop {
        match next_tok(p)? {
            Token::WhiteSpace(_) | Token::Comment(_) => continue,
            Token::Colon => break,
            Token::Semicolon => return None,
            t => {
                consume_block_of(p, &t);
                skip_invalid_declaration(p);
                return None;
            }
        }
    }
    let mut first: Option<usize> = None;
    let mut last: Option<Span> = None;
    let mut prev: Option<Span> = None;
    loop {
        let start = p.position().byte_index();
        let Some(tok) = next_tok(p) else { break };
        if let Token::Semicolon = tok {
            break;
        }
        consume_block_of(p, &tok);
        let end = p.position().byte_index();
        if matches!(tok, Token::WhiteSpace(_) | Token::Comment(_)) {
            continue;
        }
        let span = Span {
            start,
            end,
            bang: matches!(tok, Token::Delim('!')),
            important_ident: matches!(&tok, Token::Ident(i) if i.eq_ignore_ascii_case("important")),
        };
        first.get_or_insert(start);
        prev = last;
        last = Some(span);
    }
    let value_start = first?;
    let last = last?;
    let mut end = last.end;
    let mut important = false;
    if last.important_ident {
        if let Some(bang) = prev.filter(|s| s.bang) {
            important = true;
            if value_start >= bang.start {
                return None;
            }
            end = bang.start;
        }
    }
    let value = src.get(value_start..end)?.trim_matches(CSS_WS_ASCII);
    if value.is_empty() {
        return None;
    }
    Some(Declaration { name: name.to_owned(), value: value.to_owned(), important })
}

const CSS_WS_ASCII: &[char] = &[' ', '\t', '\r', '\n', '\x0b', '\x0c'];

/// The `<x> [<y>]` shorthands the cascade tracks expanded
/// (`css/Parser.zig:36-41`).
const AXIS_SHORTHANDS: &[(&str, &str, &str)] = &[
    ("overflow", "overflow-x", "overflow-y"),
    ("overscroll-behavior", "overscroll-behavior-x", "overscroll-behavior-y"),
];

fn axis_shorthand(name: &str) -> Option<(&'static str, &'static str)> {
    AXIS_SHORTHANDS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|&(_, x, y)| (x, y))
}

/// `css/Parser.zig:80-88`.
fn split_axis_pair(value: &str) -> Option<(&str, &str)> {
    let mut it = value.split(is_ascii_ws_char).filter(|s| !s.is_empty());
    let x = it.next()?;
    let y = it.next().unwrap_or(x);
    if it.next().is_some() {
        return None;
    }
    Some((x, y))
}

fn is_custom_property(name: &str) -> bool {
    name.starts_with("--")
}

/// StyleManager.zig:1578-1597: the axes a touch may pan along.
fn parse_touch_action(value: &str) -> Option<(bool, bool)> {
    if value.eq_ignore_ascii_case("auto") || value.eq_ignore_ascii_case("manipulation") {
        return Some((true, true));
    }
    if value.eq_ignore_ascii_case("none") {
        return Some((false, false));
    }
    let (mut x, mut y) = (false, false);
    for t in value.split(is_ascii_ws_char).filter(|s| !s.is_empty()) {
        let ci = |k: &str| t.eq_ignore_ascii_case(k);
        if ci("pan-x") || ci("pan-left") || ci("pan-right") {
            x = true;
        } else if ci("pan-y") || ci("pan-up") || ci("pan-down") {
            y = true;
        } else if !ci("pinch-zoom") {
            return None;
        }
    }
    Some((x, y))
}

/// StyleManager.zig:1536-1540 (`overlay` is Chrome's legacy alias of auto).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Overflow {
    #[default]
    Visible,
    Clip,
    Hidden,
    Scroll,
}

/// The computed `overflow-x`/`overflow-y` keyword of one declared value
/// after the per-axis coercion (`visible` → `auto`, `clip` → `hidden` when
/// the other axis scrolls); `overlay` computes to `auto`.
pub(crate) fn computed_overflow_keywords(x: Option<&str>, y: Option<&str>) -> (String, String) {
    let norm = |v: Option<&str>| -> String {
        match v.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if matches!(v.as_str(), "visible" | "hidden" | "clip" | "scroll" | "auto") => v,
            Some(v) if v == "overlay" => "auto".into(),
            _ => "visible".into(),
        }
    };
    let (x, y) = (norm(x), norm(y));
    let scroller = |v: &str| matches!(v, "hidden" | "scroll" | "auto");
    if !(scroller(&x) || scroller(&y)) {
        return (x, y);
    }
    let fix = |v: String| match v.as_str() {
        "visible" => "auto".to_owned(),
        "clip" => "hidden".to_owned(),
        _ => v,
    };
    (fix(x), fix(y))
}

/// StyleManager.zig:1544-1547.
fn overscroll_contains(value: &str) -> bool {
    value.eq_ignore_ascii_case("contain") || value.eq_ignore_ascii_case("none")
}

/// An element's inline `style` attribute as the CSSOM declaration would hold
/// it: names lowercased (custom properties keep their case), axis shorthands
/// stored as their longhands, a normal declaration never overriding an
/// earlier `!important` one, an empty value removing the property
/// (upstream CSSStyleDeclaration.applyDeclarations).
#[derive(Clone, Debug, Default)]
pub struct InlineStyle {
    pub props: Vec<Declaration>,
}

impl InlineStyle {
    pub fn parse(text: &str) -> InlineStyle {
        let mut s = InlineStyle::default();
        for d in parse_declarations(text) {
            s.set(&d.name, &d.value, d.important);
        }
        s
    }

    fn set(&mut self, name: &str, value: &str, important: bool) {
        if let Some((x, y)) = axis_shorthand(name) {
            let Some((vx, vy)) = split_axis_pair(value) else { return };
            let (vx, vy) = (vx.to_owned(), vy.to_owned());
            self.set(x, &vx, important);
            self.set(y, &vy, important);
            return;
        }
        let name = if is_custom_property(name) { name.to_owned() } else { name.to_ascii_lowercase() };
        let pos = self.props.iter().position(|p| p.name == name);
        if let Some(i) = pos {
            if self.props[i].important && !important {
                return;
            }
            if value.is_empty() {
                self.props.remove(i);
                return;
            }
            self.props[i].value = value.to_owned();
            self.props[i].important = important;
        } else if !value.is_empty() {
            self.props.push(Declaration { name, value: value.to_owned(), important });
        }
    }

    /// `declaredValue` + `declaredImportant` (CSSStyleDeclaration.zig:156-228):
    /// an axis shorthand reads as its longhands when both are present with
    /// the same priority.
    pub fn get(&self, name: &str) -> Option<(String, bool)> {
        let find = |n: &str| self.props.iter().find(|p| p.name == n);
        if let Some((x, y)) = axis_shorthand(name) {
            let px = find(x)?;
            let py = find(y)?;
            if px.important != py.important {
                return None;
            }
            let v = if px.value == py.value { px.value.clone() } else { format!("{} {}", px.value, py.value) };
            return Some((v, px.important));
        }
        let key = if is_custom_property(name) { name.to_owned() } else { name.to_ascii_lowercase() };
        find(&key).map(|p| (p.value.clone(), p.important))
    }
}

/// The memoized computed style of one element. Inherited fields are
/// resolved against the flat-tree parent; the rest are the element's own.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComputedStyle {
    /// Own display (author value, else the UA display:none fallback).
    pub display: Display,
    pub block_flex_or_grid: bool,
    /// Whether an author (inline or sheet) declaration set `display`.
    pub author_display: bool,
    /// Inherited `visibility: hidden | collapse`.
    pub visibility_hidden: bool,
    /// Inherited `pointer-events: none`.
    pub pointer_events_none: bool,
    /// This element or a flat-tree ancestor has the `inert` attribute.
    pub inert: bool,
    /// This element or a flat-tree ancestor has display:none.
    pub in_display_none: bool,
    /// This element or a flat-tree ancestor has opacity:0.
    pub in_opacity_zero: bool,
    pub overflow_x_scrolls: bool,
    pub overflow_x_clips: bool,
    pub overflow_y_scrolls: bool,
    pub overflow_y_clips: bool,
    /// A scroll container: `overflow` is `hidden`, `scroll` or `auto` in an
    /// axis (`clip` clips without one).
    pub scroll_container: bool,
    pub overscroll_x_contains: bool,
    pub overscroll_y_contains: bool,
    pub width: Length,
    pub height: Length,
    /// touch-action, intersected with the ancestors'.
    pub pan_x_blocked: bool,
    pub pan_y_blocked: bool,
    /// Own cascaded text-transform (`Inherit` takes the parent's).
    pub text_transform: TextTransform,
    pub white_space: WhiteSpace,
    pub absolutely_positioned: bool,
    pub floated: bool,
    /// Own computed `position`.
    pub position: Position,
    /// Own computed `display` is `contents` (the element generates no box).
    pub display_contents: bool,
    /// Own computed `display` is an atomic inline (`inline-block`,
    /// `inline-flex`, `inline-grid`, `inline-table`).
    pub atomic_inline: bool,
    /// Own computed `display` is a table row or row group.
    pub table_row_or_group: bool,
    /// The box kind of the own computed (blockified) `display`.
    pub box_kind: BoxKind,
    /// Own computed `z-index` (`None`: `auto`).
    pub z_index: Option<i32>,
    /// A `transform` other than `none`.
    pub transformed: bool,
    /// `opacity` below 1.
    pub translucent: bool,
    /// `isolation: isolate`.
    pub isolated: bool,
    /// The element is the containing block of its `position: absolute`
    /// descendants: positioned, or [`Self::contains_fixed`], or
    /// `will-change: position` (as Blitz lays them out:
    /// `stylo_taffy::convert::containing_block_claims`).
    pub contains_absolute: bool,
    /// The element is the containing block of its `position: fixed` (and
    /// absolute) descendants without being positioned: a transform,
    /// perspective, filter, backdrop filter, `contain: layout | paint`, a
    /// size container, or `will-change` of those. A non-atomic inline box
    /// only through a filter or `will-change`.
    pub contains_fixed: bool,
    /// Own computed `user-select`.
    pub user_select: UserSelect,
}

/// The kind of box an element's computed `display` generates, after
/// Stylo's blockification of flex/grid items, floats and absolutely
/// positioned boxes (what `innerText` breaks lines at).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum BoxKind {
    /// `display: none`.
    None,
    /// `display: contents`: no box of its own.
    Contents,
    /// An inline-level box (`inline`, `inline-block`, `inline-flex`,
    /// `inline-table`, ...).
    #[default]
    Inline,
    /// A block-level box (`block`, `flow-root`, `flex`, `grid`,
    /// `list-item`, `table`) or `table-caption`.
    Block,
    TableCell,
    TableRow,
    /// A table row group (`table-row-group`, `-header-group`, `-footer-group`).
    TableRowGroup,
    /// `table-column` / `table-column-group`.
    TableColumn,
}

/// Computed `position`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

impl Position {
    /// Anything but `static`.
    pub fn is_positioned(self) -> bool {
        self != Position::Static
    }

    /// The computed-value keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            Position::Static => "static",
            Position::Relative => "relative",
            Position::Absolute => "absolute",
            Position::Fixed => "fixed",
            Position::Sticky => "sticky",
        }
    }
}

/// Computed `user-select`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UserSelect {
    #[default]
    Auto,
    Text,
    None,
    Contain,
    All,
}

/// What computed `white-space-collapse` does to rendered text.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WhiteSpace {
    /// Spaces and newlines collapse.
    #[default]
    Collapse,
    /// `pre-line`: spaces collapse, newlines stay.
    PreserveBreaks,
    /// `pre`, `pre-wrap`, `break-spaces`.
    Preserve,
}

impl ComputedStyle {
    /// StyleManager.zig:1487-1491.
    pub fn is_hidden(&self, opts: CheckVisibilityOptions) -> bool {
        self.in_display_none
            || (opts.check_visibility && self.visibility_hidden)
            || (opts.check_opacity && self.in_opacity_zero)
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Where a sheet's text comes from.
#[derive(Clone, Debug)]
pub enum SheetSource {
    /// A `<style>` element: its text content is re-read whenever the DOM
    /// changes (upstream reads it at every rebuild, StyleManager.zig:133-136).
    StyleElement,
    /// Text supplied by the loader (`<link rel=stylesheet>`, CSSOM edits).
    Text(String),
}

struct Sheet {
    owner: NodeId,
    source: SheetSource,
    /// Bumped whenever `source` is set (a loader or CSSOM text, or the
    /// `<style>` element's sheet read again).
    revision: u64,
    /// `CSSStyleSheet.disabled`: the sheet applies nothing (its text is empty
    /// to the renderer). Kept when the sheet is read again (a `<style>`'s
    /// registration can come after a script disabled its sheet) or edited.
    disabled: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Stamp {
    version: u64,
    generation: u64,
    focused: u32,
    hovered: u32,
}

/// The style engine of one document (upstream: one StyleManager per frame).
pub struct StyleEngine {
    doc: NodeId,
    viewport: Viewport,
    sheets: Vec<Sheet>,
    popover_open: FxHashSet<NodeId>,
    generation: u64,
    /// Generation bumps by [`StyleEngine::invalidate`] (anything but a sheet
    /// being added, filled or removed).
    invalidations: u64,
    /// `computed()` answers, for the observation stamp in `memo_at`.
    memo: RefCell<StyleMemo>,
    memo_at: Cell<Option<ObservationStamp>>,
    /// The layout engine, which computed styles come from, and the
    /// observation stamp its styles were last synced at.
    external: RefCell<Option<Engine>>,
    /// Set while the engine syncs (it reads the sheets here, not styles).
    external_syncing: Cell<bool>,
    /// Whether a style rule of the engine's sheets depends on `:active`, as
    /// of its last sync (true until one tells otherwise).
    active_styled: Cell<bool>,
    /// The rules declaring properties the engine's Stylo does not parse
    /// ([`crate::gecko_only`]), for an observation stamp.
    /// for the [`StyleEngine::author_sheets_key`] they were parsed at.
    gecko_rules: RefCell<Option<(AuthorSheetsKey, Rc<Vec<crate::gecko_only::Rule>>)>>,
    /// The animation and transition rules of the sheets ([`crate::timing`]),
    /// for the [`StyleEngine::author_sheets_key`] (and the unregistered
    /// `<style>` elements, with their text revisions) they were read at.
    timing_rules: RefCell<Option<(AuthorSheetsKey, Vec<(NodeId, u64)>, Rc<crate::timing::TimingRules>)>>,
    /// Sheet revisions handed out so far.
    revisions: u64,
    /// Called before the layout engine computes styles anew (CSS rule
    /// usage tracking, `CSS.startRuleUsageTracking`), with the stamp it last
    /// ran for.
    recalc_hook: RefCell<Option<RecalcHook>>,
    recalc_hooked_at: Cell<Option<ObservationStamp>>,
    /// [`Self::set_track_style_changes`].
    track_style_changes: Cell<bool>,
    /// The page's animated values ([`Self::set_animation_source`]), the
    /// stamp of those the engine's styles have, and of those its layout has
    /// (a style-only sync restyles without laying out).
    animation_source: RefCell<Option<crate::animation::SharedAnimationSource>>,
    /// A style `stylo_style` answers for one element instead of the engine's while
    /// `getComputedStyle` serializes a transition's value ([`Self::with_style_override`]).
    style_override: RefCell<Option<(NodeId, style::servo_arc::Arc<style::properties::ComputedValues>)>>,
    animation_synced: Cell<Option<u64>>,
    animation_laid_out: Cell<Option<u64>>,
    /// Debug builds: the last [`StyleEngine::author_sheets_key`] and a hash
    /// of the sheet texts it stood for, to check the key moves whenever the
    /// texts do.
    #[cfg(debug_assertions)]
    checked_key: RefCell<Option<(AuthorSheetsKey, u64)>>,
    /// The constructed sheets each document or shadow root adopts
    /// (`adoptedStyleSheets`), by sheet id, in order; empty unless a
    /// script adopted one.
    adopted: std::collections::BTreeMap<NodeId, Vec<u64>>,
    /// The text of each adopted constructed sheet.
    constructed: rustc_hash::FxHashMap<u64, Rc<str>>,
    /// The document's `CSS.registerProperty()` registrations, in order.
    registered_properties: Vec<RegisteredProperty>,
}

/// One `CSS.registerProperty()` registration (validated by
/// [`crate::stylo::validate_property_registration`]).
#[derive(Clone, Debug)]
pub struct RegisteredProperty {
    pub name: String,
    pub syntax: String,
    pub inherits: bool,
    pub initial_value: Option<String>,
}

/// [`StyleEngine::author_sheets_key`]: equal keys, equal author sheet texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorSheetsKey {
    /// The sheet generation: sheets added, removed or given text, viewport
    /// and other invalidations.
    generation: u64,
    /// [`Dom::style_text_revision`]: a `<style>`'s child text changed.
    text_revision: u64,
    /// Which sheets (in cascade order) have a connected owner: bits 0-63,
    /// then the rest 64 per word (empty for at most 64 sheets).
    connected: u64,
    connected_rest: Vec<u64>,
}

/// [`StyleEngine::set_recalc_hook`].
pub type RecalcHook = Rc<dyn Fn(&Dom, &StyleEngine)>;

/// [`StyleEngine::observation_stamp`]: DOM version, sheet generation,
/// focused and hovered elements.
type ObservationStamp = (u64, u64, u32, u32);

/// The layout engine and the stamp its styles were last synced at.
type Engine = (Rc<dyn crate::geometry::GeometrySource>, Option<ObservationStamp>);

impl StyleEngine {
    pub fn new(doc: NodeId) -> Self {
        Self::with_viewport(doc, Viewport::default())
    }

    pub fn with_viewport(doc: NodeId, viewport: Viewport) -> Self {
        StyleEngine {
            doc,
            viewport,
            sheets: Vec::new(),
            popover_open: FxHashSet::default(),
            generation: 0,
            invalidations: 0,
            memo: RefCell::default(),
            memo_at: Cell::new(None),
            external: RefCell::new(None),
            external_syncing: Cell::new(false),
            active_styled: Cell::new(true),
            gecko_rules: RefCell::new(None),
            timing_rules: RefCell::new(None),
            revisions: 0,
            track_style_changes: Cell::new(false),
            animation_source: RefCell::new(None),
            style_override: RefCell::new(None),
            animation_synced: Cell::new(None),
            animation_laid_out: Cell::new(None),
            #[cfg(debug_assertions)]
            checked_key: RefCell::new(None),
            registered_properties: Vec::new(),
            adopted: Default::default(),
            constructed: Default::default(),
            recalc_hook: RefCell::new(None),
            recalc_hooked_at: Cell::new(None),
        }
    }

    /// Calls `hook` before each style recalculation of the layout engine
    /// (a style or layout query after the document changed); `None`: no more.
    pub fn set_recalc_hook(&self, hook: Option<RecalcHook>) {
        *self.recalc_hook.borrow_mut() = hook;
        self.recalc_hooked_at.set(None);
    }

    pub fn has_recalc_hook(&self) -> bool {
        self.recalc_hook.borrow().is_some()
    }

    /// Runs the recalc hook, once per observation stamp.
    pub(crate) fn before_recalc(&self, dom: &Dom) {
        let Some(hook) = self.recalc_hook.borrow().clone() else { return };
        let stamp = self.observation_stamp(dom);
        if self.recalc_hooked_at.replace(Some(stamp)) != Some(stamp) {
            hook(dom, self);
        }
    }

    /// The computed style of `el`, from the layout engine; `None` for stale
    /// ids, non-elements, elements of other documents and without an engine.
    pub fn stylo_style(
        &self,
        dom: &Dom,
        el: NodeId,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        self.owns(dom, el)?;
        self.external_style(dom, el)
    }

    /// Whether the layout engine records the elements whose computed style a
    /// restyle replaces, for [`Self::take_style_changes`] (told to the engine
    /// before every sync).
    pub fn set_track_style_changes(&self, on: bool) {
        self.track_style_changes.set(on);
    }

    pub fn track_style_changes(&self) -> bool {
        self.track_style_changes.get()
    }

    /// Makes `source` (the page's animations) what the layout engine samples
    /// into its cascade at each sync; `None`: nothing is animated.
    pub fn set_animation_source(&self, source: Option<crate::animation::SharedAnimationSource>) {
        *self.animation_source.borrow_mut() = source;
        self.animation_synced.set(None);
        self.animation_laid_out.set(None);
    }

    /// The animated values' stamp now (`None`: no animation source).
    fn animation_stamp(&self) -> Option<u64> {
        let source = self.animation_source.try_borrow().ok()?;
        Some(source.as_ref()?.stamp())
    }

    /// Whether the animated values changed since the engine's styles last
    /// sampled them ([`Self::take_animation_frame`]): it must sync again
    /// although the document did not change.
    pub fn animations_changed(&self) -> bool {
        self.animation_stamp().is_some_and(|stamp| Some(stamp) != self.animation_synced.get())
    }

    /// [`Self::animations_changed`] for the engine's layout, which a
    /// style-only sync leaves behind.
    pub fn animations_changed_for_layout(&self) -> bool {
        self.animation_stamp().is_some_and(|stamp| Some(stamp) != self.animation_laid_out.get())
    }

    /// The engine laid the document out with the animated values now.
    pub fn animations_laid_out(&self) {
        let stamp = self.animation_stamp();
        self.animation_synced.set(stamp);
        self.animation_laid_out.set(stamp);
    }

    /// The value a running CSS transition of `longhand` gives `el` now
    /// ([`crate::animation::AnimationSource::transition_value`]).
    pub fn transition_value(&self, el: NodeId, longhand: &str) -> Option<crate::animation::TransitionValue> {
        let source = self.animation_source.try_borrow().ok()?.clone()?;
        source.transition_value(el, longhand)
    }

    /// Runs `f` with `style` standing for `el`'s computed style (`stylo_style`); `None` while
    /// another override runs.
    pub fn with_style_override<R>(
        &self,
        el: NodeId,
        style: style::servo_arc::Arc<style::properties::ComputedValues>,
        f: impl FnOnce() -> R,
    ) -> Option<R> {
        if self.style_override.borrow().is_some() {
            return None;
        }
        *self.style_override.borrow_mut() = Some((el, style));
        let out = f();
        *self.style_override.borrow_mut() = None;
        Some(out)
    }

    /// Whether a style override runs ([`Self::with_style_override`]).
    pub fn style_overridden(&self) -> bool {
        self.style_override.borrow().is_some()
    }

    /// The animated values for a sync of the layout engine, now; `None`
    /// without an animation source.
    pub fn take_animation_frame(&self) -> Option<crate::animation::AnimationFrame> {
        let source = self.animation_source.try_borrow().ok()?.clone()?;
        let stamp = source.stamp();
        let frame = source.frame();
        self.animation_synced.set(Some(stamp));
        Some(frame)
    }

    /// The elements of this document whose computed style the engine
    /// replaced since the last call (while tracked), with the style before
    /// and after: a transition's before-change and after-change styles.
    /// Elements gone from `dom` are left out.
    pub fn take_style_changes(
        &self,
        dom: &Dom,
    ) -> Vec<(NodeId, crate::timing::StyleSnapshot, crate::timing::StyleSnapshot)> {
        let external = self.external.borrow();
        let Some((source, _)) = &*external else { return Vec::new() };
        source
            .take_style_changes()
            .into_iter()
            .filter_map(|(key, before, after)| {
                let el = crate::geometry::node_of(dom, key)?;
                self.owns(dom, el)?;
                Some((el, before, after))
            })
            .collect()
    }

    /// Records a `CSS.registerProperty()` registration; `false` (and nothing
    /// changes) when the name is registered already.
    pub fn register_property(&mut self, property: RegisteredProperty) -> bool {
        if self.registered_property(&property.name).is_some() {
            return false;
        }
        self.registered_properties.push(property);
        true
    }

    /// The registration of custom property `name`, if script made one.
    pub fn registered_property(&self, name: &str) -> Option<&RegisteredProperty> {
        self.registered_properties.iter().find(|p| p.name == name)
    }

    /// The layout engine's style of `el`'s pseudo-element `pseudo` (see
    /// [`crate::geometry::GeometrySource::element_pseudo_style`]).
    pub fn stylo_pseudo_style(
        &self,
        dom: &Dom,
        el: NodeId,
        pseudo: &style::selector_parser::PseudoElement,
        generated_only: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        self.owns(dom, el)?;
        self.engine_styled(dom)?;
        let external = self.external.borrow();
        external.as_ref()?.0.element_pseudo_style(crate::snapshot::render_key(el), pseudo, generated_only)
    }

    /// `false` when no element of the document has a generated `::before` /
    /// `::after` (styles brought up to `dom` first): then
    /// [`Self::stylo_pseudo_style`] with `generated_only` answers `None` for
    /// both of every element. One flag read on the layout engine.
    pub fn may_have_generated_before_after(&self, dom: &Dom) -> bool {
        if self.engine_styled(dom).is_none() {
            return false;
        }
        let external = self.external.borrow();
        external.as_ref().is_some_and(|(source, _)| source.may_have_generated_before_after())
    }

    /// The layout engine's style of `el` (its styles brought up to `dom`).
    fn external_style(
        &self,
        dom: &Dom,
        el: NodeId,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        if let Some((at, style)) = &*self.style_override.borrow() {
            if *at == el {
                return Some(style.clone());
            }
        }
        self.engine_styled(dom)?;
        let external = self.external.borrow();
        external.as_ref()?.0.element_style(crate::snapshot::render_key(el))
    }

    /// `el`'s starting style (`@starting-style`) from the layout engine (`None`: none applies).
    pub fn starting_style(
        &self,
        dom: &Dom,
        el: NodeId,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        self.owns(dom, el)?;
        self.engine_styled(dom)?;
        let external = self.external.borrow();
        external.as_ref()?.0.element_starting_style(crate::snapshot::render_key(el))
    }

    /// Makes `source` (the document's layout engine) the source of computed
    /// styles; `None`: elements have none.
    pub fn set_style_engine(&self, source: Option<std::rc::Rc<dyn crate::geometry::GeometrySource>>) {
        *self.external.borrow_mut() = source.map(|s| (s, None));
    }

    /// The layout engine synced `dom` (styles and layout).
    pub fn engine_synced(&self, dom: &Dom) {
        // An engine that samples no animations is not asked again for them.
        if let Some(stamp) = self.animation_stamp() {
            self.animation_synced.set(Some(stamp));
        }
        let stamp = self.observation_stamp(dom);
        if let Some((source, at)) = self.external.borrow_mut().as_mut() {
            *at = Some(stamp);
            self.active_styled.set(source.styles_depend_on_active());
        }
    }

    /// Whether which element is `:active` can change a computed style: some
    /// sheet depends on it, or no layout engine answers for the sheets (see
    /// [`crate::Dom::set_activated`]). Sheets change only with the DOM
    /// version or the sheet generation, both in the observation stamp, so
    /// the engine syncs for a change before styles are read again.
    pub fn active_styled(&self) -> bool {
        match self.external.borrow().as_ref() {
            Some((source, _)) if source.active() => self.active_styled.get(),
            _ => true,
        }
    }

    /// Brings the layout engine's styles up to `dom` (layout is left to the
    /// next geometry query); the stamp they answer for, `None` when
    /// there are no computed styles (no engine, or it is inactive or syncing).
    pub fn engine_styled(&self, dom: &Dom) -> Option<(u64, u64, u32, u32)> {
        if self.external_syncing.get() {
            return None;
        }
        let stamp = self.observation_stamp(dom);
        let source = {
            let external = self.external.borrow();
            let (source, at) = external.as_ref()?;
            if *at == Some(stamp) && !source.stale() && !self.animations_changed() {
                return Some(stamp);
            }
            if !source.active() {
                return None;
            }
            source.clone()
        };
        self.before_recalc(dom);
        self.external_syncing.set(true);
        source.sync_styles(dom, self);
        self.external_syncing.set(false);
        self.engine_synced(dom);
        Some(stamp)
    }

    /// `getComputedStyle(el).getPropertyValue(name)` as Stylo serializes the
    /// computed value (layout-dependent values are the layout's business).
    pub fn stylo_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        let style = self.stylo_style(dom, el)?;
        crate::stylo::serialize(&style, name)
    }

    /// The computed style of `el`'s `::before` / `::after` from the layout
    /// engine: `None` when it generates no box of a rendered element, unless
    /// `synthesize`, which then answers as `getComputedStyle(el, '::before')`
    /// does ([`Self::stylo_pseudo_style`]).
    pub fn pseudo_style(
        &self,
        dom: &Dom,
        el: NodeId,
        pseudo: PseudoElement,
        synthesize: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        let pseudo = match pseudo {
            PseudoElement::Before => style::selector_parser::PseudoElement::Before,
            PseudoElement::After => style::selector_parser::PseudoElement::After,
        };
        self.stylo_pseudo_style(dom, el, &pseudo, !synthesize)
    }

    /// Playwright's `getCSSContent`: the text CSS `content` puts in
    /// `pseudo` (`None`: the element's own `content`, which counts only
    /// for its alternative text after `/`). `None` when there is none:
    /// `content: none | normal`, a `display: none` or `visibility: hidden`
    /// box, or content that is not all strings and `attr()` (counters,
    /// quotes and images give nothing, as Playwright's parser rejects
    /// them). A `::before` / `::after` that is not `display: inline` is
    /// padded with a space on each side.
    pub fn css_content(&self, dom: &Dom, el: NodeId, pseudo: Option<PseudoElement>) -> Option<String> {
        use style::values::generics::counters::{Content, ContentItem};
        use style::values::specified::box_::Display as D;
        let style = match pseudo {
            Some(p) => self.pseudo_style(dom, el, p, false)?,
            None => self.stylo_style(dom, el)?,
        };
        let Content::Items(items) = &style.get_counters().content else { return None };
        let b = style.get_box();
        if b.display == D::None || style.get_inherited_box().visibility == style::computed_values::visibility::T::Hidden
        {
            return None;
        }
        let (main, alt) = items.items.split_at(items.alt_start.min(items.items.len()));
        let chosen = if !alt.is_empty() {
            alt
        } else if pseudo.is_some() {
            main
        } else {
            return None;
        };
        let mut text = String::new();
        for item in chosen {
            match item {
                ContentItem::String(s) => text.push_str(s),
                _ => return None,
            }
        }
        Some(if pseudo.is_some() && b.display != D::Inline { format!(" {text} ") } else { text })
    }

    /// [`ComputedStyle`] from Stylo's computed values, memoized per style
    /// stamp.
    fn computed_stylo(&self, dom: &Dom, el: NodeId) -> Option<ComputedStyle> {
        let idx = self.owns(dom, el)?;
        // Answers hold for the engine's stamp only.
        let stamp = self.engine_styled(dom)?;
        if self.memo_at.replace(Some(stamp)) != Some(stamp) {
            self.memo.borrow_mut().reset();
        }
        if let Some(c) = self.memo.borrow().get(el) {
            return Some(c);
        }
        let style = self.stylo_style(dom, el)?;
        let parent = flat_tree_parent_element(dom, idx).map(|p| dom.id_of(p));
        // The ancestors not memoized yet, nearest first, up to the first that is memoized or has
        // no computed style (its descendants derive from the default, as for the root). A loop,
        // not a call per level: a tree thousands of levels deep needs no stack for it.
        let mut chain = Vec::new();
        let mut above = ComputedStyle::default();
        let mut next = parent;
        while let Some(p) = next {
            if let Some(c) = self.memo.borrow().get(p) {
                above = c;
                break;
            }
            let Some(p_idx) = self.owns(dom, p) else { break };
            let Some(p_style) = self.stylo_style(dom, p) else { break };
            next = flat_tree_parent_element(dom, p_idx).map(|pp| dom.id_of(pp));
            chain.push((p, p_idx, p_style, next));
        }
        for (p, p_idx, p_style, p_parent) in chain.into_iter().rev() {
            above = self.derive_and_memoize(dom, p, p_idx, &p_style, p_parent, &above);
        }
        Some(self.derive_and_memoize(dom, el, idx, &style, parent, &above))
    }

    /// `computed_stylo` for one element whose parent's computed style is `above`: derives it,
    /// applies the menu-list and modal-dialog rules and memoizes it.
    fn derive_and_memoize(
        &self,
        dom: &Dom,
        el: NodeId,
        idx: u32,
        style: &style::servo_arc::Arc<style::properties::ComputedValues>,
        parent: Option<NodeId>,
        above: &ComputedStyle,
    ) -> ComputedStyle {
        let e = dom.n(idx).element().expect("element");
        let mut c = derive_computed(style, e, above);
        // A drop-down select's children (its options) keep their computed
        // `display` but generate no box, as in Chromium's menu list.
        if parent.is_some_and(|p| dom.element(p).is_ok_and(is_menu_list)) {
            c.in_display_none = true;
        }
        // A modal dialog blocks its document: the root is inert and the
        // topmost modal dialog's subtree is not (HTML "blocked by a modal
        // dialog"). One empty-set check when there is no modal dialog.
        if let Some(modal) = dom.topmost_modal() {
            if modal == el {
                c.inert = e.is_html() && e.attr("inert").is_some();
            } else if parent.is_none() && dom.check(modal).is_ok_and(|m| dom.n(m).owner_doc == dom.n(idx).owner_doc) {
                c.inert = true;
            }
        }
        self.memo.borrow_mut().insert(el, c);
        c
    }

    pub fn document(&self) -> NodeId {
        self.doc
    }

    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Changes the emulated viewport; `@media` rules are re-evaluated on the
    /// next query (CDP `Emulation.setDeviceMetricsOverride`).
    pub fn set_viewport(&mut self, viewport: Viewport) {
        if viewport != self.viewport {
            self.viewport = viewport;
            self.invalidate();
        }
    }

    /// `window.matchMedia(query).matches`.
    pub fn match_media(&self, query: &str) -> bool {
        media::matches(query, &self.viewport)
    }

    /// Drops every memoized answer. Call after state changes the DOM version
    /// doesn't cover (form state written through `control_state_mut`, ...).
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.invalidations += 1;
    }

    /// How many generation bumps were not a sheet being added, filled or
    /// removed (viewport, popovers, state the DOM version does not cover):
    /// while it is unchanged, the sheets are all that changed.
    pub fn invalidations(&self) -> u64 {
        self.invalidations
    }

    /// A sheet was added, or an empty one got its text: the rule set must be
    /// rebuilt, but only elements the new rules match can style differently.
    fn sheets_added(&mut self) {
        self.generation += 1;
    }

    /// Adds (or replaces, keeping its position) the sheet owned by `owner`.
    /// Sheets cascade in insertion order, like upstream's StyleSheetList
    /// (`webapi/css/StyleSheetList.zig:28-30`). A sheet applies while its
    /// owner is connected.
    pub fn add_sheet(&mut self, owner: NodeId, source: SheetSource) {
        self.revisions += 1;
        let revision = self.revisions;
        if let Some(s) = self.sheets.iter_mut().find(|s| s.owner == owner) {
            // The same text again is the same revision.
            let same = matches!((&s.source, &source), (SheetSource::Text(a), SheetSource::Text(b)) if a == b);
            if !same {
                s.revision = revision;
            }
            s.source = source;
        } else {
            self.sheets.push(Sheet { owner, source, revision, disabled: false });
        }
        self.sheets_added();
    }

    /// Replaces the text of a loader-supplied sheet (or adds it).
    pub fn set_sheet_text(&mut self, owner: NodeId, text: &str) {
        self.add_sheet(owner, SheetSource::Text(text.to_owned()));
    }

    pub fn remove_sheet(&mut self, owner: NodeId) -> bool {
        let before = self.sheets.len();
        self.sheets.retain(|s| s.owner != owner);
        let removed = self.sheets.len() != before;
        if removed {
            // A change of the sheets (not an `invalidations` one): its owner
            // leaving the document says what changed.
            self.generation += 1;
        }
        removed
    }

    /// Text the loader or CSSOM gave `owner`'s sheet; `None` for a
    /// `<style>` element read from the DOM, or no sheet.
    pub fn sheet_text(&self, owner: NodeId) -> Option<&str> {
        self.sheets.iter().find(|s| s.owner == owner).and_then(|s| match &s.source {
            _ if s.disabled => Some(""),
            SheetSource::Text(t) => Some(t.as_str()),
            SheetSource::StyleElement => None,
        })
    }

    /// `CSSStyleSheet.disabled` of the sheet `owner` (a `<style>` or `<link>`) owns.
    pub fn sheet_disabled(&self, owner: NodeId) -> bool {
        self.sheets.iter().any(|s| s.owner == owner && s.disabled)
    }

    /// Sets `CSSStyleSheet.disabled` of the sheet of `owner`: a disabled sheet applies nothing.
    /// A `<style>` element not registered yet (a shadow tree's) is registered.
    pub fn set_sheet_disabled(&mut self, owner: NodeId, disabled: bool) {
        self.revisions += 1;
        let revision = self.revisions;
        match self.sheets.iter_mut().find(|s| s.owner == owner) {
            Some(s) if s.disabled == disabled => return,
            Some(s) => {
                s.disabled = disabled;
                s.revision = revision;
            }
            None if !disabled => return,
            None => self.sheets.push(Sheet { owner, source: SheetSource::StyleElement, revision, disabled }),
        }
        self.sheets_added();
    }

    /// Calls `f` with the text of each author sheet whose owner is connected,
    /// in cascade order (CSSOM edits included), then of the constructed
    /// sheets connected documents and shadow roots adopt. A `<style>`'s text
    /// is its child text content (HTML: "the child text content of the style
    /// element"), not the text of deeper descendants.
    pub fn for_each_author_sheet(&self, dom: &Dom, f: &mut dyn FnMut(&str)) {
        for sheet in &self.sheets {
            if sheet.disabled || !dom.is_alive(sheet.owner) || !dom.is_connected(sheet.owner).unwrap_or(false) {
                continue;
            }
            match &sheet.source {
                SheetSource::Text(text) => f(text),
                SheetSource::StyleElement => f(&dom.child_text_content(sheet.owner)),
            }
        }
        for (scope, ids) in &self.adopted {
            if !dom.is_alive(*scope) || !dom.is_connected(*scope).unwrap_or(false) {
                continue;
            }
            for text in ids.iter().filter_map(|id| self.constructed.get(id)) {
                f(text);
            }
        }
    }

    /// [`StyleEngine::for_each_author_sheet`] with the root of the tree each sheet belongs to (the
    /// document or a shadow root).
    pub fn for_each_scoped_author_sheet(&self, dom: &Dom, f: &mut dyn FnMut(NodeId, &str)) {
        for sheet in &self.sheets {
            if sheet.disabled || !dom.is_alive(sheet.owner) || !dom.is_connected(sheet.owner).unwrap_or(false) {
                continue;
            }
            let Ok(owner) = dom.check(sheet.owner) else { continue };
            let scope = dom.id_of(dom.root_idx(owner));
            match &sheet.source {
                SheetSource::Text(text) => f(scope, text),
                SheetSource::StyleElement => f(scope, &dom.child_text_content(sheet.owner)),
            }
        }
        for (scope, ids) in &self.adopted {
            if !dom.is_alive(*scope) || !dom.is_connected(*scope).unwrap_or(false) {
                continue;
            }
            for text in ids.iter().filter_map(|id| self.constructed.get(id)) {
                f(*scope, text);
            }
        }
    }

    /// `scope.adoptedStyleSheets = sheets` (a document or shadow root):
    /// each constructed sheet's id and text, in order.
    pub fn set_adopted(&mut self, scope: NodeId, sheets: Vec<(u64, Rc<str>)>) {
        if sheets.is_empty() && !self.adopted.contains_key(&scope) {
            return;
        }
        let ids: Vec<u64> = sheets.iter().map(|(id, _)| *id).collect();
        for (id, text) in sheets {
            self.constructed.insert(id, text);
        }
        let old = if ids.is_empty() { self.adopted.remove(&scope) } else { self.adopted.insert(scope, ids) };
        // Texts no scope adopts any more are dropped.
        for id in old.unwrap_or_default() {
            if !self.adopted.values().any(|ids| ids.contains(&id)) {
                self.constructed.remove(&id);
            }
        }
        // Rendered as elements the render delta does not track: a whole sync.
        self.invalidate();
    }

    /// A constructed sheet's text changed (`replaceSync`, `insertRule`, ...).
    pub fn set_constructed_text(&mut self, id: u64, text: &str) {
        let Some(old) = self.constructed.get_mut(&id) else { return };
        if **old != *text {
            *old = text.into();
            self.invalidate();
        }
    }

    /// The constructed sheets (id and text) `scope` adopts, in order.
    pub fn adopted_sheets(&self, scope: NodeId) -> Vec<(u64, Rc<str>)> {
        let Some(ids) = self.adopted.get(&scope) else { return Vec::new() };
        ids.iter().filter_map(|id| Some((*id, self.constructed.get(id)?.clone()))).collect()
    }

    /// Whether any document or shadow root adopts a constructed sheet.
    pub fn has_adopted(&self) -> bool {
        !self.adopted.is_empty()
    }

    /// A key that changes when the author sheets' texts may have: the sheet
    /// generation (a sheet added, removed or given text), the DOM's
    /// [`Dom::style_text_revision`] (a `<style>`'s child text changed) and
    /// which sheets have a connected owner. Nothing is hashed or read from
    /// the sheets' texts.
    pub fn author_sheets_key(&self, dom: &Dom) -> AuthorSheetsKey {
        let mut connected = 0u64;
        let mut connected_rest = Vec::new();
        for (i, sheet) in self.sheets.iter().enumerate() {
            let on = dom.is_alive(sheet.owner) && dom.is_connected(sheet.owner).unwrap_or(false);
            if i < 64 {
                connected |= u64::from(on) << i;
            } else {
                let (word, bit) = ((i - 64) / 64, (i - 64) % 64);
                if word == connected_rest.len() {
                    connected_rest.push(0);
                }
                connected_rest[word] |= u64::from(on) << bit;
            }
        }
        let key = AuthorSheetsKey {
            generation: self.generation,
            text_revision: dom.style_text_revision(),
            connected,
            connected_rest,
        };
        #[cfg(debug_assertions)]
        self.check_sheets_key(dom, &key);
        key
    }

    /// Debug builds: the sheet texts are the same while the key is.
    #[cfg(debug_assertions)]
    fn check_sheets_key(&self, dom: &Dom, key: &AuthorSheetsKey) {
        use std::hash::Hasher;
        let mut hasher = rustc_hash::FxHasher::default();
        self.for_each_author_sheet(dom, &mut |text| {
            hasher.write(text.as_bytes());
            hasher.write_u8(0xff);
        });
        let hash = hasher.finish();
        let mut last = self.checked_key.borrow_mut();
        if let Some((k, h)) = &*last {
            debug_assert!(k != key || *h == hash, "author sheet texts changed under the same key {key:?}");
        }
        *last = Some((key.clone(), hash));
    }

    /// An identity of `owner`'s sheet text as it is now: its revision for a
    /// loader or CSSOM text (set only through this engine), the element's
    /// [`Dom::style_element_text_revision`] for a `<style>` read from the
    /// DOM (the low bit tells the two apart). `None`: no sheet.
    pub fn sheet_revision(&self, dom: &Dom, owner: NodeId) -> Option<u64> {
        let sheet = self.sheets.iter().find(|s| s.owner == owner)?;
        // A disabled sheet's text is empty: its own identity, its revision of the toggle.
        if sheet.disabled {
            return Some((sheet.revision << 2) | 2);
        }
        Some(match &sheet.source {
            SheetSource::Text(_) => sheet.revision << 2,
            SheetSource::StyleElement => (dom.style_element_text_revision(owner).unwrap_or_default() << 2) | 1,
        })
    }

    pub fn sheet_owners(&self) -> Vec<NodeId> {
        self.sheets.iter().map(|s| s.owner).collect()
    }

    /// Registers every connected HTML `<style>` element of the document not
    /// registered yet, in tree order, as a [`SheetSource::StyleElement`].
    pub fn discover_style_elements(&mut self, dom: &Dom) {
        let Ok(desc) = dom.descendants(self.doc) else { return };
        let found: Vec<NodeId> = desc
            .filter(|&n| {
                dom.node(n)
                    .ok()
                    .and_then(|n| n.element())
                    .is_some_and(|e| e.is_html() && e.name.local == local_name!("style"))
            })
            .collect();
        for n in found {
            if !self.sheets.iter().any(|s| s.owner == n) {
                self.revisions += 1;
                self.sheets.push(Sheet {
                    owner: n,
                    source: SheetSource::StyleElement,
                    revision: self.revisions,
                    disabled: false,
                });
            }
        }
        self.sheets_added();
    }

    /// Opens or closes a popover (`:popover-open`), which the UA rules read.
    pub fn set_popover_open(&mut self, el: NodeId, open: bool) {
        let changed = if open { self.popover_open.insert(el) } else { self.popover_open.remove(&el) };
        if changed {
            self.invalidate();
        }
    }

    pub fn is_popover_open(&self, el: NodeId) -> bool {
        self.popover_open.contains(&el)
    }

    // ----- cache -----------------------------------------------------------

    fn stamp(&self, dom: &Dom) -> Stamp {
        let (focused, hovered) = match dom.check(self.doc) {
            Ok(d) => (dom.focused_idx(d), dom.hovered_idx(d)),
            Err(_) => (NIL, NIL),
        };
        Stamp { version: dom.version(), generation: self.generation, focused, hovered }
    }

    /// Layout-observer invalidation key, including dynamic selector state.
    pub fn observation_stamp(&self, dom: &Dom) -> (u64, u64, u32, u32) {
        let stamp = self.stamp(dom);
        (stamp.version, stamp.generation, stamp.focused, stamp.hovered)
    }

    // ----- cascade ---------------------------------------------------------

    fn owns(&self, dom: &Dom, el: NodeId) -> Option<u32> {
        let idx = dom.check(el).ok()?;
        let n = dom.n(idx);
        (n.is_element() && n.owner_doc == self.doc.idx).then_some(idx)
    }

    /// The memoized computed style of `el`; `None` for stale ids, non-elements
    /// and elements of other documents (upstream: no owner frame).
    pub fn computed(&self, dom: &Dom, el: NodeId) -> Option<ComputedStyle> {
        self.computed_stylo(dom, el)
    }

    // ----- queries (StyleManager.zig public API) ---------------------------

    /// isHidden (StyleManager.zig:627-631). Elements without a style (stale,
    /// other document) are hidden.
    ///
    /// The `display: none` part comes from the layout engine's styles on
    /// the rendered tree ([`StyleEngine::engine_displayed`]) when it can
    /// tell, so the common query (no visibility or opacity check) derives
    /// no [`ComputedStyle`] up the ancestor chain.
    pub fn is_hidden(&self, dom: &Dom, el: NodeId, opts: CheckVisibilityOptions) -> bool {
        match self.engine_displayed(dom, el) {
            Some(false) => return true,
            Some(true) if !opts.check_visibility && !opts.check_opacity => return false,
            _ => {}
        }
        self.computed(dom, el).is_none_or(|c| c.is_hidden(opts))
    }

    /// Whether the layout engine has `el` outside every `display: none`
    /// subtree of the rendered (flat) tree, as of its styles brought up to
    /// `dom`; `None` when there is no engine or it cannot tell.
    pub fn engine_displayed(&self, dom: &Dom, el: NodeId) -> Option<bool> {
        self.owns(dom, el)?;
        self.engine_styled(dom)?;
        let external = self.external.borrow();
        external.as_ref()?.0.element_displayed(crate::snapshot::render_key(el))
    }

    /// StyleManager.zig:638-644.
    pub fn rendering(&self, dom: &Dom, el: NodeId) -> Rendering {
        match self.computed(dom, el) {
            None => Rendering::Visible,
            Some(_) if self.is_hidden(dom, el, CheckVisibilityOptions::default()) => Rendering::None,
            Some(c) if c.visibility_hidden => Rendering::Invisible,
            Some(_) => Rendering::Visible,
        }
    }

    /// Own display, honoring the UA hidden-element rules (StyleManager.zig:653-657).
    pub fn display(&self, dom: &Dom, el: NodeId) -> Display {
        self.computed(dom, el).map_or(Display::Other, |c| c.display)
    }

    /// StyleManager.zig:648-650.
    pub fn has_display_none(&self, dom: &Dom, el: NodeId) -> bool {
        self.display(dom, el) == Display::None
    }

    /// display:none from inline style or an author rule only (StyleManager.zig:663-668).
    pub fn has_author_display_none(&self, dom: &Dom, el: NodeId) -> bool {
        self.computed(dom, el).is_some_and(|c| c.author_display && c.display == Display::None)
    }

    /// StyleManager.zig:673-677.
    pub fn has_visibility_hidden_inherited(&self, dom: &Dom, el: NodeId) -> bool {
        self.computed(dom, el).is_some_and(|c| c.visibility_hidden)
    }

    /// StyleManager.zig:679-683.
    pub fn has_pointer_events_none(&self, dom: &Dom, el: NodeId) -> bool {
        self.computed(dom, el).is_some_and(|c| c.pointer_events_none)
    }

    /// Blink's VisibleToHitTestRequest, ignoring display:none (StyleManager.zig:687-692).
    /// Inert content is not hit-testable (Blink's UsedPointerEvents).
    pub fn is_hit_testable(&self, dom: &Dom, el: NodeId) -> bool {
        self.computed(dom, el).is_none_or(|c| !c.visibility_hidden && !c.pointer_events_none && !c.inert)
    }

    /// Computed text-transform: the nearest own value up `parentElement`
    /// (StyleManager.zig:680-690).
    pub fn text_transform(&self, dom: &Dom, el: NodeId) -> TextTransform {
        let mut cur = Some(el);
        while let Some(e) = cur {
            if let Some(c) = self.computed(dom, e) {
                if c.text_transform != TextTransform::Inherit {
                    return c.text_transform;
                }
            }
            cur = dom.parent_element(e).ok().flatten();
        }
        TextTransform::None
    }

    /// StyleManager.zig:705-709.
    pub fn overflow_axes(&self, dom: &Dom, el: NodeId) -> ScrollAxes {
        self.computed(dom, el)
            .map_or(ScrollAxes::default(), |c| ScrollAxes { x: c.overflow_x_scrolls, y: c.overflow_y_scrolls })
    }

    /// Axes clipped by the cascaded overflow declaration, including hidden/clip.
    pub fn overflow_clip_axes(&self, dom: &Dom, el: NodeId) -> ScrollAxes {
        self.computed(dom, el)
            .map_or(ScrollAxes::default(), |c| ScrollAxes { x: c.overflow_x_clips, y: c.overflow_y_clips })
    }

    /// StyleManager.zig:714-718.
    pub fn overscroll_contain_axes(&self, dom: &Dom, el: NodeId) -> ScrollAxes {
        self.computed(dom, el)
            .map_or(ScrollAxes::default(), |c| ScrollAxes { x: c.overscroll_x_contains, y: c.overscroll_y_contains })
    }

    /// StyleManager.zig:722-727.
    pub fn touch_pan_blocked_axes(&self, dom: &Dom, el: NodeId) -> ScrollAxes {
        self.computed(dom, el).map_or(ScrollAxes::default(), |c| ScrollAxes { x: c.pan_x_blocked, y: c.pan_y_blocked })
    }

    /// The element's inline declaration of `name` (the `el.style` view),
    /// without the cascade check.
    pub fn inline_declared_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<(String, bool)> {
        let style = dom.element(el).ok()?.attr("style")?;
        InlineStyle::parse(style).get(name)
    }

    /// inlineStyleValue (StyleManager.zig:1892-1901): the inline declaration
    /// of `name`, unless it is normal and an `!important` rule beats it.
    pub fn inline_style_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.inline_declared_value(dom, el, name).map(|(value, _)| value)
    }

    /// customPropertyValue (StyleManager.zig:1920-1957): the value of `--x`
    /// on `el` or its nearest flat-tree ancestor declaring it, as written.
    pub fn custom_property_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.stylo_value(dom, el, name).filter(|v| !v.is_empty())
    }

    /// The computed value of a property the layout does not track: the
    /// element's winning declaration (inline or rule), `inherit`/`unset`
    /// and inheritance for inherited properties, `var()` substitution, then
    /// Chromium-style serialization (`computed.rs`). `None` when nothing
    /// applies, so the caller reports the initial value.
    pub fn computed_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.value_with(dom, el, name, false)
    }

    /// Computed `display`: the author value or the UA default, blockified
    /// (css-display-3 §2.7) for the root, floats, absolutely positioned
    /// boxes and flex/grid items, so a `<span>` in a flex `<label>` reports
    /// `block` as in Chromium. `None` as for [`Self::computed_value`].
    pub fn computed_display(&self, dom: &Dom, el: NodeId) -> Option<String> {
        // Stylo blockifies as it computes.
        self.stylo_value(dom, el, "display")
    }

    /// Like [`Self::computed_value`], but numbers and px lengths keep full
    /// precision (Rust's shortest round-trip form) instead of Chromium's
    /// six-digit serialization. Layout inputs use this: rounded
    /// `getComputedStyle` text (e.g. a line-height of 1.42857 x 14px) adds up
    /// to visible sub-pixel drift.
    pub fn used_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.value_with(dom, el, name, true)
    }

    fn value_with(&self, dom: &Dom, el: NodeId, name: &str, precise: bool) -> Option<String> {
        if precise && name == "line-height" {
            // The computed value (a number stays a number), not the
            // resolved length getComputedStyle reports.
            use style::values::generics::font::GenericLineHeight as L;
            let style = self.stylo_style(dom, el)?;
            return Some(match style.get_font().line_height {
                L::Normal => "normal".into(),
                L::Number(n) => format!("{}", n.0),
                L::Length(l) => format!("{}px", style.effective_zoom.unzoom(l.0.px())),
                #[allow(unreachable_patterns)]
                _ => "normal".into(),
            });
        }
        if crate::gecko_only::handles(name) {
            return self.gecko_only_value(dom, el, name);
        }
        self.stylo_value(dom, el, name)
    }

    /// The animation and transition rules of the document's sheets
    /// ([`crate::timing`]), read again only when the sheets changed.
    pub fn timing_rules(&self, dom: &Dom) -> Rc<crate::timing::TimingRules> {
        let key = self.author_sheets_key(dom);
        // A `<style>` inserted in this task is registered once the loader
        // handles the DOM's resource events (after the task); the layout
        // engine styles with it already, so its rules count now, after the
        // registered sheets.
        let unregistered: Vec<(NodeId, u64)> = if dom.has_resource_events() {
            self.unregistered_style_elements(dom)
                .into_iter()
                .map(|n| (n, dom.style_element_text_revision(n).unwrap_or_default()))
                .collect()
        } else {
            Vec::new()
        };
        if let Some((at, pending, rules)) = &*self.timing_rules.borrow() {
            if *at == key && *pending == unregistered {
                return rules.clone();
            }
        }
        // Each sheet's rules match the elements of its own tree (a shadow root's, or the document's).
        let mut texts: Vec<(NodeId, String)> = Vec::new();
        self.for_each_scoped_author_sheet(dom, &mut |scope, text| texts.push((scope, text.to_owned())));
        texts.extend(unregistered.iter().filter_map(|(n, _)| {
            let idx = dom.check(*n).ok()?;
            Some((dom.id_of(dom.root_idx(idx)), dom.child_text_content(*n).into_owned()))
        }));
        let rules = Rc::new(crate::timing::TimingRules::parse_scoped(
            texts.iter().map(|(scope, text)| (*scope, text.as_str())),
            &self.viewport,
        ));
        *self.timing_rules.borrow_mut() = Some((key, unregistered, rules.clone()));
        rules
    }

    /// The document's HTML `<style>` elements no sheet is registered for yet, in tree order.
    fn unregistered_style_elements(&self, dom: &Dom) -> Vec<NodeId> {
        let Ok(desc) = dom.descendants(self.doc) else { return Vec::new() };
        desc.filter(|&n| {
            dom.node(n)
                .ok()
                .and_then(|n| n.element())
                .is_some_and(|e| e.is_html() && e.name.local == local_name!("style"))
                && !self.sheets.iter().any(|s| s.owner == n)
        })
        .collect()
    }

    /// `getComputedStyle` of the transition and animation properties the
    /// page's own timing reads ([`crate::timing::TimingRules::computed_value`]):
    /// the layout engine's cascade has them all `none` (its animation engine
    /// stays off). `None` for other properties.
    pub fn timing_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        if !(name.contains("transition") || name.contains("animation")) {
            return None;
        }
        self.owns(dom, el)?;
        // A style flush first: it registers the `<style>` sheets inserted since.
        self.engine_styled(dom)?;
        let style_of = |e: NodeId| self.stylo_style(dom, e);
        self.timing_rules(dom).computed_value(dom, el, name, &style_of)
    }

    /// A property the engine's Stylo does not parse ([`crate::gecko_only`]),
    /// for a connected element of this document.
    fn gecko_only_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.owns(dom, el)?;
        self.engine_styled(dom)?;
        // The rules change with the sheets (or the viewport, which bumps
        // the generation the key includes), not with every DOM change.
        let key = self.author_sheets_key(dom);
        let rules = match &*self.gecko_rules.borrow() {
            Some((at, rules)) if *at == key => Some(rules.clone()),
            _ => None,
        };
        let rules = rules.unwrap_or_else(|| {
            let mut texts = Vec::new();
            self.for_each_author_sheet(dom, &mut |text| texts.push(text.to_owned()));
            let rules = crate::gecko_only::rules(texts.iter().map(String::as_str), &self.viewport);
            *self.gecko_rules.borrow_mut() = Some((key, rules.clone()));
            rules
        });
        let inline = |e: NodeId| self.inline_declared_value(dom, e, name);
        Some(crate::gecko_only::value(dom, &rules, el, name, &inline))
    }

    /// computedFontSize (StyleManager.zig:2002-2089): inline `font-size`
    /// declarations and `font-size` attributes up the flat tree.
    pub fn computed_font_size(&self, dom: &Dom, el: Option<NodeId>) -> f64 {
        let style = el.and_then(|e| self.stylo_style(dom, e));
        style.map_or(DEFAULT_FONT_SIZE, |s| s.get_font().font_size.computed_size().px() as f64)
    }
}

/// Whether `style` makes its element the containing block of `position:
/// fixed` descendants (and so of absolute ones), and whether it makes it one
/// of absolute descendants without that (`will-change: position`), whatever
/// its `position`: the rules Blitz lays out by
/// (`stylo_taffy::convert::containing_block_claims` and
/// `inline_containing_block_claims`, for a non-atomic inline box, to which
/// transforms and containment do not apply).
fn containing_block_claims(
    style: &style::properties::ComputedValues,
    display: style::values::specified::box_::Display,
) -> (bool, bool) {
    use style::values::computed::{Perspective, Rotate, Scale, Translate};
    use style::values::specified::box_::{Contain, WillChangeBits};
    let b = style.get_box();
    let effects = style.get_effects();
    let filtered = !effects.filter.0.is_empty() || !effects.backdrop_filter.0.is_empty();
    let absolute_only = b.will_change.bits.intersects(WillChangeBits::POSITION);
    if display.is_inline_flow() {
        return (filtered || b.will_change.bits.intersects(WillChangeBits::FIXPOS_CB_NON_SVG), absolute_only);
    }
    let fixed = !b.transform.0.is_empty()
        || !matches!(b.translate, Translate::None)
        || !matches!(b.rotate, Rotate::None)
        || !matches!(b.scale, Scale::None)
        || !matches!(b.perspective, Perspective::None)
        || b.will_change.bits.intersects(
            WillChangeBits::TRANSFORM
                | WillChangeBits::PERSPECTIVE
                | WillChangeBits::FIXPOS_CB_NON_SVG
                | WillChangeBits::CONTAIN,
        )
        || b.contain.intersects(Contain::LAYOUT | Contain::PAINT)
        || filtered;
    (fixed, absolute_only)
}

/// The [`ComputedStyle`] of an element whose Stylo computed values are
/// `style`, below a flat-tree parent computed as `above`. Values read
/// typed; only non-initial values of the rarely set properties go through
/// their serialization.
fn derive_computed(
    style: &style::properties::ComputedValues,
    e: &crate::node::ElementData,
    above: &ComputedStyle,
) -> ComputedStyle {
    use style::values::specified::box_::{
        Display as D, Float as SFloat, Overflow as SOverflow, OverscrollBehavior, PositionProperty, TouchAction,
    };
    let get = |name: &str| crate::stylo::serialize(style, name).unwrap_or_default();
    let b = style.get_box();
    let d = b.display;
    let display = if d == D::None {
        Display::None
    } else if d == D::Flex || d == D::InlineFlex {
        Display::Flex
    } else if d == D::Grid || d == D::InlineGrid {
        Display::Grid
    } else {
        Display::Other
    };
    let overflow = |o: SOverflow| match o {
        SOverflow::Visible => Overflow::Visible,
        SOverflow::Hidden => Overflow::Hidden,
        SOverflow::Clip => Overflow::Clip,
        _ => Overflow::Scroll,
    };
    let (ox, oy) = (overflow(b.overflow_x), overflow(b.overflow_y));
    // The initial values typed, anything else through its serialization.
    let (pan_x, pan_y) = if b.touch_action == TouchAction::AUTO {
        (true, true)
    } else {
        parse_touch_action(&get("touch-action")).unwrap_or((true, true))
    };
    let overscroll =
        |o: OverscrollBehavior, name: &str| o != OverscrollBehavior::Auto && overscroll_contains(&get(name));
    let size = |s: &style::values::computed::Size, name: &str| match s {
        style::values::generics::length::GenericSize::Auto => Length::default(),
        _ => Length::parse(&get(name)),
    };
    let position = style.get_position();
    let ua = if e.is_html() { ua_display_of(&e.name.local) } else { "inline" };
    let effects = style.get_effects();
    let (contains_fixed, contains_absolute_only) = containing_block_claims(style, d);
    ComputedStyle {
        display,
        block_flex_or_grid: d == D::Flex || d == D::Grid,
        author_display: d.to_u16() != display_keyword_bits(ua),
        visibility_hidden: style.get_inherited_box().visibility != style::computed_values::visibility::T::Visible,
        pointer_events_none: style.get_inherited_ui().pointer_events
            == style::values::specified::ui::PointerEvents::None,
        inert: above.inert || (e.is_html() && e.attr("inert").is_some()),
        in_display_none: display == Display::None || above.in_display_none,
        in_opacity_zero: above.in_opacity_zero || effects.opacity == 0.0,
        overflow_x_scrolls: ox == Overflow::Scroll,
        overflow_x_clips: ox != Overflow::Visible,
        overflow_y_scrolls: oy == Overflow::Scroll,
        overflow_y_clips: oy != Overflow::Visible,
        scroll_container: matches!(ox, Overflow::Hidden | Overflow::Scroll)
            || matches!(oy, Overflow::Hidden | Overflow::Scroll),
        overscroll_x_contains: overscroll(b.overscroll_behavior_x, "overscroll-behavior-x"),
        overscroll_y_contains: overscroll(b.overscroll_behavior_y, "overscroll-behavior-y"),
        width: size(&position.width, "width"),
        height: size(&position.height, "height"),
        pan_x_blocked: !pan_x || above.pan_x_blocked,
        pan_y_blocked: !pan_y || above.pan_y_blocked,
        text_transform: if style.get_inherited_text().text_transform.is_empty() {
            TextTransform::None
        } else {
            TextTransform::parse(&get("text-transform")).unwrap_or_default()
        },
        white_space: match style.get_inherited_text().slow_clone_white_space_collapse() {
            style::computed_values::white_space_collapse::T::Collapse => WhiteSpace::Collapse,
            style::computed_values::white_space_collapse::T::PreserveBreaks => WhiteSpace::PreserveBreaks,
            _ => WhiteSpace::Preserve,
        },
        absolutely_positioned: matches!(b.position, PositionProperty::Absolute | PositionProperty::Fixed),
        floated: b.float != SFloat::None,
        position: match b.position {
            PositionProperty::Static => Position::Static,
            PositionProperty::Relative => Position::Relative,
            PositionProperty::Absolute => Position::Absolute,
            PositionProperty::Fixed => Position::Fixed,
            PositionProperty::Sticky => Position::Sticky,
            #[allow(unreachable_patterns)]
            _ => Position::Static,
        },
        display_contents: d == D::Contents,
        atomic_inline: d == D::InlineBlock || d == D::InlineFlex || d == D::InlineGrid || d == D::InlineTable,
        table_row_or_group: d == D::TableRow
            || d == D::TableRowGroup
            || d == D::TableHeaderGroup
            || d == D::TableFooterGroup,
        box_kind: box_kind_of(d),
        z_index: match position.z_index {
            style::values::generics::position::GenericZIndex::Integer(z) => Some(z),
            style::values::generics::position::GenericZIndex::Auto => None,
        },
        transformed: !b.transform.0.is_empty(),
        translucent: effects.opacity < 1.0,
        isolated: b.isolation == style::computed_values::isolation::T::Isolate,
        contains_absolute: b.position != PositionProperty::Static || contains_fixed || contains_absolute_only,
        contains_fixed,
        user_select: {
            use style::values::specified::ui::UserSelect as U;
            match style.get_ui().user_select {
                U::Auto => UserSelect::Auto,
                U::Text => UserSelect::Text,
                U::None => UserSelect::None,
                U::Contain => UserSelect::Contain,
                U::All => UserSelect::All,
            }
        },
    }
}

/// The [`BoxKind`] of a computed Stylo `display`.
fn box_kind_of(d: style::values::specified::box_::Display) -> BoxKind {
    use style::values::specified::box_::{Display as D, DisplayInside as I, DisplayOutside as O};
    if d == D::None {
        return BoxKind::None;
    }
    if d == D::Contents {
        return BoxKind::Contents;
    }
    match d.outside() {
        O::None => BoxKind::None,
        O::Inline => BoxKind::Inline,
        O::Block | O::TableCaption => BoxKind::Block,
        O::InternalTable => match d.inside() {
            I::TableCell => BoxKind::TableCell,
            I::TableRow => BoxKind::TableRow,
            I::TableColumn | I::TableColumnGroup => BoxKind::TableColumn,
            _ => BoxKind::TableRowGroup,
        },
        #[allow(unreachable_patterns)]
        _ => BoxKind::Inline,
    }
}

/// Memoized [`ComputedStyle`]s for one observation stamp: a sparse set
/// over node slots (a slot's position in `dense`, checked against the id
/// stored there), so starting over for a new stamp is O(1) and a lookup is
/// two array reads.
#[derive(Default)]
struct StyleMemo {
    /// Node slot -> index in `dense` (stale when it points past the end or
    /// at another node's entry).
    sparse: Vec<u32>,
    dense: Vec<(NodeId, ComputedStyle)>,
}

impl StyleMemo {
    fn reset(&mut self) {
        self.dense.clear();
    }

    fn get(&self, el: NodeId) -> Option<ComputedStyle> {
        let i = *self.sparse.get(el.idx as usize)? as usize;
        self.dense.get(i).filter(|(id, _)| *id == el).map(|(_, c)| *c)
    }

    fn insert(&mut self, el: NodeId, c: ComputedStyle) {
        let slot = el.idx as usize;
        if slot >= self.sparse.len() {
            self.sparse.resize(slot + 1, u32::MAX);
        }
        let i = self.sparse[slot] as usize;
        if let Some(entry) = self.dense.get_mut(i).filter(|(id, _)| *id == el) {
            entry.1 = c;
            return;
        }
        self.sparse[slot] = self.dense.len() as u32;
        self.dense.push((el, c));
    }
}

/// Whether `e` is a drop-down `<select>` (Chromium's menu list: neither
/// `multiple` nor a `size` above 1), whose children generate no boxes.
pub fn is_menu_list(e: &crate::node::ElementData) -> bool {
    e.is_html()
        && &*e.name.local == "select"
        && e.attr("multiple").is_none()
        && !e.attr("size").and_then(|s| s.trim().parse::<u32>().ok()).is_some_and(|s| s > 1)
}

/// flatTreeParentElement (webapi/Node.zig:1756-1775): shadow content takes
/// its host. Slot assignment is not modeled (pyro-dom has no assigned-slot
/// state), so a slotted node takes its DOM parent.
pub(crate) fn flat_tree_parent_element(dom: &Dom, idx: u32) -> Option<u32> {
    let p = dom.n(idx).parent;
    if p == NIL {
        return None;
    }
    match &dom.n(p).kind {
        NodeKind::Element(_) => Some(p),
        NodeKind::Fragment(f) if f.shadow.is_some() && f.host != NIL => dom.n(f.host).is_element().then_some(f.host),
        _ => None,
    }
}

/// [`ua_display`] of an HTML element's local name, lowercased without
/// allocating when it is lowercase already (any parsed HTML name).
pub(crate) fn ua_display_of(local: &str) -> &'static str {
    if local.bytes().any(|b| b.is_ascii_uppercase()) {
        ua_display(&local.to_ascii_lowercase())
    } else {
        ua_display(local)
    }
}

/// The bits of Stylo's computed `Display` for a keyword [`ua_display`]
/// returns (`u16::MAX` for any other).
fn display_keyword_bits(keyword: &str) -> u16 {
    use style::values::specified::box_::Display as D;
    match keyword {
        "block" => D::Block.to_u16(),
        "list-item" => D::Block.to_u16() | D::LIST_ITEM_MASK,
        "table" => D::Table.to_u16(),
        "table-header-group" => D::TableHeaderGroup.to_u16(),
        "table-row-group" => D::TableRowGroup.to_u16(),
        "table-footer-group" => D::TableFooterGroup.to_u16(),
        "table-row" => D::TableRow.to_u16(),
        "table-cell" => D::TableCell.to_u16(),
        "table-caption" => D::TableCaption.to_u16(),
        "table-column-group" => D::TableColumnGroup.to_u16(),
        "table-column" => D::TableColumn.to_u16(),
        "inline-block" => D::InlineBlock.to_u16(),
        "inline" => D::Inline.to_u16(),
        _ => u16::MAX,
    }
}

/// HTML rendering section display defaults. `display: none` elements are
/// filtered earlier by `has_display_none`.
pub(crate) fn ua_display(tag: &str) -> &'static str {
    match tag {
        "html" | "body" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "dl" | "dt" | "dd"
        | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "form" | "fieldset" | "figure"
        | "figcaption" | "blockquote" | "pre" | "hr" | "address" | "details" | "summary" | "dialog" | "menu"
        | "legend" | "center" | "search" | "hgroup" | "optgroup" | "option" => "block",
        "li" => "list-item",
        "table" => "table",
        "thead" => "table-header-group",
        "tbody" => "table-row-group",
        "tfoot" => "table-footer-group",
        "tr" => "table-row",
        "td" | "th" => "table-cell",
        "caption" => "table-caption",
        "colgroup" => "table-column-group",
        "col" => "table-column",
        "button" | "input" | "select" | "textarea" | "meter" | "progress" => "inline-block",
        _ => "inline",
    }
}
