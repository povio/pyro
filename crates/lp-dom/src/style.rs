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

use std::cell::RefCell;
use std::rc::Rc;

use cssparser::{ParseError, Parser, Token};
use markup5ever::{local_name, ns};
use rustc_hash::{FxHashMap, FxHashSet};
use selectors::parser::{Component, Selector, SelectorList};

use crate::dom::Dom;
use crate::media::{self, Viewport};
use crate::node::{NodeId, NodeKind, NIL};
use crate::select::{parse_selectors, Impl, PseudoClass, Selectors};

/// The font-size an element inherits when nothing declares one
/// (StyleManager.zig:1998).
pub const DEFAULT_FONT_SIZE: f64 = 16.0;

/// Hard cap on `@media` / `@layer` nesting (StyleManager.zig:115).
const MAX_AT_RULE_NESTING: u8 = 32;
/// StyleManager.zig:1759.
const MAX_LAYERS: usize = 1024;
/// StyleManager.zig:1753.
const NO_LAYER: u16 = u16::MAX;
/// StyleManager.zig:1755.
const UNLAYERED_RANK: u64 = (1 << 11) - 1;
/// Priority bit layout (StyleManager.zig:1761-1768).
const SPEC_SHIFT: u32 = 22;
const RANK_SHIFT: u32 = 52;
const IMPORTANT_BIT: u64 = 1 << 63;
const MAX_DOC_ORDER: u32 = (1 << 22) - 2;
/// StyleManager.zig:1779-1780.
const INLINE_PRIORITY: u64 = IMPORTANT_BIT - 1;
const INLINE_IMPORTANT_PRIORITY: u64 = u64::MAX;
/// StyleManager.zig:1994.
const MAX_FONT_ANCESTOR_DEPTH: u8 = 32;

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

impl Display {
    fn parse(value: &str) -> Display {
        if value.eq_ignore_ascii_case("none") {
            Display::None
        } else if value.eq_ignore_ascii_case("flex") || value.eq_ignore_ascii_case("inline-flex") {
            Display::Flex
        } else if value.eq_ignore_ascii_case("grid") || value.eq_ignore_ascii_case("inline-grid") {
            Display::Grid
        } else {
            Display::Other
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
    /// Declared but needs layout (auto, %, em, ...), or undeclared.
    #[default]
    Auto,
    /// Intrinsic sizes (min-/max-/fit-content).
    Content,
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
            LengthUnit::Auto | LengthUnit::Content => None,
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

// Field indices of the tracked longhands.
const F_DISPLAY: usize = 0;
const F_BLOCK_FG: usize = 1;
const F_VISIBILITY: usize = 2;
const F_OPACITY: usize = 3;
const F_POINTER_EVENTS: usize = 4;
const F_OVERFLOW_X: usize = 5;
const F_OVERFLOW_Y: usize = 6;
const F_OVERSCROLL_X: usize = 7;
const F_OVERSCROLL_Y: usize = 8;
const F_WIDTH: usize = 9;
const F_HEIGHT: usize = 10;
const F_PAN_X: usize = 11;
const F_PAN_Y: usize = 12;
const F_TEXT_TRANSFORM: usize = 13;
const F_ABSOLUTE: usize = 14;
const F_FLOAT: usize = 15;
const NF: usize = 16;

/// Tracked property names, in Declarations.names order
/// (StyleManager.zig:1426, 1510, 1564, 1622, 1641).
const TRACKED: [&str; 14] = [
    "display",
    "visibility",
    "opacity",
    "pointer-events",
    "overflow-x",
    "overflow-y",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "width",
    "height",
    "touch-action",
    "text-transform",
    "position",
    "float",
];

/// One block's parsed values for the tracked properties (the union of
/// upstream's per-group `Declared` structs).
#[derive(Clone, Debug, Default)]
struct Declared {
    display: Option<Display>,
    block_flex_or_grid: Option<bool>,
    /// Inner `None`: an explicit inherit, which still wins the cascade.
    visibility_hidden: Option<Option<bool>>,
    opacity_zero: Option<bool>,
    pointer_events_none: Option<Option<bool>>,
    overflow_x: Option<Overflow>,
    overflow_y: Option<Overflow>,
    overscroll_x: Option<bool>,
    overscroll_y: Option<bool>,
    width: Option<Length>,
    height: Option<Length>,
    pan_x_blocked: Option<bool>,
    pan_y_blocked: Option<bool>,
    text_transform: Option<TextTransform>,
    absolutely_positioned: Option<bool>,
    floated: Option<bool>,
}

/// StyleManager.zig:2035-2046.
fn parse_inherited_flag(value: &str, on: &[&str], off: &[&str]) -> Option<Option<bool>> {
    let ci = |k: &str| value.eq_ignore_ascii_case(k);
    if ci("inherit") || ci("unset") || ci("revert") || ci("revert-layer") {
        return Some(None);
    }
    if on.iter().any(|k| ci(k)) {
        return Some(Some(true));
    }
    if off.iter().any(|k| ci(k)) {
        return Some(Some(false));
    }
    None
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

impl Declared {
    /// The per-group `Declared.apply` functions (StyleManager.zig:1435-1450,
    /// 1519-1547, 1569-1597, 1626-1630, 1646-1653).
    fn apply(&mut self, name: &str, value: &str) {
        let n = |k: &str| name.eq_ignore_ascii_case(k);
        let v = |k: &str| value.eq_ignore_ascii_case(k);
        if n("display") {
            self.display = Some(Display::parse(value));
            self.block_flex_or_grid = Some(v("flex") || v("grid"));
        } else if n("visibility") {
            self.visibility_hidden = parse_inherited_flag(value, &["hidden", "collapse"], &["visible", "initial"]);
        } else if n("opacity") {
            self.opacity_zero = Some(v("0"));
        } else if n("pointer-events") {
            self.pointer_events_none = parse_inherited_flag(
                value,
                &["none"],
                &[
                    "auto",
                    "initial",
                    "visiblePainted",
                    "visibleFill",
                    "visibleStroke",
                    "visible",
                    "painted",
                    "fill",
                    "stroke",
                    "all",
                    "bounding-box",
                ],
            );
        } else if n("overflow-x") {
            self.overflow_x = Some(Overflow::parse(value));
        } else if n("overflow-y") {
            self.overflow_y = Some(Overflow::parse(value));
        } else if n("overscroll-behavior-x") {
            self.overscroll_x = Some(overscroll_contains(value));
        } else if n("overscroll-behavior-y") {
            self.overscroll_y = Some(overscroll_contains(value));
        } else if n("width") {
            self.width = Some(Length::parse(value));
        } else if n("height") {
            self.height = Some(Length::parse(value));
        } else if n("touch-action") {
            if let Some((x, y)) = parse_touch_action(value) {
                self.pan_x_blocked = Some(!x);
                self.pan_y_blocked = Some(!y);
            }
        } else if n("text-transform") {
            self.text_transform = TextTransform::parse(value);
        } else if n("position") {
            self.absolutely_positioned = Some(v("absolute") || v("fixed"));
        } else if n("float") {
            self.floated = Some(v("left") || v("right") || v("inline-start") || v("inline-end"));
        }
    }

    fn mask(&self) -> u32 {
        let mut m = 0u32;
        let mut set = |present: bool, f: usize| {
            if present {
                m |= 1 << f;
            }
        };
        set(self.display.is_some(), F_DISPLAY);
        set(self.block_flex_or_grid.is_some(), F_BLOCK_FG);
        set(self.visibility_hidden.is_some(), F_VISIBILITY);
        set(self.opacity_zero.is_some(), F_OPACITY);
        set(self.pointer_events_none.is_some(), F_POINTER_EVENTS);
        set(self.overflow_x.is_some(), F_OVERFLOW_X);
        set(self.overflow_y.is_some(), F_OVERFLOW_Y);
        set(self.overscroll_x.is_some(), F_OVERSCROLL_X);
        set(self.overscroll_y.is_some(), F_OVERSCROLL_Y);
        set(self.width.is_some(), F_WIDTH);
        set(self.height.is_some(), F_HEIGHT);
        set(self.pan_x_blocked.is_some(), F_PAN_X);
        set(self.pan_y_blocked.is_some(), F_PAN_Y);
        set(self.text_transform.is_some(), F_TEXT_TRANSFORM);
        set(self.absolutely_positioned.is_some(), F_ABSOLUTE);
        set(self.floated.is_some(), F_FLOAT);
        m
    }
}

/// StyleManager.zig:1536-1540 (`overlay` is Chrome's legacy alias of auto).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Overflow {
    #[default]
    Visible,
    Clip,
    Scroll,
}
impl Overflow {
    fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("auto")
            || value.eq_ignore_ascii_case("scroll")
            || value.eq_ignore_ascii_case("overlay")
        {
            Self::Scroll
        } else if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("clip") {
            Self::Clip
        } else {
            Self::Visible
        }
    }
}

/// StyleManager.zig:1544-1547.
fn overscroll_contains(value: &str) -> bool {
    value.eq_ignore_ascii_case("contain") || value.eq_ignore_ascii_case("none")
}

/// One block's winning value per tracked property, folded in declaration
/// order (StyleManager.zig:1830-1884): a normal declaration never overrides
/// an earlier `!important` one; an empty value removes the property.
#[derive(Default)]
struct Slots<'a> {
    slots: [(Option<&'a str>, bool); TRACKED.len()],
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Importance {
    Any,
    Normal,
    Important,
}

impl<'a> Slots<'a> {
    fn apply(&mut self, name: &str, value: &'a str, important: bool) {
        if let Some((x, y)) = axis_shorthand(name) {
            let Some((vx, vy)) = split_axis_pair(value) else { return };
            self.apply(x, vx, important);
            self.apply(y, vy, important);
            return;
        }
        if let Some(i) = TRACKED.iter().position(|t| t.eq_ignore_ascii_case(name)) {
            let slot = &mut self.slots[i];
            if slot.1 && !important {
                return;
            }
            if value.is_empty() {
                *slot = (None, false);
                return;
            }
            *slot = (Some(value), important);
        }
    }

    fn props(&self, importance: Importance) -> Declared {
        let mut d = Declared::default();
        for (name, &(value, important)) in TRACKED.iter().zip(self.slots.iter()) {
            let Some(value) = value else { continue };
            match importance {
                Importance::Any => {}
                Importance::Normal if important => continue,
                Importance::Important if !important => continue,
                _ => {}
            }
            d.apply(name, value);
        }
        d
    }
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

// ---------------------------------------------------------------------------
// Stylesheet parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Item {
    Style {
        selectors: Option<Selectors>,
        normal: Rc<Declared>,
        important: Rc<Declared>,
        normal_mask: u32,
        important_mask: u32,
        /// `--*` declarations, deduplicated per block.
        customs: Vec<(String, Rc<str>)>,
    },
    Media {
        query: String,
        items: Vec<Item>,
    },
    Layer {
        name: String,
        items: Vec<Item>,
    },
    LayerStatement {
        names: String,
    },
}

fn parse_stylesheet(text: &str) -> Vec<Item> {
    let mut p = Parser::new(text);
    let mut out = Vec::new();
    parse_rule_list(&mut p, text, 0, &mut out);
    out
}

/// Top-level / at-rule-body rule list. Only style rules, `@media` and
/// `@layer` participate (StyleManager.zig:117-155, 219-249).
fn parse_rule_list(p: &mut Parser<'_>, src: &str, depth: u8, out: &mut Vec<Item>) {
    loop {
        let start = p.position().byte_index();
        let Some(tok) = next_tok(p) else { return };
        match tok {
            Token::WhiteSpace(_) | Token::Comment(_) | Token::CDO | Token::CDC | Token::Semicolon => {}
            Token::AtKeyword(ref name) => parse_at_rule(p, src, name, depth, out),
            first => {
                // Qualified rule: the prelude runs to the `{}` block.
                let mut tok = first;
                let brace = loop {
                    if let Token::CurlyBracketBlock = tok {
                        break p.position().byte_index() - 1;
                    }
                    consume_block_of(p, &tok);
                    match next_tok(p) {
                        Some(t) => tok = t,
                        None => return,
                    }
                };
                let selector_text = src.get(start..brace).unwrap_or("").trim_matches(CSS_WS_ASCII).to_owned();
                let body: Result<(usize, usize), ParseError<()>> = p.parse_nested_block(|n| {
                    let s = n.position().byte_index();
                    while n.next_including_whitespace_and_comments().is_ok() {}
                    Ok((s, n.position().byte_index()))
                });
                let Ok((s, e)) = body else { continue };
                if let Some(item) = style_item(&selector_text, src.get(s..e).unwrap_or("")) {
                    out.push(item);
                }
            }
        }
    }
}

fn parse_at_rule(p: &mut Parser<'_>, src: &str, name: &str, depth: u8, out: &mut Vec<Item>) {
    let prelude_start = p.position().byte_index();
    let mut has_block = false;
    let prelude_end = loop {
        let before = p.position().byte_index();
        match next_tok(p) {
            Some(Token::Semicolon) => break before,
            Some(Token::CurlyBracketBlock) => {
                has_block = true;
                break before;
            }
            Some(t) => consume_block_of(p, &t),
            None => break p.position().byte_index(),
        }
    };
    let prelude = src.get(prelude_start..prelude_end).unwrap_or("").to_owned();
    let is_media = name.eq_ignore_ascii_case("media");
    let is_layer = name.eq_ignore_ascii_case("layer");
    if depth >= MAX_AT_RULE_NESTING || !(is_media || is_layer) {
        if has_block {
            consume_block_of(p, &Token::CurlyBracketBlock);
        }
        return;
    }
    if !has_block {
        if is_layer {
            out.push(Item::LayerStatement { names: prelude });
        }
        return;
    }
    let mut items = Vec::new();
    let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
        parse_rule_list(n, src, depth + 1, &mut items);
        Ok(())
    });
    let trimmed = prelude.trim_matches(CSS_WS_ASCII).to_owned();
    if is_media {
        out.push(Item::Media { query: trimmed, items });
    } else {
        out.push(Item::Layer { name: trimmed, items });
    }
}

/// addRawRule / foldDeclarations (StyleManager.zig:472-479, 1810-1826).
fn style_item(selector_text: &str, block: &str) -> Option<Item> {
    if selector_text.is_empty() {
        return None;
    }
    let decls = parse_declarations(block);
    let mut slots = Slots::default();
    let mut customs: Vec<(String, Rc<str>, bool)> = Vec::new();
    for d in &decls {
        if is_custom_property(&d.name) {
            if let Some(c) = customs.iter_mut().find(|c| c.0 == d.name) {
                if c.2 && !d.important {
                    continue;
                }
                c.1 = Rc::from(d.value.as_str());
                c.2 = d.important;
            } else {
                customs.push((d.name.clone(), Rc::from(d.value.as_str()), d.important));
            }
            continue;
        }
        slots.apply(&d.name, &d.value, d.important);
    }
    let normal = slots.props(Importance::Normal);
    let important = slots.props(Importance::Important);
    let (normal_mask, important_mask) = (normal.mask(), important.mask());
    if normal_mask == 0 && important_mask == 0 && customs.is_empty() {
        return None;
    }
    Some(Item::Style {
        selectors: parse_selectors(selector_text).ok(),
        normal: Rc::new(normal),
        important: Rc::new(important),
        normal_mask,
        important_mask,
        customs: customs.into_iter().map(|(n, v, _)| (n, v)).collect(),
    })
}

// ---------------------------------------------------------------------------
// Rule set (the flattened cascade input)
// ---------------------------------------------------------------------------

struct Rule {
    selector: Selectors,
    decl: Rc<Declared>,
    mask: u32,
    priority: u64,
}

struct CustomRule {
    selector: Selectors,
    value: Rc<str>,
    priority: u64,
}

#[derive(Default)]
struct RuleSet {
    rules: Vec<Rule>,
    by_id: FxHashMap<String, Vec<u32>>,
    by_class: FxHashMap<String, Vec<u32>>,
    by_tag: FxHashMap<String, Vec<u32>>,
    other: Vec<u32>,
    customs: FxHashMap<String, Vec<CustomRule>>,
}

enum BucketKey {
    Id(String),
    Class(String),
    Tag(String),
    Other,
}

/// StyleManager.zig:1198-1235: best bucket of the rightmost compound, or
/// `None` for selectors with a dynamic pseudo-class or a pseudo-element.
fn bucket_key(sel: &Selector<Impl>) -> Option<BucketKey> {
    if sel.has_pseudo_element() {
        return None;
    }
    let mut best = BucketKey::Other;
    let mut it = sel.iter();
    for c in &mut it {
        match c {
            Component::ID(id) => best = BucketKey::Id(id.0.to_string()),
            Component::Class(class) => {
                if !matches!(best, BucketKey::Id(_)) {
                    best = BucketKey::Class(class.0.to_string());
                }
            }
            Component::LocalName(l) => {
                if matches!(best, BucketKey::Other) {
                    best = BucketKey::Tag(l.lower_name.0.to_string());
                }
            }
            Component::NonTSPseudoClass(pc) => {
                if matches!(
                    pc,
                    PseudoClass::Hover
                        | PseudoClass::Active
                        | PseudoClass::Focus
                        | PseudoClass::FocusWithin
                        | PseudoClass::FocusVisible
                        | PseudoClass::Visited
                        | PseudoClass::Target
                ) {
                    return None;
                }
            }
            Component::PseudoElement(_) => return None,
            _ => {}
        }
    }
    Some(best)
}

struct Layer {
    path: String,
    depth: u16,
    parent: u16,
    rank: u32,
}

struct PendingRule {
    selector: Selector<Impl>,
    decl: Rc<Declared>,
    mask: u32,
    important: bool,
    layer: u16,
    doc: u32,
    bucket: BucketKey,
}

struct PendingCustom {
    name: String,
    selector: Selector<Impl>,
    value: Rc<str>,
    layer: u16,
    doc: u32,
}

/// The rebuild (StyleManager.zig:575-609): walks every active sheet in
/// order, assigning document order and layers, then stamps layer ranks.
struct Builder<'v> {
    viewport: &'v Viewport,
    layers: Vec<Layer>,
    layer_ids: FxHashMap<String, u16>,
    next_anon: u32,
    next_doc: u32,
    rule_layers: Vec<u16>,
    pending: Vec<PendingRule>,
    customs: Vec<PendingCustom>,
}

impl<'v> Builder<'v> {
    fn new(viewport: &'v Viewport) -> Self {
        Builder {
            viewport,
            layers: Vec::new(),
            layer_ids: FxHashMap::default(),
            next_anon: 0,
            next_doc: 1,
            rule_layers: Vec::new(),
            pending: Vec::new(),
            customs: Vec::new(),
        }
    }

    /// StyleManager.zig:551-557 (the rank is stamped in `finish`).
    fn next_doc_order(&mut self, layer: u16) -> u32 {
        self.rule_layers.push(layer);
        let d = self.next_doc.min(MAX_DOC_ORDER);
        self.next_doc += 1;
        d
    }

    fn walk(&mut self, items: &[Item], depth: u8, layer: u16) {
        for item in items {
            match item {
                Item::Style { selectors, normal, important, normal_mask, important_mask, customs } => {
                    let Some(list) = selectors else { continue };
                    // StyleManager.zig:485-525
                    if !customs.is_empty() {
                        let doc = self.next_doc_order(layer);
                        for (name, value) in customs {
                            for sel in list.slice() {
                                if bucket_key(sel).is_none() {
                                    continue;
                                }
                                self.customs.push(PendingCustom {
                                    name: name.clone(),
                                    selector: sel.clone(),
                                    value: value.clone(),
                                    layer,
                                    doc,
                                });
                            }
                        }
                    }
                    if *normal_mask == 0 && *important_mask == 0 {
                        continue;
                    }
                    for sel in list.slice() {
                        let Some(bucket) = bucket_key(sel) else { continue };
                        let doc = self.next_doc_order(layer);
                        if *normal_mask != 0 {
                            self.pending.push(PendingRule {
                                selector: sel.clone(),
                                decl: normal.clone(),
                                mask: *normal_mask,
                                important: false,
                                layer,
                                doc,
                                bucket: clone_bucket(&bucket),
                            });
                        }
                        if *important_mask != 0 {
                            self.pending.push(PendingRule {
                                selector: sel.clone(),
                                decl: important.clone(),
                                mask: *important_mask,
                                important: true,
                                layer,
                                doc,
                                bucket,
                            });
                        }
                    }
                }
                // StyleManager.zig:162-175
                Item::Media { query, items } => {
                    if depth >= MAX_AT_RULE_NESTING || !media::matches(query, self.viewport) {
                        continue;
                    }
                    self.walk(items, depth + 1, layer);
                }
                // StyleManager.zig:186-229 (block form)
                Item::Layer { name, items } => {
                    if depth >= MAX_AT_RULE_NESTING {
                        continue;
                    }
                    if !name.is_empty() && !is_valid_layer_name(name) {
                        continue;
                    }
                    let id = if name.is_empty() {
                        self.intern_anonymous_layer(layer)
                    } else {
                        self.register_layer_path(layer, name)
                    };
                    self.walk(items, depth + 1, id);
                }
                // StyleManager.zig:194-215 (statement form)
                Item::LayerStatement { names } => {
                    if depth >= MAX_AT_RULE_NESTING {
                        continue;
                    }
                    if !names.split(',').all(|n| is_valid_layer_name(n.trim_matches(CSS_WS_ASCII))) {
                        continue;
                    }
                    for n in names.split(',') {
                        self.register_layer_path(layer, n.trim_matches(CSS_WS_ASCII));
                    }
                }
            }
        }
    }

    /// StyleManager.zig:297-314.
    fn register_layer_path(&mut self, parent: u16, dotted: &str) -> u16 {
        let mut current = parent;
        for component in dotted.split('.') {
            if current != NO_LAYER && self.layers[current as usize].depth >= MAX_AT_RULE_NESTING as u16 {
                break;
            }
            current = self.intern_layer(current, component);
        }
        current
    }

    /// StyleManager.zig:317-323.
    fn intern_anonymous_layer(&mut self, parent: u16) -> u16 {
        let name = format!("\0{}", self.next_anon);
        self.next_anon += 1;
        self.intern_layer(parent, &name)
    }

    /// StyleManager.zig:325-346.
    fn intern_layer(&mut self, parent: u16, name: &str) -> u16 {
        let path = if parent == NO_LAYER {
            name.to_owned()
        } else {
            format!("{}.{}", self.layers[parent as usize].path, name)
        };
        if let Some(&id) = self.layer_ids.get(&path) {
            return id;
        }
        if self.layers.len() >= MAX_LAYERS {
            return parent;
        }
        let id = self.layers.len() as u16;
        let depth = if parent == NO_LAYER { 1 } else { self.layers[parent as usize].depth + 1 };
        self.layer_ids.insert(path.clone(), id);
        self.layers.push(Layer { path, depth, parent, rank: 0 });
        id
    }

    /// finalizeLayerRanks (StyleManager.zig:377-454) and the bucketing.
    fn finish(mut self) -> RuleSet {
        if !self.layers.is_empty() {
            let paths: Vec<Vec<u16>> = (0..self.layers.len())
                .map(|i| {
                    let mut path = vec![0u16; self.layers[i].depth as usize];
                    let mut id = i as u16;
                    let mut d = path.len();
                    while d > 0 {
                        d -= 1;
                        path[d] = id;
                        id = self.layers[id as usize].parent;
                    }
                    path
                })
                .collect();
            let mut order: Vec<u16> = (0..self.layers.len() as u16).collect();
            order.sort_by(|&a, &b| {
                let (pa, pb) = (&paths[a as usize], &paths[b as usize]);
                for (ca, cb) in pa.iter().zip(pb.iter()) {
                    if ca != cb {
                        return ca.cmp(cb);
                    }
                }
                // A longer path (sub-layer) ranks below its ancestor.
                pb.len().cmp(&pa.len())
            });
            for (rank, &id) in order.iter().enumerate() {
                self.layers[id as usize].rank = rank as u32;
            }
        }
        let rank_of = |layers: &[Layer], layer: u16, important: bool| -> u64 {
            if layer == NO_LAYER {
                if important {
                    0
                } else {
                    UNLAYERED_RANK
                }
            } else {
                let r = layers[layer as usize].rank as u64;
                if important {
                    UNLAYERED_RANK - r
                } else {
                    r
                }
            }
        };
        let mut rs = RuleSet::default();
        for p in self.pending {
            let spec = p.selector.specificity() as u64;
            let mut priority =
                (rank_of(&self.layers, p.layer, p.important) << RANK_SHIFT) | (spec << SPEC_SHIFT) | p.doc as u64;
            if p.important {
                priority |= IMPORTANT_BIT;
            }
            let idx = rs.rules.len() as u32;
            rs.rules.push(Rule { selector: SelectorList::from_one(p.selector), decl: p.decl, mask: p.mask, priority });
            match p.bucket {
                BucketKey::Id(k) => rs.by_id.entry(k).or_default().push(idx),
                BucketKey::Class(k) => rs.by_class.entry(k).or_default().push(idx),
                BucketKey::Tag(k) => rs.by_tag.entry(k).or_default().push(idx),
                BucketKey::Other => rs.other.push(idx),
            }
        }
        for c in self.customs {
            let spec = c.selector.specificity() as u64;
            let priority = (rank_of(&self.layers, c.layer, false) << RANK_SHIFT) | (spec << SPEC_SHIFT) | c.doc as u64;
            rs.customs.entry(c.name).or_default().push(CustomRule {
                selector: SelectorList::from_one(c.selector),
                value: c.value,
                priority,
            });
        }
        rs
    }
}

fn clone_bucket(b: &BucketKey) -> BucketKey {
    match b {
        BucketKey::Id(s) => BucketKey::Id(s.clone()),
        BucketKey::Class(s) => BucketKey::Class(s.clone()),
        BucketKey::Tag(s) => BucketKey::Tag(s.clone()),
        BucketKey::Other => BucketKey::Other,
    }
}

/// StyleManager.zig:348-372.
fn is_valid_layer_name(dotted: &str) -> bool {
    dotted.split('.').all(is_valid_layer_component)
}

fn is_valid_layer_component(c: &str) -> bool {
    let b = c.as_bytes();
    if b.is_empty() || b[0].is_ascii_digit() {
        return false;
    }
    b.iter().all(|&ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_' || ch >= 0x80)
}

// ---------------------------------------------------------------------------
// Computed values
// ---------------------------------------------------------------------------

/// An element's own cascaded values (upstream `Cascaded`).
#[derive(Clone, Copy, Debug, Default)]
struct Cascaded {
    display: Display,
    block_flex_or_grid: bool,
    visibility_hidden: Option<bool>,
    opacity_zero: bool,
    pointer_events_none: Option<bool>,
    overflow_x: Overflow,
    overflow_y: Overflow,
    overscroll_x: bool,
    overscroll_y: bool,
    width: Length,
    height: Length,
    pan_x_blocked: bool,
    pan_y_blocked: bool,
    text_transform: TextTransform,
    absolutely_positioned: bool,
    floated: bool,
}

fn apply_declared(c: &mut Cascaded, pri: &mut [u64; NF], d: &Declared, p: u64) {
    macro_rules! set {
        ($field:ident, $f:expr) => {
            if let Some(v) = d.$field {
                if p > pri[$f] {
                    c.$field = v;
                    pri[$f] = p;
                }
            }
        };
    }
    set!(display, F_DISPLAY);
    set!(block_flex_or_grid, F_BLOCK_FG);
    set!(visibility_hidden, F_VISIBILITY);
    set!(opacity_zero, F_OPACITY);
    set!(pointer_events_none, F_POINTER_EVENTS);
    set!(overflow_x, F_OVERFLOW_X);
    set!(overflow_y, F_OVERFLOW_Y);
    set!(overscroll_x, F_OVERSCROLL_X);
    set!(overscroll_y, F_OVERSCROLL_Y);
    set!(width, F_WIDTH);
    set!(height, F_HEIGHT);
    set!(pan_x_blocked, F_PAN_X);
    set!(pan_y_blocked, F_PAN_Y);
    set!(text_transform, F_TEXT_TRANSFORM);
    set!(absolutely_positioned, F_ABSOLUTE);
    set!(floated, F_FLOAT);
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
    /// This element or a flat-tree ancestor has display:none.
    pub in_display_none: bool,
    /// This element or a flat-tree ancestor has opacity:0.
    pub in_opacity_zero: bool,
    pub overflow_x_scrolls: bool,
    pub overflow_x_clips: bool,
    pub overflow_y_scrolls: bool,
    pub overflow_y_clips: bool,
    pub overscroll_x_contains: bool,
    pub overscroll_y_contains: bool,
    pub width: Length,
    pub height: Length,
    /// touch-action, intersected with the ancestors'.
    pub pan_x_blocked: bool,
    pub pan_y_blocked: bool,
    /// Own cascaded text-transform (`Inherit` takes the parent's).
    pub text_transform: TextTransform,
    pub absolutely_positioned: bool,
    pub floated: bool,
    /// Fields whose winning declaration is an `!important` sheet rule.
    important_rule_mask: u32,
}

impl ComputedStyle {
    /// StyleManager.zig:1487-1491.
    pub fn is_hidden(&self, opts: CheckVisibilityOptions) -> bool {
        self.in_display_none
            || (opts.check_visibility && self.visibility_hidden)
            || (opts.check_opacity && self.in_opacity_zero)
    }

    /// StyleManager.zig:1475-1485 and 1609-1614.
    fn resolve(own: &Cascaded, mask: u32, parent: &ComputedStyle) -> ComputedStyle {
        ComputedStyle {
            display: own.display,
            block_flex_or_grid: own.block_flex_or_grid,
            author_display: mask & (1 << 31) != 0,
            visibility_hidden: own.visibility_hidden.unwrap_or(parent.visibility_hidden),
            pointer_events_none: own.pointer_events_none.unwrap_or(parent.pointer_events_none),
            in_display_none: own.display == Display::None || parent.in_display_none,
            in_opacity_zero: own.opacity_zero || parent.in_opacity_zero,
            overflow_x_scrolls: own.overflow_x == Overflow::Scroll,
            overflow_x_clips: own.overflow_x != Overflow::Visible,
            overflow_y_scrolls: own.overflow_y == Overflow::Scroll,
            overflow_y_clips: own.overflow_y != Overflow::Visible,
            overscroll_x_contains: own.overscroll_x,
            overscroll_y_contains: own.overscroll_y,
            width: own.width,
            height: own.height,
            pan_x_blocked: own.pan_x_blocked || parent.pan_x_blocked,
            pan_y_blocked: own.pan_y_blocked || parent.pan_y_blocked,
            text_transform: own.text_transform,
            absolutely_positioned: own.absolutely_positioned,
            floated: own.floated,
            important_rule_mask: mask & !(1 << 31),
        }
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
}

struct ParsedSheet {
    active: bool,
    text: String,
    items: Rc<Vec<Item>>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Stamp {
    version: u64,
    generation: u64,
    focused: u32,
    hovered: u32,
}

#[derive(Default)]
struct Cache {
    stamp: Option<Stamp>,
    /// Generation the rule set was built for.
    rules_generation: Option<u64>,
    parsed: Vec<Option<ParsedSheet>>,
    rules: Rc<RuleSet>,
    memo: FxHashMap<NodeId, ComputedStyle>,
    extent: Option<(f64, f64)>,
}

/// The style engine of one document (upstream: one StyleManager per frame).
pub struct StyleEngine {
    doc: NodeId,
    viewport: Viewport,
    sheets: Vec<Sheet>,
    popover_open: FxHashSet<NodeId>,
    generation: u64,
    cache: RefCell<Cache>,
}

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
            cache: RefCell::new(Cache::default()),
        }
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
    }

    /// Adds (or replaces, keeping its position) the sheet owned by `owner`.
    /// Sheets cascade in insertion order, like upstream's StyleSheetList
    /// (`webapi/css/StyleSheetList.zig:28-30`). A sheet applies while its
    /// owner is connected.
    pub fn add_sheet(&mut self, owner: NodeId, source: SheetSource) {
        if let Some(s) = self.sheets.iter_mut().find(|s| s.owner == owner) {
            s.source = source;
        } else {
            self.sheets.push(Sheet { owner, source });
        }
        self.invalidate();
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
            self.cache.get_mut().parsed.clear();
            self.invalidate();
        }
        removed
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
                self.sheets.push(Sheet { owner: n, source: SheetSource::StyleElement });
            }
        }
        self.invalidate();
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

    /// Brings the rule set and memo up to date with `dom`.
    fn ensure(&self, dom: &Dom) {
        let stamp = self.stamp(dom);
        let mut cache = self.cache.borrow_mut();
        if cache.stamp == Some(stamp) {
            return;
        }
        let tree_or_sheets_changed = match cache.stamp {
            Some(old) => old.version != stamp.version || old.generation != stamp.generation,
            None => true,
        };
        if tree_or_sheets_changed {
            let mut changed = cache.rules_generation != Some(self.generation);
            cache.parsed.resize_with(self.sheets.len(), || None);
            let mut buf = String::new();
            for (i, sheet) in self.sheets.iter().enumerate() {
                let active = dom.is_alive(sheet.owner) && dom.is_connected(sheet.owner).unwrap_or(false);
                let text: &str = match &sheet.source {
                    SheetSource::Text(t) => t,
                    SheetSource::StyleElement => {
                        buf.clear();
                        if active {
                            if let Ok(idx) = dom.check(sheet.owner) {
                                dom.collect_text(idx, &mut buf);
                            }
                        }
                        &buf
                    }
                };
                let up_to_date = cache.parsed[i].as_ref().is_some_and(|p| p.active == active && p.text == text);
                if !up_to_date {
                    let items = if active { parse_stylesheet(text) } else { Vec::new() };
                    cache.parsed[i] = Some(ParsedSheet { active, text: text.to_owned(), items: Rc::new(items) });
                    changed = true;
                }
            }
            if changed {
                let mut b = Builder::new(&self.viewport);
                for p in cache.parsed.iter().flatten() {
                    if p.active {
                        b.walk(&p.items, 0, NO_LAYER);
                    }
                }
                cache.rules = Rc::new(b.finish());
                cache.rules_generation = Some(self.generation);
            }
        }
        cache.memo.clear();
        cache.extent = None;
        cache.stamp = Some(stamp);
    }

    pub(crate) fn cached_extent(&self, dom: &Dom) -> Option<(f64, f64)> {
        self.ensure(dom);
        self.cache.borrow().extent
    }

    pub(crate) fn store_extent(&self, extent: (f64, f64)) {
        self.cache.borrow_mut().extent = Some(extent);
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
        let idx = self.owns(dom, el)?;
        self.ensure(dom);
        let mut cache = self.cache.borrow_mut();
        let Cache { rules, memo, .. } = &mut *cache;
        Some(self.computed_idx(dom, rules, memo, idx))
    }

    /// inheritedProps (StyleManager.zig:927-952): resolves the flat-tree path
    /// up to the first memoized ancestor.
    fn computed_idx(
        &self,
        dom: &Dom,
        rules: &RuleSet,
        memo: &mut FxHashMap<NodeId, ComputedStyle>,
        idx: u32,
    ) -> ComputedStyle {
        let mut path: Vec<(u32, Cascaded, u32)> = Vec::new();
        let mut above = ComputedStyle::default();
        let mut cur = Some(idx);
        while let Some(e) = cur {
            if let Some(m) = memo.get(&dom.id_of(e)) {
                above = *m;
                break;
            }
            let (own, mask) = self.cascade_own(dom, rules, e);
            path.push((e, own, mask));
            cur = flat_tree_parent_element(dom, e);
        }
        for (e, own, mask) in path.into_iter().rev() {
            above = ComputedStyle::resolve(&own, mask, &above);
            memo.insert(dom.id_of(e), above);
        }
        above
    }

    /// compute (StyleManager.zig:963-1057) for every group at once. The
    /// returned mask carries the important-rule fields, and bit 31 when an
    /// author declaration set `display`.
    fn cascade_own(&self, dom: &Dom, rules: &RuleSet, idx: u32) -> (Cascaded, u32) {
        let mut c = Cascaded::default();
        let mut pri = [0u64; NF];
        let e = dom.n(idx).element().expect("element");

        if let Some(style) = e.attr("style") {
            let decls = parse_declarations(style);
            let mut slots = Slots::default();
            for d in &decls {
                if !is_custom_property(&d.name) {
                    slots.apply(&d.name, &d.value, d.important);
                }
            }
            apply_declared(&mut c, &mut pri, &slots.props(Importance::Normal), INLINE_PRIORITY);
            apply_declared(&mut c, &mut pri, &slots.props(Importance::Important), INLINE_IMPORTANT_PRIORITY);
        }

        let id = dom.id_of(idx);
        let mut check = |list: &[u32]| {
            for &ri in list {
                let r = &rules.rules[ri as usize];
                let mut relevant = false;
                let mut m = r.mask;
                while m != 0 {
                    let f = m.trailing_zeros() as usize;
                    if r.priority > pri[f] {
                        relevant = true;
                        break;
                    }
                    m &= m - 1;
                }
                if !relevant || !dom.matches(id, &r.selector).unwrap_or(false) {
                    continue;
                }
                apply_declared(&mut c, &mut pri, &r.decl, r.priority);
            }
        };
        if let Some(i) = e.attr("id") {
            if let Some(list) = rules.by_id.get(i) {
                check(list);
            }
        }
        if let Some(class) = e.attr("class") {
            for token in class.split(is_ascii_ws_char).filter(|s| !s.is_empty()) {
                if let Some(list) = rules.by_class.get(token) {
                    check(list);
                }
            }
        }
        if !rules.by_tag.is_empty() {
            let local = e.name.local.to_ascii_lowercase();
            if let Some(list) = rules.by_tag.get(local.as_str()) {
                check(list);
            }
        }
        check(&rules.other);

        let mut mask = 0u32;
        for (f, &p) in pri.iter().enumerate() {
            if p & IMPORTANT_BIT != 0 && p != INLINE_IMPORTANT_PRIORITY {
                mask |= 1 << f;
            }
        }
        // Visibility.finish (StyleManager.zig:1499-1504)
        let author_display = pri[F_DISPLAY] != 0;
        if author_display {
            mask |= 1 << 31;
        } else if self.matches_ua_display_none_rule(dom, idx) {
            c.display = Display::None;
        }
        (c, mask)
    }

    /// matchesUaDisplayNoneRule (StyleManager.zig:1064-1099), HTML Rendering
    /// §15.3.1 "Hidden elements".
    fn matches_ua_display_none_rule(&self, dom: &Dom, idx: u32) -> bool {
        let e = dom.n(idx).element().expect("element");
        let tag = ua_tag(e);
        if let Some(t) = tag {
            // Tag.isHiddenByUaStylesheet (webapi/Element.zig:2629-2648)
            if matches!(
                t,
                "area"
                    | "base"
                    | "datalist"
                    | "head"
                    | "link"
                    | "meta"
                    | "noscript"
                    | "param"
                    | "script"
                    | "source"
                    | "style"
                    | "template"
                    | "title"
                    | "track"
            ) {
                return true;
            }
        }
        if e.attr("hidden").is_some() {
            return true;
        }
        let html = e.is_html();
        if html && tag == Some("input") && dom.input_type_idx(idx) == crate::forms::InputType::Hidden {
            return true;
        }
        let id = dom.id_of(idx);
        if html && tag == Some("dialog") {
            if e.attr("open").is_none() && !self.popover_open.contains(&id) {
                return true;
            }
        } else if e.attr("popover").is_some() && !self.popover_open.contains(&id) {
            return true;
        }
        if !(html && tag == Some("summary")) {
            let p = dom.n(idx).parent;
            if p != NIL {
                if let Some(pe) = dom.n(p).element() {
                    if pe.is_html() && pe.name.local == local_name!("details") && pe.attr("open").is_none() {
                        return true;
                    }
                }
            }
        }
        false
    }

    // ----- queries (StyleManager.zig public API) ---------------------------

    /// isHidden (StyleManager.zig:627-631). Elements without a style (stale,
    /// other document) are hidden.
    pub fn is_hidden(&self, dom: &Dom, el: NodeId, opts: CheckVisibilityOptions) -> bool {
        self.computed(dom, el).is_none_or(|c| c.is_hidden(opts))
    }

    /// StyleManager.zig:638-644.
    pub fn rendering(&self, dom: &Dom, el: NodeId) -> Rendering {
        match self.computed(dom, el) {
            None => Rendering::Visible,
            Some(c) if c.is_hidden(CheckVisibilityOptions::default()) => Rendering::None,
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
    pub fn is_hit_testable(&self, dom: &Dom, el: NodeId) -> bool {
        self.computed(dom, el).is_none_or(|c| !c.visibility_hidden && !c.pointer_events_none)
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

    /// Own computed width/height in px (StyleManager.zig:731-736).
    pub fn declared_size(&self, dom: &Dom, el: NodeId, width: bool) -> Option<f64> {
        let c = self.computed(dom, el)?;
        let l = if width { c.width } else { c.height };
        l.resolve(&self.viewport)
    }

    /// Like `declared_size`, but only the inline style counts (StyleManager.zig:739-743).
    pub fn inline_size(&self, dom: &Dom, el: NodeId, width: bool) -> Option<f64> {
        let style = dom.element(el).ok()?.attr("style")?;
        let decls = parse_declarations(style);
        let mut slots = Slots::default();
        for d in &decls {
            slots.apply(&d.name, &d.value, d.important);
        }
        let d = slots.props(Importance::Any);
        let l = if width { d.width } else { d.height }?;
        l.resolve(&self.viewport)
    }

    /// A block-level flex/grid container in normal flow with an auto width
    /// that isn't a flex/grid item (StyleManager.zig:764-779).
    pub fn width_from_containing_block(&self, dom: &Dom, el: NodeId) -> bool {
        let Some(c) = self.computed(dom, el) else { return false };
        if !c.block_flex_or_grid || c.width.unit != LengthUnit::Auto {
            return false;
        }
        if c.absolutely_positioned || c.floated {
            return false;
        }
        let Ok(idx) = dom.check(el) else { return false };
        let Some(parent) = flat_tree_parent_element(dom, idx) else { return true };
        !matches!(self.display(dom, dom.id_of(parent)), Display::Flex | Display::Grid)
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
        let (value, important) = self.inline_declared_value(dom, el, name)?;
        if !important && self.important_rule_wins(dom, el, name, &value) {
            return None;
        }
        Some(value)
    }

    /// importantRuleWins (StyleManager.zig:1905-1916): whether an
    /// `!important` rule beats a normal inline `name: value`.
    fn important_rule_wins(&self, dom: &Dom, el: NodeId, name: &str, value: &str) -> bool {
        let mut slots = Slots::default();
        slots.apply(name, value, false);
        let declared = slots.props(Importance::Any).mask();
        if declared == 0 {
            return false;
        }
        self.computed(dom, el).is_some_and(|c| c.important_rule_mask & declared != 0)
    }

    /// customPropertyValue (StyleManager.zig:1920-1957): the value of `--x`
    /// on `el` or its nearest flat-tree ancestor declaring it, as written.
    pub fn custom_property_value(&self, dom: &Dom, el: NodeId, name: &str) -> Option<String> {
        self.owns(dom, el)?;
        self.ensure(dom);
        let rules = self.cache.borrow().rules.clone();
        let candidates = rules.customs.get(name);
        let mut cur = dom.check(el).ok();
        while let Some(idx) = cur {
            let id = dom.id_of(idx);
            if let Some(v) = self.inline_style_value(dom, id, name) {
                return Some(v);
            }
            let mut winner: Option<&Rc<str>> = None;
            let mut priority = 0u64;
            for r in candidates.into_iter().flatten() {
                if r.priority <= priority {
                    continue;
                }
                if !dom.matches(id, &r.selector).unwrap_or(false) {
                    continue;
                }
                winner = Some(&r.value);
                priority = r.priority;
            }
            if let Some(v) = winner {
                return Some(v.to_string());
            }
            cur = flat_tree_parent_element(dom, idx);
        }
        None
    }

    /// computedFontSize (StyleManager.zig:2002-2089): inline `font-size`
    /// declarations and `font-size` attributes up the flat tree.
    pub fn computed_font_size(&self, dom: &Dom, el: Option<NodeId>) -> f64 {
        self.font_size_at(dom, el.and_then(|e| dom.check(e).ok()), 0)
    }

    fn font_size_at(&self, dom: &Dom, el: Option<u32>, depth: u8) -> f64 {
        if depth >= MAX_FONT_ANCESTOR_DEPTH {
            return DEFAULT_FONT_SIZE;
        }
        let Some(idx) = el else { return DEFAULT_FONT_SIZE };
        if !dom.n(idx).is_element() {
            return DEFAULT_FONT_SIZE;
        }
        let parent = flat_tree_parent_element(dom, idx);
        let id = dom.id_of(idx);
        if let Some(raw) = self.inline_style_value(dom, id, "font-size") {
            if let Some(size) = self.parse_font_size(dom, &raw, parent, depth + 1) {
                return size;
            }
        }
        if let Some(raw) = dom.n(idx).element().and_then(|e| e.attr("font-size")) {
            let raw = raw.to_owned();
            if let Some(size) = self.parse_font_size(dom, &raw, parent, depth + 1) {
                return size;
            }
        }
        self.font_size_at(dom, parent, depth + 1)
    }

    fn parse_font_size(&self, dom: &Dom, raw: &str, parent: Option<u32>, depth: u8) -> Option<f64> {
        let value = raw.trim_matches(CSS_WS);
        let ci = |k: &str| value.eq_ignore_ascii_case(k);
        if ci("inherit") || ci("unset") {
            return Some(self.font_size_at(dom, parent, depth));
        }
        if ci("initial") {
            return Some(DEFAULT_FONT_SIZE);
        }
        // Absolute-size keywords as Chrome maps them (StyleManager.zig:2049-2058).
        const KEYWORDS: &[(&str, f64)] = &[
            ("xx-small", 9.0),
            ("x-small", 10.0),
            ("small", 13.0),
            ("medium", 16.0),
            ("large", 18.0),
            ("x-large", 24.0),
            ("xx-large", 32.0),
            ("xxx-large", 48.0),
        ];
        if let Some(&(_, s)) = KEYWORDS.iter().find(|(k, _)| ci(k)) {
            return Some(s);
        }
        if ci("smaller") {
            return Some(self.font_size_at(dom, parent, depth) / 1.2);
        }
        if ci("larger") {
            return Some(self.font_size_at(dom, parent, depth) * 1.2);
        }
        let (v, unit) = parse_unit(value)?;
        let factor = match absolute_length_factor(unit) {
            Some(f) => f,
            None => match unit {
                Unit::Percentage => self.font_size_at(dom, parent, depth) / 100.0,
                Unit::Em => self.font_size_at(dom, parent, depth),
                Unit::Ex => self.font_size_at(dom, parent, depth) / 2.0,
                // The root's own `rem` resolves against the initial size.
                Unit::Rem => {
                    if parent.is_none() {
                        DEFAULT_FONT_SIZE
                    } else {
                        let root = dom.document_element(self.doc).ok().flatten().and_then(|r| dom.check(r).ok());
                        self.font_size_at(dom, root, depth)
                    }
                }
                _ => return None,
            },
        };
        let size = v * factor;
        (size >= 0.0 && size.is_finite()).then_some(size)
    }
}

/// The tag name upstream's `Element.Tag` would give, for HTML and SVG
/// elements (lowercase local name); `None` for other namespaces.
pub(crate) fn ua_tag(e: &crate::node::ElementData) -> Option<&str> {
    if e.name.ns == ns!(html) || e.name.ns == ns!(svg) {
        Some(&e.name.local)
    } else {
        None
    }
}

/// flatTreeParentElement (webapi/Node.zig:1756-1775): shadow content takes
/// its host. Slot assignment is not modeled (lp-dom has no assigned-slot
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
