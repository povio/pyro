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

use crate::computed;
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
/// Bound on inheritance/`currentcolor` recursion for generic computed values.
const MAX_COMPUTED_DEPTH: u8 = 64;

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
    width_variable: Option<Rc<str>>,
    height_variable: Option<Rc<str>>,
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
            self.width_variable = is_variable_length(value).then(|| Rc::from(value));
        } else if n("height") {
            self.height = Some(Length::parse(value));
            self.height_variable = is_variable_length(value).then(|| Rc::from(value));
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
        customs: Vec<(String, Rc<str>, bool)>,
        /// Every other declaration as longhands, for getComputedStyle values
        /// the layout does not track (see `computed.rs`).
        generics: Rc<[(String, Rc<str>, bool)]>,
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
    /// `@property --name { inherits: ...; initial-value: ... }`.
    Property {
        name: String,
        inherits: bool,
        initial: Option<Rc<str>>,
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
    if name.eq_ignore_ascii_case("property") && has_block {
        // Registered custom property (CSS Properties and Values API).
        let body: Result<(usize, usize), ParseError<()>> = p.parse_nested_block(|n| {
            let s = n.position().byte_index();
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok((s, n.position().byte_index()))
        });
        let name = prelude.trim_matches(CSS_WS_ASCII);
        if let (Ok((s, e)), true) = (body, is_custom_property(name)) {
            let decls = parse_declarations(src.get(s..e).unwrap_or(""));
            let get = |n: &str| {
                decls.iter().rev().find(|d| d.name.eq_ignore_ascii_case(n)).map(|d| d.value.trim().to_owned())
            };
            // `inherits` is required; an invalid rule registers nothing.
            if let Some(inherits) = get("inherits").and_then(|v| match v.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }) {
                out.push(Item::Property {
                    name: name.to_owned(),
                    inherits,
                    initial: get("initial-value").map(|v| Rc::from(v.as_str())),
                });
            }
        }
        return;
    }
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
    let mut generics: Vec<(String, Rc<str>, bool)> = Vec::new();
    for d in &decls {
        if !is_custom_property(&d.name) {
            for (name, value) in crate::computed::expand(&d.name, &d.value) {
                if let Some(g) = generics.iter_mut().find(|g| g.0 == name) {
                    if g.2 && !d.important {
                        continue;
                    }
                    g.1 = Rc::from(value.as_str());
                    g.2 = d.important;
                } else {
                    generics.push((name, Rc::from(value.as_str()), d.important));
                }
            }
        }
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
    if normal_mask == 0 && important_mask == 0 && customs.is_empty() && generics.is_empty() {
        return None;
    }
    Some(Item::Style {
        selectors: parse_selectors(selector_text).ok(),
        normal: Rc::new(normal),
        important: Rc::new(important),
        normal_mask,
        important_mask,
        customs,
        generics: generics.into(),
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
    /// `@property` registrations: (inherits, initial value). The last valid
    /// registration of a name wins.
    properties: FxHashMap<String, (bool, Option<Rc<str>>)>,
    /// A `:has()` the selective invalidation cannot bound (sibling-relative,
    /// or an unbounded universal anchor): DOM changes invalidate everything.
    has_relative: bool,
    /// The other `:has()` anchors: changes below a matching ancestor (within
    /// `depth` levels) may restyle it, its subtree and later siblings.
    has_anchors: Vec<HasAnchor>,
    /// Every selector whose match can differ between elements of the same
    /// attributes and inherited style (`:has()`, structural pseudo-classes,
    /// sibling combinators, validity), with its bucket: style sharing
    /// compares their matches on both elements (Stylo's revalidation).
    revalidate: Vec<(Selectors, BucketKey)>,
    /// Keys of compounds testing `:valid`/`:invalid`. A form, fieldset or
    /// select is valid only if its controls are, so changes below one that
    /// matches a key may restyle it, its subtree and later siblings.
    validity_keys: Vec<AnchorKey>,
    /// Keys of state-dependent compounds left of a sibling combinator: an
    /// element matching one (before or after a change) may restyle its later
    /// siblings.
    sibling_keys: Vec<AnchorKey>,
    /// Hashes (`dom::attr_name_hash`, as written and lowercased) of every
    /// attribute name a selector tests.
    attr_names: FxHashSet<u64>,
    /// Keys of every compound left of a sibling combinator.
    structural_sibling_keys: Vec<ChildKey>,
    /// Keys of compounds testing structural pseudo-classes (`:nth-*`,
    /// `:first-child`...), `:empty`, `:focus`/`:focus-visible`,
    /// `:focus-within` and `:hover`.
    nth_keys: Vec<ChildKey>,
    empty_keys: Vec<AnchorKey>,
    focus_keys: Vec<AnchorKey>,
    within_keys: Vec<AnchorKey>,
    hover_keys: Vec<AnchorKey>,
    /// Every author declaration of each selector, bucketed like `rules`, for
    /// the generic computed values (`StyleEngine::declared_map`).
    generic_rules: Vec<GenericRule>,
    generic_by_id: FxHashMap<String, Vec<u32>>,
    generic_by_class: FxHashMap<String, Vec<u32>>,
    generic_by_tag: FxHashMap<String, Vec<u32>>,
    generic_other: Vec<u32>,
}

struct GenericRule {
    selector: Selectors,
    decls: Rc<[(String, Rc<str>, bool)]>,
    normal: u64,
    important: u64,
    /// Bloom hashes its ancestor compounds require (0: none), to skip it for
    /// elements whose ancestors lack one (`AncestorBits`).
    ancestors: [u32; 4],
}

/// Selector ancestor hashes (no-quirks) of `sel`, unpacked; 0 for none.
fn ancestor_hashes(sel: &Selector<Impl>) -> [u32; 4] {
    let h = selectors::parser::AncestorHashes::new(sel, selectors::context::QuirksMode::NoQuirks);
    let mask = selectors::bloom::BLOOM_HASH_MASK;
    [h.packed_hashes[0] & mask, h.packed_hashes[1] & mask, h.packed_hashes[2] & mask, h.fourth_hash()]
}

/// A 2048-bit, two-probe Bloom filter of the ancestor filter hashes of an
/// element's ancestors (`Dom::ancestor_filter_hashes`), like Blink's and
/// Stylo's ancestor filter but memoized per element for a layout pass.
#[derive(Clone)]
struct AncestorBits([u64; 32]);

impl AncestorBits {
    fn probes(hash: u32) -> [usize; 2] {
        let h = (hash & selectors::bloom::BLOOM_HASH_MASK) as usize;
        [h & 2047, (h >> 11) & 2047]
    }
    fn insert(&mut self, hash: u32) {
        for b in Self::probes(hash) {
            self.0[b / 64] |= 1 << (b % 64);
        }
    }
    fn might_contain(&self, hash: u32) -> bool {
        Self::probes(hash).iter().all(|&b| self.0[b / 64] & (1 << (b % 64)) != 0)
    }
}

/// The ancestor filter of element `idx` for the current layout pass; None
/// outside a pass, in quirks mode (case-insensitive classes and ids) and in
/// trees not rooted at a document (shadow trees, where matching may reach
/// the host: full matching there).
fn ancestor_bits(dom: &Dom, idx: u32) -> Option<Rc<AncestorBits>> {
    if dom.in_quirks_mode(idx) {
        return None;
    }
    PASS_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        let memo = &mut m.as_mut()?.ancestor_bits;
        let key = dom_key(dom);
        if let Some(b) = memo.get(&(key, idx)) {
            return b.clone();
        }
        // Walk up to a memoized parent (or the top), then fill in downwards:
        // an element's filter is its parent's plus the parent's own hashes.
        let mut chain = vec![idx];
        let start = loop {
            let top = *chain.last().expect("non-empty");
            match dom.selector_parent(top) {
                None => {
                    let p = dom.n(top).parent;
                    let rooted = p == NIL || matches!(dom.n(p).kind, NodeKind::Document(_));
                    break rooted.then_some((AncestorBits([0; 32]), None));
                }
                Some(p) => match memo.get(&(key, p)) {
                    Some(b) => break b.as_ref().map(|b| ((**b).clone(), Some(p))),
                    None => chain.push(p),
                },
            }
        };
        let Some((mut bits, mut parent)) = start else {
            for x in chain {
                memo.insert((key, x), None);
            }
            return None;
        };
        let mut out = None;
        while let Some(x) = chain.pop() {
            if let Some(p) = parent {
                dom.ancestor_filter_hashes(p, |h| bits.insert(h));
            }
            let rc = Rc::new(bits.clone());
            memo.insert((key, x), Some(rc.clone()));
            out = Some(rc);
            parent = Some(x);
        }
        out
    })
}

struct PendingGeneric {
    selector: Selector<Impl>,
    decls: Rc<[(String, Rc<str>, bool)]>,
    layer: u16,
    doc: u32,
    bucket: BucketKey,
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
    important: bool,
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
    generics: Vec<PendingGeneric>,
    properties: FxHashMap<String, (bool, Option<Rc<str>>)>,
    has_relative: bool,
    has_anchors: Vec<HasAnchor>,
    revalidate: Vec<(Selectors, BucketKey)>,
    validity_keys: Vec<AnchorKey>,
    sibling_keys: Vec<AnchorKey>,
    attr_names: FxHashSet<u64>,
    structural_sibling_keys: Vec<ChildKey>,
    nth_keys: Vec<ChildKey>,
    empty_keys: Vec<AnchorKey>,
    focus_keys: Vec<AnchorKey>,
    within_keys: Vec<AnchorKey>,
    hover_keys: Vec<AnchorKey>,
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
            generics: Vec::new(),
            properties: FxHashMap::default(),
            has_relative: false,
            has_anchors: Vec::new(),
            revalidate: Vec::new(),
            validity_keys: Vec::new(),
            sibling_keys: Vec::new(),
            attr_names: FxHashSet::default(),
            structural_sibling_keys: Vec::new(),
            nth_keys: Vec::new(),
            empty_keys: Vec::new(),
            focus_keys: Vec::new(),
            within_keys: Vec::new(),
            hover_keys: Vec::new(),
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
                Item::Style { selectors, normal, important, normal_mask, important_mask, customs, generics } => {
                    let Some(list) = selectors else { continue };
                    for sel in list.slice() {
                        let mut anchors = Vec::new();
                        if needs_revalidation(sel) {
                            if let Some(bucket) = bucket_key(sel) {
                                self.revalidate.push((SelectorList::from_one(sel.clone()), bucket));
                            }
                        }
                        if collect_has_anchors(sel, &mut anchors) {
                            for a in anchors {
                                if !self.has_anchors.contains(&a) {
                                    self.has_anchors.push(a);
                                }
                            }
                        } else {
                            self.has_relative = true;
                        }
                        let mut keys = Vec::new();
                        if collect_validity_keys(sel, &mut keys) {
                            for k in keys {
                                if !self.validity_keys.contains(&k) {
                                    self.validity_keys.push(k);
                                }
                            }
                        } else {
                            self.has_relative = true;
                        }
                        collect_sibling_keys(sel, &mut self.sibling_keys, &mut self.structural_sibling_keys);
                        collect_attr_names(sel, &mut self.attr_names);
                        let pseudo = |c: &Component<Impl>, wanted: &[PseudoClass]| matches!(c, Component::NonTSPseudoClass(pc) if wanted.contains(pc));
                        collect_nth_keys(sel, &mut self.nth_keys, false);
                        collect_keys(sel, &|c| matches!(c, Component::Empty), &mut self.empty_keys);
                        collect_keys(
                            sel,
                            &|c| pseudo(c, &[PseudoClass::Focus, PseudoClass::FocusVisible]),
                            &mut self.focus_keys,
                        );
                        collect_keys(sel, &|c| pseudo(c, &[PseudoClass::FocusWithin]), &mut self.within_keys);
                        collect_keys(sel, &|c| pseudo(c, &[PseudoClass::Hover]), &mut self.hover_keys);
                    }
                    // StyleManager.zig:485-525; generic declarations share the block's order.
                    if !customs.is_empty() || !generics.is_empty() {
                        let doc = self.next_doc_order(layer);
                        for (name, value, important) in customs {
                            for sel in list.slice() {
                                if bucket_key(sel).is_none() {
                                    continue;
                                }
                                self.customs.push(PendingCustom {
                                    name: name.clone(),
                                    important: *important,
                                    selector: sel.clone(),
                                    value: value.clone(),
                                    layer,
                                    doc,
                                });
                            }
                        }
                        if !generics.is_empty() {
                            for sel in list.slice() {
                                let Some(bucket) = bucket_key(sel) else { continue };
                                self.generics.push(PendingGeneric {
                                    selector: sel.clone(),
                                    decls: generics.clone(),
                                    layer,
                                    doc,
                                    bucket,
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
                Item::Property { name, inherits, initial } => {
                    self.properties.insert(name.clone(), (*inherits, initial.clone()));
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
        let layers = &self.layers;
        let to_rule = |c: PendingCustom| {
            let spec = c.selector.specificity() as u64;
            let mut priority =
                (rank_of(layers, c.layer, c.important) << RANK_SHIFT) | (spec << SPEC_SHIFT) | c.doc as u64;
            if c.important {
                priority |= IMPORTANT_BIT;
            }
            (c.name, CustomRule { selector: SelectorList::from_one(c.selector), value: c.value, priority })
        };
        rs.properties = std::mem::take(&mut self.properties);
        rs.has_relative = self.has_relative;
        rs.has_anchors = std::mem::take(&mut self.has_anchors);
        rs.revalidate = std::mem::take(&mut self.revalidate);
        rs.validity_keys = std::mem::take(&mut self.validity_keys);
        rs.sibling_keys = std::mem::take(&mut self.sibling_keys);
        rs.attr_names = std::mem::take(&mut self.attr_names);
        rs.structural_sibling_keys = std::mem::take(&mut self.structural_sibling_keys);
        rs.nth_keys = std::mem::take(&mut self.nth_keys);
        rs.empty_keys = std::mem::take(&mut self.empty_keys);
        rs.focus_keys = std::mem::take(&mut self.focus_keys);
        rs.within_keys = std::mem::take(&mut self.within_keys);
        rs.hover_keys = std::mem::take(&mut self.hover_keys);
        for (name, rule) in self.customs.into_iter().map(to_rule) {
            rs.customs.entry(name).or_default().push(rule);
        }
        for g in self.generics {
            let spec = (g.selector.specificity() as u64) << SPEC_SHIFT;
            let normal = (rank_of(layers, g.layer, false) << RANK_SHIFT) | spec | g.doc as u64;
            let important = (rank_of(layers, g.layer, true) << RANK_SHIFT) | spec | g.doc as u64 | IMPORTANT_BIT;
            let idx = rs.generic_rules.len() as u32;
            let ancestors = ancestor_hashes(&g.selector);
            rs.generic_rules.push(GenericRule {
                ancestors,
                selector: SelectorList::from_one(g.selector),
                decls: g.decls,
                normal,
                important,
            });
            match g.bucket {
                BucketKey::Id(k) => rs.generic_by_id.entry(k).or_default().push(idx),
                BucketKey::Class(k) => rs.generic_by_class.entry(k).or_default().push(idx),
                BucketKey::Tag(k) => rs.generic_by_tag.entry(k).or_default().push(idx),
                BucketKey::Other => rs.generic_other.push(idx),
            }
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

// Width/height retain pending var() values through the cascade, then resolve
// the supported whole-value form before memoizing numeric computed lengths.
fn is_variable_length(raw: &str) -> bool {
    raw.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("var("))
}

fn whole_variable(raw: &str) -> Option<(String, Option<String>)> {
    let mut parser = Parser::new(raw);
    parser.expect_function_matching("var").ok()?;
    let parsed: Result<_, ParseError<()>> = parser.parse_nested_block(|p| {
        let name = p.expect_ident_cloned()?.to_string();
        if !name.starts_with("--") || name.len() == 2 {
            return Err(ParseError::custom(()));
        }
        if p.is_exhausted() {
            return Ok((name, None));
        }
        p.expect_comma()?;
        let start = p.position();
        while let Ok(token) = p.next_including_whitespace_and_comments().cloned() {
            consume_block_of(p, &token);
        }
        Ok((name, Some(p.slice_from(start).trim().to_owned())))
    });
    let result = parsed.ok()?;
    parser.expect_exhausted().ok()?;
    Some(result)
}

/// Computed values of one property by (document, element, precise).
type InheritedByElement = FxHashMap<(usize, u32, bool), Option<String>>;

/// Memo of ancestor-derived values for one layout pass, during which the
/// DOM and style sheets cannot change (the pass holds `&Dom`). Keys include
/// the document's address: a thread may lay out several documents (frames),
/// whose node indices overlap.
#[derive(Default)]
struct PassMemo {
    custom: FxHashMap<(usize, u32, String), Option<(NodeId, String)>>,
    font_size: FxHashMap<(usize, u32), f64>,
    font_weight: FxHashMap<(usize, u32), f64>,
    /// Computed values of inherited properties by property, then (document,
    /// element, precise): an element without its own value takes its
    /// parent's, so each ancestor resolves once per pass.
    inherited: FxHashMap<String, InheritedByElement>,
    ancestor_bits: FxHashMap<(usize, u32), Option<Rc<AncestorBits>>>,
}

fn dom_key(dom: &Dom) -> usize {
    dom as *const Dom as usize
}

thread_local! {
    static PASS_MEMO: RefCell<Option<PassMemo>> = const { RefCell::new(None) };
}

/// Runs `f` with ancestor-derived style values memoized; used by layout
/// passes, which read the cascade for every element against a fixed DOM.
pub fn with_pass_memo<R>(f: impl FnOnce() -> R) -> R {
    let outer = PASS_MEMO.with(|m| m.borrow_mut().replace(PassMemo::default()));
    let r = f();
    PASS_MEMO.with(|m| *m.borrow_mut() = outer);
    r
}

/// Whether element `a` (unstyled) has the cascade of element `c`: see
/// `StyleEngine::shared_flow_style`. `c` is `a`'s previous element sibling
/// or a cousin at the same depth whose ancestors up to the common one share
/// their cascade pairwise (recorded when they shared).
fn shares_cascade(dom: &Dom, rules: &RuleSet, a: u32, c: u32) -> bool {
    let (Some(ea), Some(ec)) = (dom.n(a).element(), dom.n(c).element()) else { return false };
    if a == c || ea.name != ec.name || !ea.is_html() || !shareable_tag(&ea.name.local) {
        return false;
    }
    let ignorable = |attr: &crate::node::Attr| {
        let plain =
            attr.name.ns.is_empty() && (attr.name.local.starts_with("data-") || attr.name.local.starts_with("aria-"));
        plain && !rules.attr_names.contains(&attr.name.local.get_hash())
    };
    // The attributes that matter, sorted; None for an id or popover one.
    fn attrs<'e>(
        e: &'e crate::node::ElementData,
        ignorable: &dyn Fn(&crate::node::Attr) -> bool,
    ) -> Option<Vec<(&'e markup5ever::QualName, &'e str)>> {
        let mut v = Vec::with_capacity(e.attrs.len());
        for x in e.attrs.iter() {
            if x.name.ns.is_empty() && matches!(&*x.name.local, "id" | "popover") {
                return None;
            }
            if !ignorable(x) {
                v.push((&x.name, &*x.value));
            }
        }
        v.sort_by(|x, y| (&*x.0.local, &*x.0.ns).cmp(&(&*y.0.local, &*y.0.ns)));
        Some(v)
    }
    match (attrs(ea, &ignorable), attrs(ec, &ignorable)) {
        (Some(x), Some(y)) if x == y => {}
        _ => return false,
    }
    // Selectors that can tell them apart (position among siblings, earlier
    // siblings, contents via :has(), validity): those that can apply (same
    // classes and tag on both) must match both or neither. Compounds above
    // them test shared ancestors or pairs that shared.
    let classes: Vec<&str> = ea.attr("class").map(|v| v.split_ascii_whitespace().collect()).unwrap_or_default();
    for (selector, bucket) in &rules.revalidate {
        let applies = match bucket {
            BucketKey::Id(_) => false,
            BucketKey::Class(k) => classes.contains(&k.as_str()),
            BucketKey::Tag(t) => (*ea.name.local).eq_ignore_ascii_case(t),
            BucketKey::Other => true,
        };
        if applies
            && dom.matches(dom.id_of(a), selector).unwrap_or(true)
                != dom.matches(dom.id_of(c), selector).unwrap_or(false)
        {
            return false;
        }
    }
    // Hover and focus: `.group:hover .x` reads ancestors. Neither side, up
    // to the common ancestor, may hold the hovered or focused element.
    let doc = dom.n(a).owner_doc;
    let targets = [dom.hovered_idx(doc), dom.focused_idx(doc)];
    if targets.iter().any(|&t| t != NIL) {
        let (mut x, mut y) = (a, c);
        while x != y && x != NIL && y != NIL {
            for side in [x, y] {
                if targets.iter().any(|&t| t != NIL && dom.is_inclusive_ancestor(side, t)) {
                    return false;
                }
            }
            x = dom.n(x).parent;
            y = dom.n(y).parent;
        }
        if x != y {
            return false;
        }
    }
    true
}

/// Tags whose style can depend on state no attribute shows (form controls,
/// links, media, open/closed widgets): never shared.
fn shareable_tag(tag: &str) -> bool {
    !matches!(
        tag,
        "input"
            | "select"
            | "textarea"
            | "button"
            | "option"
            | "optgroup"
            | "label"
            | "fieldset"
            | "form"
            | "output"
            | "meter"
            | "progress"
            | "datalist"
            | "a"
            | "area"
            | "link"
            | "img"
            | "picture"
            | "video"
            | "audio"
            | "canvas"
            | "iframe"
            | "object"
            | "embed"
            | "svg"
            | "math"
            | "details"
            | "summary"
            | "dialog"
            | "slot"
            | "template"
            | "html"
            | "body"
            | "head"
    )
}

/// Whether a selector can match one of two elements and not the other
/// although they have the same attributes and inherited style: `:has()`,
/// structural pseudo-classes (`:nth-*`, `:first/last/only-*`, `:empty`),
/// sibling combinators, `:valid`/`:invalid`, anywhere in it.
fn needs_revalidation(sel: &Selector<Impl>) -> bool {
    sel.iter_raw_match_order().any(|c| match c {
        Component::Combinator(
            selectors::parser::Combinator::NextSibling | selectors::parser::Combinator::LaterSibling,
        ) => true,
        Component::Nth(_) | Component::NthOf(_) | Component::Empty | Component::Has(_) => true,
        Component::NonTSPseudoClass(pc) => matches!(pc, PseudoClass::Valid | PseudoClass::Invalid),
        Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
            list.slice().iter().any(needs_revalidation)
        }
        Component::Slotted(s) => needs_revalidation(s),
        Component::Host(Some(s)) => needs_revalidation(s),
        _ => false,
    })
}

/// Roots of the subtrees whose cached styles a set of DOM changes can
/// affect, or None when full invalidation is needed. An element's own
/// attributes or state affect it, its descendants (descendant/child
/// combinators, inheritance) and its later siblings with their subtrees
/// (sibling combinators); a child-list or text change affects the parent
/// the same way (:empty, :nth-child, new or removed nodes).
/// `tag#id.class` of an invalidation root (diagnostics).
fn describe_root(dom: &Dom, root: u32) -> String {
    match &dom.n(root).kind {
        crate::node::NodeKind::Element(e) => {
            let mut s = e.name.local.to_string();
            if let Some(id) = e.attr("id") {
                s.push('#');
                s.push_str(id);
            }
            if let Some(class) = e.attr("class").and_then(|c| c.split_ascii_whitespace().next()) {
                s.push('.');
                s.push_str(class);
            }
            s.chars().take(60).collect()
        }
        _ => "#node".to_owned(),
    }
}

/// Why selective invalidation gave up (diagnostics).
enum NotSelective {
    /// An unjournaled change (or one selection cannot bound), made at this
    /// location.
    Unknown(&'static std::panic::Location<'static>),
}

/// The focused and hovered elements before and after the changes (NIL:
/// none; equal when they did not move).
#[derive(Clone, Copy)]
struct PseudoMove {
    old_focus: u32,
    new_focus: u32,
    old_hover: u32,
    new_hover: u32,
}

/// The elements whose subtrees `changes` may restyle, or why every element
/// must be restyled.
///
/// A change can restyle the changed element and its subtree (inheritance,
/// descendant and child combinators). It can restyle later siblings and
/// their subtrees only through a sibling combinator: for state changes when
/// the element matches a state-dependent left compound before or after the
/// change (`sibling_keys`), for insertions and removals through any
/// (`structural_sibling_keys`: the adjacency changes). It can restyle the
/// parent's other children through structural pseudo-classes (`nth_keys`),
/// the parent itself through `:empty` (`empty_keys`), and ancestors through
/// `:has()` (`has_anchors`), `:valid`/`:invalid` of forms, fieldsets and
/// selects (`validity_keys`), and `:focus-within`/`:hover` when the focused
/// or hovered element moves with the subtree. A moved focus or hover
/// restyles the elements whose `:focus`, `:focus-within` or `:hover` match
/// changed: the old and new elements and the ancestors only one of them
/// has. Sibling-relative `:has()` and shadow trees never get here.
fn invalidation_roots(
    dom: &Dom,
    changes: &[crate::dom::Change],
    rules: &RuleSet,
    pseudo: PseudoMove,
) -> Result<Vec<u32>, NotSelective> {
    use crate::dom::Change;
    use crate::node::NodeKind;
    // Journal entries can name nodes that `collect_garbage` has freed since
    // (removed subtrees). A freed node needs no restyle (its cache entries
    // are keyed by its old NodeId and never served again); a slot reused by
    // a new node only over-invalidates.
    let live = |i: u32| i != NIL && dom.slot_live(i);
    let (anchors, validity) = (&rules.has_anchors[..], &rules.validity_keys[..]);
    let element = |x: u32| match &dom.n(x).kind {
        NodeKind::Element(e) => Some(&**e),
        _ => None,
    };
    // Keys are tested many times per element in a batch (each change, each
    // key): split an element's class list once.
    let class_cache: RefCell<FxHashMap<u32, Rc<[&str]>>> = RefCell::new(FxHashMap::default());
    let classes = |x: u32| -> Rc<[&str]> {
        if let Some(c) = class_cache.borrow().get(&x) {
            return c.clone();
        }
        let c: Rc<[&str]> =
            element(x).and_then(|e| e.attr("class")).map(|v| v.split_ascii_whitespace().collect()).unwrap_or_default();
        class_cache.borrow_mut().insert(x, c.clone());
        c
    };
    let key_at = |k: &AnchorKey, x: u32| -> bool {
        let Some(e) = element(x) else { return false };
        match k {
            AnchorKey::Class(c) => classes(x).iter().any(|t| t == c),
            _ => k.matches(e),
        }
    };
    let child_key_at = |k: &ChildKey, x: u32, p: u32| -> bool {
        key_at(&k.key, x) && k.parent.as_ref().is_none_or(|pk| p != NIL && key_at(pk, p))
    };
    let any_key = |keys: &[AnchorKey], x: u32| keys.iter().any(|k| key_at(k, x));
    // Whether a state change of element `x` (`attr`: the hash of a changed
    // attribute's name) can change its match of a sibling-combinator left
    // compound. Keys are necessary conditions: one that does not match now
    // matched before too unless the change touched the attribute it reads.
    let siblings_affected = |x: u32, attr: Option<u64>| -> bool {
        if element(x).is_none() {
            return true;
        }
        rules.sibling_keys.iter().any(|k| {
            key_at(k, x)
                || match (k, attr) {
                    (AnchorKey::Class(_), Some(h)) => h == local_name!("class").get_hash(),
                    (AnchorKey::Attr(name), Some(h)) => {
                        h == crate::dom::attr_name_hash(name)
                            || h == crate::dom::attr_name_hash(&name.to_ascii_lowercase())
                    }
                    _ => false,
                }
        })
    };
    let is_radio = |i: u32| {
        element(i).is_some_and(|e| e.is_html() && e.name.local == local_name!("input"))
            && dom.input_type_idx(i) == crate::forms::InputType::Radio
    };
    let radios_below = |x: u32| -> Vec<u32> {
        dom.descendants(dom.id_of(x)).into_iter().flatten().map(|n| n.idx).filter(|&n| is_radio(n)).collect()
    };
    // Ancestors from `first` up (`first` at `level`) that a change of an
    // element below may restyle: `:has()` anchors within their depth, and
    // forms, fieldsets and selects tested with `:valid`/`:invalid`.
    // Per batch: each element's anchor reach (the largest depth of the
    // anchors admitting it, u32::MAX for unbounded; None: no anchor) and
    // validity match, computed once (a batch walks the same ancestors from
    // many changes, and React forces a flush per metric read).
    let reach_cache: RefCell<FxHashMap<u32, (Option<u32>, bool)>> = RefCell::new(FxHashMap::default());
    let reach = |x: u32, e: &crate::node::ElementData| -> (Option<u32>, bool) {
        if let Some(&r) = reach_cache.borrow().get(&x) {
            return r;
        }
        let classes: Vec<&str> = e.attr("class").map(|c| c.split_ascii_whitespace().collect()).unwrap_or_default();
        let key_matches = |k: &AnchorKey| match k {
            AnchorKey::Class(c) => classes.iter().any(|t| t == c),
            _ => k.matches(e),
        };
        let anchor = anchors
            .iter()
            .filter(|a| key_matches(&a.key) && a.admits_context(dom, x))
            .map(|a| a.depth.unwrap_or(u32::MAX))
            .max();
        let validated = !validity.is_empty()
            && e.is_html()
            && matches!(&*e.name.local, "form" | "fieldset" | "select")
            && validity.iter().any(key_matches);
        reach_cache.borrow_mut().insert(x, (anchor, validated));
        (anchor, validated)
    };
    // The lowest level each ancestor was reached at in this batch: a walk that
    // reaches it again at that level or higher stops (depth bounds only admit
    // more at lower levels, validity does not depend on the level, so every
    // ancestor above was already examined at least as favourably).
    let walked: RefCell<FxHashMap<u32, u32>> = RefCell::new(FxHashMap::default());
    let walk_up = |first: u32, level: u32| -> Vec<u32> {
        let mut out = Vec::new();
        if anchors.is_empty() && validity.is_empty() {
            return out;
        }
        let mut level = level;
        let mut cur = first;
        while cur != NIL {
            let Some(e) = element(cur) else { break };
            {
                let mut walked = walked.borrow_mut();
                match walked.get(&cur) {
                    Some(&seen) if seen <= level => break,
                    _ => {
                        walked.insert(cur, level);
                    }
                }
            }
            let (anchor, validated) = reach(cur, e);
            if anchor.is_some_and(|d| level <= d) || validated {
                out.push(cur);
            }
            cur = dom.n(cur).parent;
            level += 1;
        }
        out
    };
    // A radio's validity depends on its whole group: the members and what
    // their validity restyles above them.
    let radio_roots = |radios: Vec<u32>| -> Vec<u32> {
        let mut out = Vec::new();
        for m in radios {
            out.push(m);
            out.extend(walk_up(dom.n(m).parent, 1));
        }
        out
    };
    let inclusive_ancestors = |x: u32| -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = x;
        while cur != NIL {
            out.push(cur);
            cur = dom.n(cur).parent;
        }
        out
    };
    // The focused and hovered elements, before and after: moving a subtree
    // that holds one changes :focus-within/:hover of the ancestors it left
    // and joined.
    let mut targets: Vec<u32> = [pseudo.old_focus, pseudo.new_focus, pseudo.old_hover, pseudo.new_hover]
        .into_iter()
        .filter(|&t| live(t))
        .collect();
    targets.dedup();
    let holds_target = |n: u32| targets.iter().any(|&t| inclusive_ancestors(t).contains(&n));
    let pseudo_ancestors = |p: u32| -> Vec<u32> {
        inclusive_ancestors(p)
            .into_iter()
            .filter(|&a| any_key(&rules.within_keys, a) || any_key(&rules.hover_keys, a))
            .collect()
    };
    // The parent side of an insertion or removal: its own :empty, the
    // :has()/validity ancestors (from the parent itself, whose children
    // changed), and its children's structural pseudo-classes.
    let parent_side = |p: u32| -> Vec<u32> {
        let mut out = walk_up(p, 0);
        if any_key(&rules.empty_keys, p) {
            out.push(p);
        }
        if !rules.nth_keys.is_empty() {
            let mut child = dom.n(p).first_child;
            while child != NIL {
                if element(child).is_some() && rules.nth_keys.iter().any(|k| child_key_at(k, child, p)) {
                    out.push(child);
                }
                child = dom.n(child).next;
            }
        }
        out
    };
    // Whether inserting or removing next to `x` (the node itself, or its
    // previous sibling now) can change which later siblings a sibling
    // combinator joins.
    let adjacency = |x: u32, p: u32| {
        x != NIL
            && (!live(x)
                || (element(x).is_some() && rules.structural_sibling_keys.iter().any(|k| child_key_at(k, x, p))))
    };

    // (root, with later siblings)
    let mut found: Vec<(u32, bool)> = Vec::new();
    // Parents whose side was taken: it depends only on the current tree,
    // so a parent of many changes in a batch (a list or row React fills
    // child by child) is examined once instead of per change, each time
    // over all of its children.
    let mut sided: FxHashSet<u32> = FxHashSet::default();
    let state = |x: u32| (x, siblings_affected(x, None));
    for c in changes {
        match *c {
            Change::Pseudo => {}
            Change::Unknown(at) => return Err(NotSelective::Unknown(at)),
            Change::Attr(i, h, plain) => {
                // An unreferenced data-*/aria-* attribute restyles nothing.
                if !live(i) || (plain && !rules.attr_names.contains(&h)) {
                    continue;
                }
                found.extend(walk_up(dom.n(i).parent, 1).into_iter().map(state));
                // `type`, `name`, `form`, `checked` and `required` regroup or
                // revalidate radio groups (also when `type` leaves radio).
                let input = element(i).is_some_and(|e| e.is_html() && e.name.local == local_name!("input"));
                if !validity.is_empty() && input && (is_radio(i) || h == local_name!("type").get_hash()) {
                    found.extend(radio_roots(radios_below(dom.root_idx(i))).into_iter().map(state));
                }
                found.push((i, siblings_affected(i, Some(h))));
            }
            Change::State(i) => {
                if !live(i) {
                    continue;
                }
                found.extend(walk_up(dom.n(i).parent, 1).into_iter().map(state));
                if !validity.is_empty() && is_radio(i) {
                    found.extend(radio_roots(dom.radio_group(i)).into_iter().map(state));
                }
                found.push(state(i));
            }
            Change::Inserted(p, n) => {
                if !live(p) {
                    continue;
                }
                if sided.insert(p) {
                    found.extend(parent_side(p).into_iter().map(state));
                }
                // Moved on since: its later entries cover where it went.
                if !live(n) || dom.n(n).parent != p {
                    continue;
                }
                if !validity.is_empty() {
                    // Inserted radios join (and may complete) a group.
                    for r in radios_below(n) {
                        found.extend(radio_roots(dom.radio_group(r)).into_iter().map(state));
                    }
                }
                if holds_target(n) {
                    found.extend(pseudo_ancestors(p).into_iter().map(state));
                }
                let siblings = adjacency(n, p) || adjacency(dom.n(n).prev, p) || siblings_affected(n, None);
                found.push((n, siblings));
            }
            Change::Removed(p, n, next) => {
                if !live(p) {
                    continue;
                }
                if sided.insert(p) {
                    found.extend(parent_side(p).into_iter().map(state));
                }
                let mut siblings = live(next) && (adjacency(n, p) || adjacency(dom.n(next).prev, p));
                if live(n) {
                    // Removed radios leave a group the detached subtree no
                    // longer reaches: every radio left in the tree.
                    if !validity.is_empty() && !radios_below(n).is_empty() {
                        found.extend(radio_roots(radios_below(dom.root_idx(p))).into_iter().map(state));
                    }
                    if holds_target(n) {
                        found.extend(pseudo_ancestors(p).into_iter().map(state));
                    }
                    siblings |= siblings_affected(n, None);
                    // The removed subtree itself (styles of detached elements
                    // are still computed).
                    found.push((n, false));
                } else if !validity.is_empty() {
                    // Freed: whether it held radios is unknown.
                    found.extend(radio_roots(radios_below(dom.root_idx(p))).into_iter().map(state));
                }
                if siblings && live(next) && dom.n(next).parent == p {
                    found.push((next, true));
                }
            }
            Change::Text(t) => {
                if !live(t) {
                    continue;
                }
                let p = dom.n(t).parent;
                if p == NIL {
                    continue;
                }
                found.extend(walk_up(p, 0).into_iter().map(state));
                if element(p).is_some() {
                    found.push(state(p));
                }
            }
        }
    }
    // Focus and hover that moved.
    let changed_chain = |old: u32, new: u32| -> Vec<u32> {
        if old == new {
            return Vec::new();
        }
        let a = if live(old) { inclusive_ancestors(old) } else { Vec::new() };
        let b = if live(new) { inclusive_ancestors(new) } else { Vec::new() };
        a.iter().filter(|x| !b.contains(x)).chain(b.iter().filter(|x| !a.contains(x))).copied().collect()
    };
    let mut pseudo_changed: Vec<u32> = Vec::new();
    if pseudo.old_focus != pseudo.new_focus {
        for x in [pseudo.old_focus, pseudo.new_focus] {
            if live(x) {
                pseudo_changed.push(x);
                if any_key(&rules.focus_keys, x) {
                    found.push(state(x));
                }
            }
        }
        for x in changed_chain(pseudo.old_focus, pseudo.new_focus) {
            pseudo_changed.push(x);
            if any_key(&rules.within_keys, x) {
                found.push(state(x));
            }
        }
    }
    for x in changed_chain(pseudo.old_hover, pseudo.new_hover) {
        pseudo_changed.push(x);
        if any_key(&rules.hover_keys, x) {
            found.push(state(x));
        }
    }
    // :has() anchors whose argument sees those states.
    pseudo_changed.sort_unstable();
    pseudo_changed.dedup();
    for x in pseudo_changed {
        found.extend(walk_up(dom.n(x).parent, 1).into_iter().map(state));
    }

    let mut roots: Vec<u32> = Vec::new();
    let mut seen = FxHashSet::default();
    // Nodes whose later siblings are already roots too: a walk can stop
    // there.
    let mut tails = FxHashSet::default();
    for (x, with_siblings) in found {
        if !with_siblings {
            if seen.insert(x) {
                roots.push(x);
            }
            continue;
        }
        let mut cur = x;
        while cur != NIL && tails.insert(cur) {
            if seen.insert(cur) {
                roots.push(cur);
            }
            cur = dom.n(cur).next;
        }
    }
    // A root inside another root's subtree adds nothing.
    let set: FxHashSet<u32> = roots.iter().copied().collect();
    roots.retain(|&r| {
        let mut cur = dom.n(r).parent;
        while cur != NIL {
            if set.contains(&cur) {
                return false;
            }
            cur = dom.n(cur).parent;
        }
        true
    });
    Ok(roots)
}

/// Keys of the compounds of `sel` containing a component `hit` accepts,
/// also through `:is()`/`:where()`/`:not()` (an inner subject compound
/// constrains the same element: the outer key). `:has()` arguments are
/// skipped: they restyle the anchor, which the anchor walk covers.
fn collect_keys(sel: &Selector<Impl>, hit: &dyn Fn(&Component<Impl>) -> bool, out: &mut Vec<AnchorKey>) {
    fn subject_hits(sel: &Selector<Impl>, hit: &dyn Fn(&Component<Impl>) -> bool) -> bool {
        sel.iter().any(|c| match c {
            Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                hit(c) || list.slice().iter().any(|inner| subject_hits(inner, hit))
            }
            _ => hit(c),
        })
    }
    fn walk(
        sel: &Selector<Impl>,
        hit: &dyn Fn(&Component<Impl>) -> bool,
        out: &mut Vec<AnchorKey>,
        skip_subject: bool,
    ) {
        let mut it = sel.iter();
        let mut subject = true;
        loop {
            let compound: Vec<&Component<Impl>> = (&mut it).collect();
            let mut tests = false;
            for c in &compound {
                if hit(c) {
                    tests = true;
                }
                if let Component::Is(list) | Component::Where(list) | Component::Negation(list) = c {
                    for inner in list.slice() {
                        tests |= subject_hits(inner, hit);
                        walk(inner, hit, out, true);
                    }
                }
            }
            if tests && !(skip_subject && subject) {
                let key = compound_key(&compound);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
            subject = false;
            if it.next_sequence().is_none() {
                return;
            }
        }
    }
    walk(sel, hit, out, false)
}

/// The key of the compound after `it`'s current one when they are joined by
/// a child combinator (the parent's compound).
fn parent_key(it: &selectors::parser::SelectorIter<'_, Impl>) -> Option<AnchorKey> {
    let mut peek = it.clone();
    if !matches!(peek.next_sequence(), Some(selectors::parser::Combinator::Child)) {
        return None;
    }
    let parent: Vec<&Component<Impl>> = (&mut peek).collect();
    Some(compound_key(&parent)).filter(|k| *k != AnchorKey::Any)
}

/// Keys (with the parent's, through `>`) of compounds testing structural
/// pseudo-classes, also through `:is()`/`:where()`/`:not()`. A compound that
/// is only an `:is()`/`:where()` stands for its inner selectors' subjects
/// (TOS `:where(.space-y-4>:not(:last-child))`); other inner subjects are
/// the outer element, keyed by the outer compound.
fn collect_nth_keys(sel: &Selector<Impl>, out: &mut Vec<ChildKey>, skip_subject: bool) {
    fn hit(c: &Component<Impl>) -> bool {
        match c {
            Component::Nth(_) | Component::NthOf(_) => true,
            Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                list.slice().iter().any(|inner| inner.iter().any(hit))
            }
            _ => false,
        }
    }
    let mut it = sel.iter();
    let mut subject = true;
    loop {
        let compound: Vec<&Component<Impl>> = (&mut it).collect();
        let wrapper = match compound.as_slice() {
            [Component::Is(list) | Component::Where(list)] => Some(list),
            _ => None,
        };
        if let Some(list) = wrapper {
            for inner in list.slice() {
                collect_nth_keys(inner, out, skip_subject && subject);
            }
        } else {
            if compound.iter().any(|c| hit(c)) && !(skip_subject && subject) {
                let entry = ChildKey { key: compound_key(&compound), parent: parent_key(&it) };
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
            for c in &compound {
                if let Component::Is(list) | Component::Where(list) | Component::Negation(list) = c {
                    for inner in list.slice() {
                        collect_nth_keys(inner, out, true);
                    }
                }
            }
        }
        subject = false;
        if it.next_sequence().is_none() {
            return;
        }
    }
}

/// Every attribute name `sel` tests, at any nesting (hashes of the name as
/// written and lowercased).
fn collect_attr_names(sel: &Selector<Impl>, out: &mut FxHashSet<u64>) {
    fn add(out: &mut FxHashSet<u64>, name: &str) {
        out.insert(crate::dom::attr_name_hash(name));
        out.insert(crate::dom::attr_name_hash(&name.to_ascii_lowercase()));
    }
    for c in sel.iter_raw_match_order() {
        match c {
            Component::AttributeInNoNamespaceExists { local_name, local_name_lower } => {
                add(out, &local_name.0);
                add(out, &local_name_lower.0);
            }
            Component::AttributeInNoNamespace { local_name, .. } => add(out, &local_name.0),
            Component::AttributeOther(a) => {
                add(out, &a.local_name.0);
                add(out, &a.local_name_lower.0);
            }
            Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                for inner in list.slice() {
                    collect_attr_names(inner, out);
                }
            }
            Component::Has(list) => {
                for rel in list.iter() {
                    collect_attr_names(&rel.selector, out);
                }
            }
            Component::NthOf(data) => {
                for inner in data.selectors() {
                    collect_attr_names(inner, out);
                }
            }
            Component::Slotted(inner) | Component::Host(Some(inner)) => collect_attr_names(inner, out),
            _ => {}
        }
    }
}

/// Keys of the state-dependent compounds left of a sibling combinator
/// (`+`/`~`), also inside `:is()`/`:where()`/`:not()`; `:nth-child(of S)`
/// makes every element's state matter to its siblings (key Any). A compound
/// of type selectors only cannot change its match with attributes or state,
/// but insertions and removals change adjacency for every left compound
/// (`structural`).
fn collect_sibling_keys(sel: &Selector<Impl>, out: &mut Vec<AnchorKey>, structural: &mut Vec<ChildKey>) {
    use selectors::parser::Combinator;
    let mut it = sel.iter();
    let mut left_of_sibling = false;
    loop {
        let compound: Vec<&Component<Impl>> = (&mut it).collect();
        let state_dependent = compound.iter().any(|c| {
            !matches!(
                c,
                Component::LocalName(_)
                    | Component::ExplicitUniversalType
                    | Component::ExplicitAnyNamespace
                    | Component::ExplicitNoNamespace
                    | Component::DefaultNamespace(_)
                    | Component::Namespace(..)
            )
        });
        if left_of_sibling {
            let key = compound_key(&compound);
            if state_dependent && !out.contains(&key) {
                out.push(key.clone());
            }
            let entry = ChildKey { key, parent: parent_key(&it) };
            if !structural.contains(&entry) {
                structural.push(entry);
            }
        }
        for c in &compound {
            match c {
                Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                    for inner in list.slice() {
                        collect_sibling_keys(inner, out, structural);
                    }
                }
                Component::NthOf(_) if !out.contains(&AnchorKey::Any) => out.push(AnchorKey::Any),
                _ => {}
            }
        }
        match it.next_sequence() {
            Some(Combinator::NextSibling | Combinator::LaterSibling) => left_of_sibling = true,
            Some(_) => left_of_sibling = false,
            None => return,
        }
    }
}

/// Keys of the compounds of `sel` that test `:valid`/`:invalid` (also via
/// `:is()`/`:where()`/`:not()`). False when one sits inside `:has()`: a
/// form's validity changes with controls below it, beyond what the anchor
/// depth bounds, so every change invalidates everything.
fn collect_validity_keys(sel: &Selector<Impl>, out: &mut Vec<AnchorKey>) -> bool {
    fn mentions(sel: &Selector<Impl>) -> bool {
        sel.iter_raw_match_order().any(|c| match c {
            Component::NonTSPseudoClass(PseudoClass::Valid | PseudoClass::Invalid) => true,
            Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                list.slice().iter().any(mentions)
            }
            Component::Has(list) => list.iter().any(|rel| mentions(&rel.selector)),
            _ => false,
        })
    }
    let mut it = sel.iter();
    loop {
        let compound: Vec<&Component<Impl>> = (&mut it).collect();
        let mut tests = false;
        for c in &compound {
            match c {
                Component::NonTSPseudoClass(PseudoClass::Valid | PseudoClass::Invalid) => tests = true,
                Component::Has(list) => {
                    if list.iter().any(|rel| mentions(&rel.selector)) {
                        return false;
                    }
                }
                Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                    for inner in list.slice() {
                        // The inner subject compound constrains this element
                        // (keyed by the outer compound; the recursion also
                        // records the inner key, which may be broader).
                        if inner.iter().any(|c| {
                            matches!(c, Component::NonTSPseudoClass(PseudoClass::Valid | PseudoClass::Invalid))
                        }) {
                            tests = true;
                        }
                        if !collect_validity_keys(inner, out) {
                            return false;
                        }
                    }
                }
                _ => {}
            }
        }
        if tests {
            let key = compound_key(&compound);
            if !out.contains(&key) {
                out.push(key);
            }
        }
        if it.next_sequence().is_none() {
            return true;
        }
    }
}

/// What a `:has()` anchor compound requires (a necessary condition).
#[derive(Clone, Debug, PartialEq)]
enum AnchorKey {
    Class(String),
    Tag(String),
    Attr(String),
    Any,
}

impl AnchorKey {
    fn matches(&self, e: &crate::node::ElementData) -> bool {
        match self {
            AnchorKey::Class(c) => e.attr("class").is_some_and(|v| v.split_ascii_whitespace().any(|t| t == c)),
            AnchorKey::Tag(t) => (*e.name.local).eq_ignore_ascii_case(t),
            AnchorKey::Attr(a) => e.attrs.iter().any(|x| (*x.name.local).eq_ignore_ascii_case(a)),
            AnchorKey::Any => true,
        }
    }
}

/// A compound key, and the key of the parent compound when the compound is
/// a child (`>`) of one: both necessary conditions.
#[derive(Clone, Debug, PartialEq)]
struct ChildKey {
    key: AnchorKey,
    parent: Option<AnchorKey>,
}

/// A potential `:has()` anchor: elements matching `key` whose descendants
/// within `depth` levels (None: any depth) can change its match. `context`:
/// the compound before the anchor's, through a child (`true`: the parent
/// must match) or descendant combinator (an ancestor must match).
#[derive(Clone, Debug, PartialEq)]
struct HasAnchor {
    key: AnchorKey,
    depth: Option<u32>,
    context: Option<(AnchorKey, bool)>,
}

impl HasAnchor {
    /// Whether element `x`'s ancestors satisfy the context in the current
    /// tree (the key is checked separately). A change below `x` cannot change
    /// its ancestors; a change of them in the same batch roots their subtree,
    /// `x` included.
    fn admits_context(&self, dom: &Dom, x: u32) -> bool {
        let Some((key, parent_only)) = &self.context else { return true };
        let mut cur = dom.n(x).parent;
        while cur != NIL {
            let crate::node::NodeKind::Element(a) = &dom.n(cur).kind else { return false };
            if key.matches(a) {
                return true;
            }
            if *parent_only {
                return false;
            }
            cur = dom.n(cur).parent;
        }
        false
    }
}

/// Collects the `:has()` anchors of a selector (recursing into `:is()`,
/// `:where()` and `:not()`); false when one cannot be bounded: a
/// sibling-relative argument, or a universal anchor at unlimited depth.
fn collect_has_anchors(sel: &Selector<Impl>, out: &mut Vec<HasAnchor>) -> bool {
    use selectors::parser::RelativeSelectorMatchHint as Hint;
    let mut it = sel.iter();
    loop {
        let compound: Vec<&Component<Impl>> = (&mut it).collect();
        for c in &compound {
            match c {
                Component::Has(list) => {
                    for rel in list.iter() {
                        if !matches!(rel.match_hint, Hint::InChild | Hint::InSubtree) {
                            return false;
                        }
                        // Child-only chains bound the depth.
                        let mut children = 0u32;
                        let mut bounded = true;
                        let mut ri = rel.selector.iter();
                        loop {
                            for _ in &mut ri {}
                            match ri.next_sequence() {
                                Some(selectors::parser::Combinator::Child) => children += 1,
                                Some(_) => bounded = false,
                                None => break,
                            }
                        }
                        let depth = (bounded && children > 0).then_some(children);
                        let key = compound_key(&compound);
                        if key == AnchorKey::Any && depth.is_none() {
                            return false;
                        }
                        let mut peek = it.clone();
                        let parent_only = match peek.next_sequence() {
                            Some(selectors::parser::Combinator::Child) => Some(true),
                            Some(selectors::parser::Combinator::Descendant) => Some(false),
                            _ => None,
                        };
                        let context = parent_only.and_then(|parent_only| {
                            let before: Vec<&Component<Impl>> = (&mut peek).collect();
                            Some((compound_key(&before), parent_only)).filter(|(k, _)| *k != AnchorKey::Any)
                        });
                        out.push(HasAnchor { key, depth, context });
                    }
                }
                Component::Is(list) | Component::Where(list) | Component::Negation(list) => {
                    for inner in list.slice() {
                        if !collect_has_anchors(inner, out) {
                            return false;
                        }
                    }
                }
                _ => {}
            }
        }
        if it.next_sequence().is_none() {
            return true;
        }
    }
}

/// A necessary condition for an element to match a compound: a class, else
/// the tag, else an attribute name, else any element.
fn compound_key(compound: &[&Component<Impl>]) -> AnchorKey {
    for c in compound {
        if let Component::Class(name) = c {
            return AnchorKey::Class(name.0.to_string());
        }
    }
    for c in compound {
        if let Component::LocalName(l) = c {
            return AnchorKey::Tag(l.lower_name.0.to_string());
        }
    }
    for c in compound {
        match c {
            Component::AttributeInNoNamespaceExists { local_name, .. } => {
                return AnchorKey::Attr(local_name.0.to_string());
            }
            Component::AttributeInNoNamespace { local_name, .. } => return AnchorKey::Attr(local_name.0.to_string()),
            _ => {}
        }
    }
    // `:where(.group)` / `:is(.x)` with one single-compound selector: its key.
    for c in compound {
        if let Component::Is(list) | Component::Where(list) = c {
            if let [inner] = list.slice() {
                let mut it = inner.iter();
                let parts: Vec<&Component<Impl>> = (&mut it).collect();
                if it.next_sequence().is_none() {
                    let key = compound_key(&parts);
                    if key != AnchorKey::Any {
                        return key;
                    }
                }
            }
        }
    }
    AnchorKey::Any
}

// Return the defining element too: substitution occurs there, before the value
// is inherited, so a descendant's custom-property override cannot alter it.
fn custom_property_source(dom: &Dom, rules: &RuleSet, el: NodeId, name: &str) -> Option<(NodeId, String)> {
    // A registered property that does not inherit takes the element's own
    // value or its initial value (Tailwind registers its --tw-* variables
    // with `inherits: false`).
    if let Some((inherits, initial)) = rules.properties.get(name) {
        let found = if *inherits {
            custom_property_inherited(dom, rules, el, name)
        } else {
            dom.check(el).ok().and_then(|idx| own_custom_property(dom, rules, idx, name))
        };
        return found.or_else(|| initial.as_ref().map(|v| (el, v.to_string())));
    }
    custom_property_inherited(dom, rules, el, name)
}

fn custom_property_inherited(dom: &Dom, rules: &RuleSet, el: NodeId, name: &str) -> Option<(NodeId, String)> {
    let active = PASS_MEMO.with(|m| m.borrow().is_some());
    if !active {
        return custom_property_source_walk(dom, rules, el, name);
    }
    // Walk up until a memoized ancestor or a declaring element, then record
    // the answer for every element passed (they inherit it).
    let mut visited = Vec::new();
    let mut cur = dom.check(el).ok();
    let mut found = None;
    while let Some(idx) = cur {
        if let Some(hit) = PASS_MEMO
            .with(|m| m.borrow().as_ref().and_then(|m| m.custom.get(&(dom_key(dom), idx, name.to_owned())).cloned()))
        {
            found = hit;
            break;
        }
        visited.push(idx);
        if let Some(v) = own_custom_property(dom, rules, idx, name) {
            found = Some(v);
            break;
        }
        cur = flat_tree_parent_element(dom, idx);
    }
    PASS_MEMO.with(|m| {
        if let Some(m) = m.borrow_mut().as_mut() {
            for idx in visited {
                m.custom.insert((dom_key(dom), idx, name.to_owned()), found.clone());
            }
        }
    });
    found
}

/// The custom property declared on the element itself (not inherited).
fn own_custom_property(dom: &Dom, rules: &RuleSet, idx: u32, name: &str) -> Option<(NodeId, String)> {
    let id = dom.id_of(idx);
    let mut value = None;
    let mut priority = 0;
    if let Some(style) = dom.n(idx).element().and_then(|e| e.attr("style")) {
        if let Some((raw, important)) = InlineStyle::parse(style).get(name) {
            value = Some(raw);
            priority = if important { INLINE_IMPORTANT_PRIORITY } else { INLINE_PRIORITY };
        }
    }
    for rule in rules.customs.get(name).into_iter().flatten() {
        if rule.priority > priority && dom.matches(id, &rule.selector).unwrap_or(false) {
            value = Some(rule.value.to_string());
            priority = rule.priority;
        }
    }
    value.filter(|v| !v.eq_ignore_ascii_case("inherit") && !v.eq_ignore_ascii_case("unset")).map(|v| (id, v))
}

fn custom_property_source_walk(dom: &Dom, rules: &RuleSet, el: NodeId, name: &str) -> Option<(NodeId, String)> {
    let mut cur = dom.check(el).ok();
    while let Some(idx) = cur {
        let id = dom.id_of(idx);
        let mut value = None;
        let mut priority = 0;
        if let Some(style) = dom.n(idx).element().and_then(|e| e.attr("style")) {
            if let Some((raw, important)) = InlineStyle::parse(style).get(name) {
                value = Some(raw);
                priority = if important { INLINE_IMPORTANT_PRIORITY } else { INLINE_PRIORITY };
            }
        }
        for rule in rules.customs.get(name).into_iter().flatten() {
            if rule.priority > priority && dom.matches(id, &rule.selector).unwrap_or(false) {
                value = Some(rule.value.to_string());
                priority = rule.priority;
            }
        }
        if let Some(value) = value {
            if !value.eq_ignore_ascii_case("inherit") && !value.eq_ignore_ascii_case("unset") {
                return Some((id, value));
            }
        }
        cur = flat_tree_parent_element(dom, idx);
    }
    None
}

struct DimensionVariables<'a> {
    dom: &'a Dom,
    rules: &'a RuleSet,
    active: Vec<(NodeId, String)>,
    cyclic: FxHashSet<(NodeId, String)>,
    remaining: u16,
}
impl DimensionVariables<'_> {
    fn value(&mut self, el: NodeId, raw: &str, depth: u8) -> Option<String> {
        if depth >= 32 || raw.len() > 4096 || self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let raw = raw.trim();
        if !is_variable_length(raw) {
            return Some(raw.to_owned());
        }
        let (name, fallback) = whole_variable(raw)?;
        let value = self.custom(el, &name, depth + 1);
        // Dependencies in fallbacks participate in cycles even when the first
        // argument exists. Evaluate both bounded branches before choosing.
        let fallback = fallback.and_then(|raw| self.value(el, &raw, depth + 1));
        value.or(fallback)
    }

    fn custom(&mut self, el: NodeId, name: &str, depth: u8) -> Option<String> {
        let (source, raw) = custom_property_source(self.dom, self.rules, el, name)?;
        if raw.eq_ignore_ascii_case("initial") {
            return None;
        }
        let key = (source, name.to_owned());
        if let Some(at) = self.active.iter().position(|k| k == &key) {
            self.cyclic.extend(self.active[at..].iter().cloned());
            return None;
        }
        self.active.push(key.clone());
        let result = self.value(source, &raw, depth);
        self.active.pop();
        if self.cyclic.contains(&key) {
            None
        } else {
            result
        }
    }
}

fn resolve_dimension_variable(dom: &Dom, rules: &RuleSet, el: NodeId, raw: &str) -> Length {
    DimensionVariables { dom, rules, active: Vec::new(), cyclic: FxHashSet::default(), remaining: 128 }
        .value(el, raw, 0)
        .map(|s| Length::parse(&s))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Computed values
// ---------------------------------------------------------------------------

/// An element's own cascaded values (upstream `Cascaded`).
#[derive(Clone, Debug, Default)]
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
    width_variable: Option<Rc<str>>,
    height_variable: Option<Rc<str>>,
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
    if let Some(v) = d.width.filter(|_| p > pri[F_WIDTH]) {
        c.width = v;
        c.width_variable = d.width_variable.clone();
        pri[F_WIDTH] = p;
    }
    if let Some(v) = d.height.filter(|_| p > pri[F_HEIGHT]) {
        c.height = v;
        c.height_variable = d.height_variable.clone();
        pri[F_HEIGHT] = p;
    }
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
            inert: parent.inert,
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
    /// Winning generic declarations per element (`declared_map`), dropped with `memo`.
    declared: FxHashMap<NodeId, Rc<FxHashMap<String, String>>>,
    /// Flow layout inputs per element (`FlowStyleSource`), dropped with `memo`.
    flow: FxHashMap<NodeId, FlowEntry>,
    /// Stamps `flow` entries (a recomputed element gets a new one).
    flow_seq: u64,
    /// Elements whose cached styles were dropped since layout last asked
    /// (`take_restyled`); None: all of them.
    restyled: Option<Vec<NodeId>>,
    extent: Option<(f64, f64)>,
    /// The focused and hovered elements of `stamp` (indices there), to tell
    /// a freed one from a live one.
    pseudo_ids: (Option<NodeId>, Option<NodeId>),
}

/// A cached flow style. `stamp` tells a recomputed entry from the one an
/// element shared from; `shared_from` is (source element, its stamp then)
/// when the style was shared rather than computed.
struct FlowEntry {
    style: Option<crate::flow::FlowStyle>,
    stamp: u64,
    shared_from: Option<(NodeId, u64)>,
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
        let mut rules_rebuilt = false;
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
                rules_rebuilt = true;
            }
        }
        // Selective invalidation when only the DOM changed and the journal
        // covers it; anything uncertain (rules, generation, focus/hover,
        // :has(), shadow trees, unknown changes) invalidates every element.
        use crate::layout::flow_stats::{note_full, Full};
        let changes = match cache.stamp {
            Some(_) if rules_rebuilt => {
                note_full(Full::Rules);
                None
            }
            Some(old) if old.generation != stamp.generation => {
                note_full(Full::Stamp);
                None
            }
            // A previously focused/hovered element freed (or its slot reused)
            // since: where it was is unknown.
            Some(old)
                if (old.focused != stamp.focused && !cache.pseudo_ids.0.is_none_or(|id| dom.check(id).is_ok()))
                    || (old.hovered != stamp.hovered && !cache.pseudo_ids.1.is_none_or(|id| dom.check(id).is_ok())) =>
            {
                note_full(Full::Stamp);
                None
            }
            Some(_) if cache.rules.has_relative => {
                note_full(Full::Relative);
                None
            }
            Some(_) if dom.shadow_root_count() != 0 => {
                note_full(Full::ShadowRoots);
                None
            }
            Some(old) => {
                let changes = dom.changes_since(old.version).map(|c| c.collect::<Vec<_>>());
                if changes.is_none() {
                    note_full(Full::Journal);
                }
                changes
            }
            None => None,
        };
        let pseudo = cache.stamp.map(|old| PseudoMove {
            old_focus: old.focused,
            new_focus: stamp.focused,
            old_hover: old.hovered,
            new_hover: stamp.hovered,
        });
        let roots = changes.and_then(|c| match invalidation_roots(dom, &c, &cache.rules, pseudo?) {
            Ok(roots) => Some(roots),
            Err(why) => {
                note_full(Full::Unknown);
                crate::layout::flow_stats::note_unknown(|| match why {
                    NotSelective::Unknown(at) => {
                        let file = at.file().rsplit('/').next().unwrap_or(at.file());
                        format!("{file}:{}", at.line())
                    }
                });
                None
            }
        });
        match roots {
            Some(roots) => {
                let mut stale = Vec::new();
                for root in roots {
                    if let Ok(it) = dom.descendants(dom.id_of(root)) {
                        let before = stale.len();
                        stale.extend(it);
                        crate::layout::flow_stats::note_root(stale.len() - before, || describe_root(dom, root));
                    }
                }
                if stale.len() * 2 > cache.memo.len().max(cache.declared.len()).max(cache.flow.len()) {
                    note_full(Full::Large);
                    cache.memo.clear();
                    cache.declared.clear();
                    cache.flow.clear();
                    cache.restyled = None;
                } else {
                    crate::layout::flow_stats::note_selective(stale.len());
                    if let Some(restyled) = cache.restyled.as_mut() {
                        restyled.extend_from_slice(&stale);
                    }
                    for n in stale {
                        cache.memo.remove(&n);
                        cache.declared.remove(&n);
                        cache.flow.remove(&n);
                    }
                }
            }
            None => {
                cache.memo.clear();
                cache.declared.clear();
                cache.flow.clear();
                cache.restyled = None;
            }
        }
        cache.extent = None;
        cache.stamp = Some(stamp);
        cache.pseudo_ids = (
            (stamp.focused != NIL).then(|| dom.id_of(stamp.focused)),
            (stamp.hovered != NIL).then(|| dom.id_of(stamp.hovered)),
        );
    }

    /// The elements whose styles were dropped since the last call (None:
    /// all), after bringing the cache up to date with `dom`. Layout drops
    /// their subtree measurements.
    pub(crate) fn take_restyled(&self, dom: &Dom) -> Option<Vec<NodeId>> {
        self.ensure(dom);
        self.cache.borrow_mut().restyled.replace(Vec::new())
    }

    pub(crate) fn cached_flow_style(&self, dom: &Dom, el: NodeId) -> Option<Option<crate::flow::FlowStyle>> {
        self.ensure(dom);
        self.cache.borrow().flow.get(&el).map(|e| e.style.clone())
    }

    pub(crate) fn store_flow_style(
        &self,
        el: NodeId,
        style: Option<crate::flow::FlowStyle>,
        shared_from: Option<(NodeId, u64)>,
    ) {
        let mut cache = self.cache.borrow_mut();
        cache.flow_seq += 1;
        let stamp = cache.flow_seq;
        cache.flow.insert(el, FlowEntry { style, stamp, shared_from });
    }

    /// The flow style of an element whose cascade must equal that of an
    /// already styled one (Blink's and Stylo's style sharing): one of its
    /// four previous element siblings (striped rows alternate), or the
    /// element at its position under the element
    /// its parent shared from (rows React inserts: each cell and its
    /// contents then share with the row above). Returns the style and the
    /// source with its stamp. Conservative: same tag, no id, the same
    /// attributes (bar data-/aria- ones no selector reads), no state-bearing
    /// elements (form controls, links, replaced, dialog/details, popovers),
    /// the same result on both for every selector that could still tell
    /// them apart (structural pseudo-classes, sibling combinators, :has(),
    /// validity), no hovered or focused element under either side, and no
    /// shadow roots or unbounded :has() at all.
    pub(crate) fn shared_flow_style(
        &self,
        dom: &Dom,
        el: NodeId,
    ) -> Option<(Option<crate::flow::FlowStyle>, (NodeId, u64))> {
        let idx = self.owns(dom, el)?;
        let rules = self.cache.borrow().rules.clone();
        if rules.has_relative || dom.shadow_root_count() != 0 {
            return None;
        }
        let parent = dom.selector_parent(idx)?;
        let cache = self.cache.borrow();
        // Up to 4 previous element siblings (striped rows alternate).
        let mut candidates: [Option<u32>; 5] = [None; 5];
        let mut prev = dom.n(idx).prev;
        let mut found = 0;
        while prev != NIL && found < 4 {
            if dom.n(prev).is_element() {
                candidates[found] = Some(prev);
                found += 1;
            }
            prev = dom.n(prev).prev;
        }
        // The same-position child of the element the parent shared from.
        if let Some(&FlowEntry { shared_from: Some((src, stamp)), .. }) = cache.flow.get(&dom.id_of(parent)) {
            if cache.flow.get(&src).is_some_and(|e| e.stamp == stamp) {
                let position = |p: u32, of: Option<u32>, nth: Option<usize>| -> (usize, Option<u32>) {
                    let mut i = 0;
                    let mut c = dom.n(p).first_child;
                    while c != NIL {
                        if dom.n(c).is_element() {
                            if Some(c) == of || Some(i) == nth {
                                return (i, Some(c));
                            }
                            i += 1;
                        }
                        c = dom.n(c).next;
                    }
                    (i, None)
                };
                let (k, _) = position(parent, Some(idx), None);
                if let Ok(src_idx) = dom.check(src) {
                    candidates[4] = position(src_idx, None, Some(k)).1;
                }
            }
        }
        for c in candidates.into_iter().flatten() {
            let Some(entry) = cache.flow.get(&dom.id_of(c)) else { continue };
            if shares_cascade(dom, &rules, idx, c) {
                return Some((entry.style.clone(), (dom.id_of(c), entry.stamp)));
            }
        }
        None
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
            above.inert |= dom.n(e).element().is_some_and(|x| x.is_html() && x.attr("inert").is_some());
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
        if let Some(raw) = &c.width_variable {
            c.width = resolve_dimension_variable(dom, rules, id, raw);
        }
        if let Some(raw) = &c.height_variable {
            c.height = resolve_dimension_variable(dom, rules, id, raw);
        }

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
        let mut l = if width { d.width } else { d.height }?;
        if let Some(raw) = if width { &d.width_variable } else { &d.height_variable } {
            self.ensure(dom);
            l = resolve_dimension_variable(dom, &self.cache.borrow().rules, el, raw);
        }
        l.resolve(&self.viewport)
    }

    /// A block-level flex/grid container in normal flow with an auto width
    /// that isn't a flex/grid item (StyleManager.zig:764-779). Unresolved
    /// percentages retain the same fallback; definite ones resolve in Layout.
    pub fn width_from_containing_block(&self, dom: &Dom, el: NodeId) -> bool {
        let Some(c) = self.computed(dom, el) else { return false };
        if !c.block_flex_or_grid || !matches!(c.width.unit, LengthUnit::Auto | LengthUnit::Percentage) {
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
        custom_property_source(dom, &rules, el, name).map(|(_, value)| value)
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
        let idx = self.owns(dom, el)?;
        self.ensure(dom);
        let rules = self.cache.borrow().rules.clone();
        let value =
            |i: u32, name: &str| self.computed_value_at(dom, &rules, i, name, 0, false).map(|v| v.to_ascii_lowercase());
        let display = |i: u32| {
            value(i, "display").unwrap_or_else(|| match dom.n(i).element() {
                Some(e) if e.is_html() => crate::style_flow::ua_display(&e.name.local.to_ascii_lowercase()).to_owned(),
                _ => "inline".to_owned(),
            })
        };
        let own = display(idx);
        if matches!(own.as_str(), "none" | "contents") {
            return Some(own);
        }
        let root = matches!(dom.n(dom.n(idx).parent).kind, NodeKind::Document(_));
        let floated = value(idx, "float").is_some_and(|v| v != "none");
        let out_of_flow = matches!(value(idx, "position").as_deref(), Some("absolute" | "fixed"));
        // The parent box: display:contents ancestors generate none.
        let mut parent = flat_tree_parent_element(dom, idx);
        while let Some(p) = parent.filter(|&p| display(p) == "contents") {
            parent = flat_tree_parent_element(dom, p);
        }
        let item =
            parent.is_some_and(|p| matches!(display(p).as_str(), "flex" | "inline-flex" | "grid" | "inline-grid"));
        if !(root || floated || out_of_flow || item) {
            return Some(own);
        }
        let blockified = match own.as_str() {
            "inline-flex" => "flex",
            "inline-grid" => "grid",
            "inline-table" => "table",
            "inline" | "inline-block" | "run-in" => "block",
            d if d.starts_with("table-") || d.starts_with("ruby") => "block",
            _ => return Some(own),
        };
        Some(blockified.to_owned())
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
        let idx = self.owns(dom, el)?;
        self.ensure(dom);
        let rules = self.cache.borrow().rules.clone();
        self.computed_value_at(dom, &rules, idx, name, 0, precise)
    }

    /// Computed font-weight as a number (Blink's FontSelectionValue): keywords
    /// resolve, `bolder`/`lighter` against the parent (css-fonts-4 relative
    /// weights), and the UA sheet's `b, strong, optgroup { bolder }` and
    /// `th, h1-h6 { bold }` apply without an author value.
    fn font_weight_at(&self, dom: &Dom, rules: &RuleSet, idx: u32, depth: u8) -> f64 {
        if depth >= MAX_COMPUTED_DEPTH {
            return 400.0;
        }
        if let Some(v) =
            PASS_MEMO.with(|m| m.borrow().as_ref().and_then(|m| m.font_weight.get(&(dom_key(dom), idx)).copied()))
        {
            return v;
        }
        let v = self.font_weight_at_uncached(dom, rules, idx, depth);
        PASS_MEMO.with(|m| {
            if let Some(m) = m.borrow_mut().as_mut() {
                m.font_weight.insert((dom_key(dom), idx), v);
            }
        });
        v
    }

    fn font_weight_at_uncached(&self, dom: &Dom, rules: &RuleSet, idx: u32, depth: u8) -> f64 {
        let parent =
            || flat_tree_parent_element(dom, idx).map_or(400.0, |p| self.font_weight_at(dom, rules, p, depth + 1));
        let id = dom.id_of(idx);
        let declared = self.declared_value(dom, rules, id, "font-weight").and_then(|raw| {
            let mut lookup = |var: &str| custom_property_source(dom, rules, id, var).map(|(_, value)| value);
            computed::substitute_vars(&raw, &mut lookup, 0)
        });
        let ua = || match dom.n(idx).element().filter(|e| e.is_html()).map(|e| &*e.name.local) {
            Some("b" | "strong" | "optgroup") => Some("bolder"),
            Some("th" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => Some("bold"),
            _ => None,
        };
        let value = declared.map(|v| v.trim().to_ascii_lowercase()).or_else(|| ua().map(str::to_owned));
        match value.as_deref() {
            None | Some("inherit" | "unset" | "revert" | "revert-layer") => parent(),
            Some("initial" | "normal") => 400.0,
            Some("bold") => 700.0,
            Some("bolder") => match parent() {
                p if p < 350.0 => 400.0,
                p if p < 550.0 => 700.0,
                p => p.max(900.0),
            },
            Some("lighter") => match parent() {
                p if p < 100.0 => p,
                p if p < 550.0 => 100.0,
                p if p < 750.0 => 400.0,
                _ => 700.0,
            },
            Some(v) => computed::resolve_number(v).filter(|n| (1.0..=1000.0).contains(n)).unwrap_or_else(parent),
        }
    }

    fn computed_value_at(
        &self,
        dom: &Dom,
        rules: &RuleSet,
        idx: u32,
        name: &str,
        depth: u8,
        precise: bool,
    ) -> Option<String> {
        let num = |x: f64| if precise { format!("{x}") } else { computed::fmt_num(x) };
        if depth >= MAX_COMPUTED_DEPTH {
            return None;
        }
        let id = dom.id_of(idx);
        if name == "font-size" {
            // Inherited and resolved against ancestors; None when no author value applies anywhere up the tree.
            let mut cur = Some(idx);
            while let Some(i) = cur {
                if self.declared_value(dom, rules, dom.id_of(i), name).is_some() {
                    return Some(format!("{}px", num(self.font_size_at_cascade(dom, rules, idx, depth))));
                }
                cur = flat_tree_parent_element(dom, i);
            }
            return None;
        }
        if name == "font-weight" {
            return Some(num(self.font_weight_at(dom, rules, idx, depth)));
        }
        let inherited = computed::is_inherited(name);
        let from_parent = || {
            flat_tree_parent_element(dom, idx)
                .and_then(|p| self.computed_value_memo(dom, rules, p, name, depth + 1, precise))
        };
        let unset = || if inherited { from_parent() } else { None };
        let Some(raw) = self.declared_value(dom, rules, id, name) else { return unset() };
        match raw.trim().to_ascii_lowercase().as_str() {
            "inherit" => return from_parent(),
            "unset" | "revert" | "revert-layer" => return unset(),
            "initial" => return None,
            _ => {}
        }
        let mut lookup = |var: &str| custom_property_source(dom, rules, id, var).map(|(_, value)| value);
        // Invalid at computed-value time behaves as `unset`.
        let Some(value) = computed::substitute_vars(&raw, &mut lookup, 0) else { return unset() };
        let value = value.trim();
        if computed::is_color_property(name) {
            let mut current = || {
                let color = if name == "color" {
                    from_parent()
                } else {
                    self.computed_value_at(dom, rules, idx, "color", depth + 1, precise)
                };
                Some(color.unwrap_or_else(|| "rgb(0, 0, 0)".into()))
            };
            return Some(computed::serialize_color(value, &mut current).unwrap_or_else(|| value.to_owned()));
        }
        if computed::is_length_property(name) {
            let cx = self.length_context(dom, rules, idx, depth);
            // Percentages stay as declared: their basis is a used value the faux layout lacks.
            if let Some((px, 0.0)) = computed::resolve_len(value, &cx).filter(|_| !value.contains('%')) {
                return Some(format!("{}px", num(px)));
            }
        }
        Some(computed::resolve_number(value).map_or_else(|| value.to_owned(), num))
    }

    /// `computed_value_at` memoized for the layout pass (inherited lookups:
    /// every descendant without its own value asks its parent).
    fn computed_value_memo(
        &self,
        dom: &Dom,
        rules: &RuleSet,
        idx: u32,
        name: &str,
        depth: u8,
        precise: bool,
    ) -> Option<String> {
        let key = (dom_key(dom), idx, precise);
        let hit = PASS_MEMO
            .with(|m| m.borrow().as_ref().map(|m| m.inherited.get(name).and_then(|by_el| by_el.get(&key).cloned())));
        match hit {
            None => return self.computed_value_at(dom, rules, idx, name, depth, precise),
            Some(Some(v)) => return v,
            Some(None) => {}
        }
        let v = self.computed_value_at(dom, rules, idx, name, depth, precise);
        PASS_MEMO.with(|m| {
            if let Some(m) = m.borrow_mut().as_mut() {
                match m.inherited.get_mut(name) {
                    Some(by_el) => {
                        by_el.insert(key, v.clone());
                    }
                    None => {
                        m.inherited.insert(name.to_owned(), FxHashMap::from_iter([(key, v.clone())]));
                    }
                }
            }
        });
        v
    }

    /// The element's own winning declaration of a longhand: inline (shorthands
    /// expanded) against the author rules, then SVG presentation attributes.
    fn declared_value(&self, dom: &Dom, rules: &RuleSet, el: NodeId, name: &str) -> Option<String> {
        if let Some(value) = self.declared_map(dom, rules, el).get(name) {
            return Some(value.clone());
        }
        // SVG presentation attributes sit below every author rule (SVG 2 §6.6).
        if computed::is_presentation_attribute(name) {
            let e = dom.element(el).ok().filter(|e| e.name.ns == ns!(svg))?;
            return e.attr(name).map(str::to_owned);
        }
        None
    }

    /// All winning author declarations of `el`, computed in one pass over the
    /// bucketed candidate rules and cached until the DOM or sheets change.
    fn declared_map(&self, dom: &Dom, rules: &RuleSet, el: NodeId) -> Rc<FxHashMap<String, String>> {
        if let Some(map) = self.cache.borrow().declared.get(&el) {
            return map.clone();
        }
        let mut winners: FxHashMap<String, (String, u64)> = FxHashMap::default();
        let mut offer = |name: &str, value: &str, priority: u64| match winners.get_mut(name) {
            Some(slot) if slot.1 > priority => {}
            Some(slot) => *slot = (value.to_owned(), priority),
            None => {
                winners.insert(name.to_owned(), (value.to_owned(), priority));
            }
        };
        let Ok(e) = dom.element(el) else { return Rc::default() };
        if let Some(style) = e.attr("style") {
            for d in parse_declarations(style) {
                if is_custom_property(&d.name) {
                    continue;
                }
                let p = if d.important { INLINE_IMPORTANT_PRIORITY } else { INLINE_PRIORITY };
                for (longhand, v) in computed::expand(&d.name, &d.value) {
                    offer(&longhand, &v, p);
                }
            }
        }
        // Rules whose ancestor compounds need a tag, id or class no ancestor
        // has cannot match (most `.a *`-style rules, tested on every element).
        let mut bits: Option<Option<Rc<AncestorBits>>> = None;
        let mut check = |list: &[u32]| {
            for &ri in list {
                let r = &rules.generic_rules[ri as usize];
                if r.ancestors[0] != 0 {
                    let b = bits.get_or_insert_with(|| dom.check(el).ok().and_then(|idx| ancestor_bits(dom, idx)));
                    if b.as_ref().is_some_and(|b| r.ancestors.iter().any(|&h| h != 0 && !b.might_contain(h))) {
                        continue;
                    }
                }
                if !dom.matches(el, &r.selector).unwrap_or(false) {
                    continue;
                }
                for (name, value, important) in r.decls.iter() {
                    offer(name, value, if *important { r.important } else { r.normal });
                }
            }
        };
        if let Some(i) = e.attr("id") {
            if let Some(list) = rules.generic_by_id.get(i) {
                check(list);
            }
        }
        if let Some(class) = e.attr("class") {
            for token in class.split(is_ascii_ws_char).filter(|s| !s.is_empty()) {
                if let Some(list) = rules.generic_by_class.get(token) {
                    check(list);
                }
            }
        }
        if !rules.generic_by_tag.is_empty() {
            let local = e.name.local.to_ascii_lowercase();
            if let Some(list) = rules.generic_by_tag.get(local.as_str()) {
                check(list);
            }
        }
        check(&rules.generic_other);
        let map: Rc<FxHashMap<String, String>> = Rc::new(winners.into_iter().map(|(k, (v, _))| (k, v)).collect());
        self.cache.borrow_mut().declared.insert(el, map.clone());
        map
    }

    /// Units for lengths declared on `idx`: its font-size (em), the root's (rem), the viewport (vw/vh).
    fn length_context(&self, dom: &Dom, rules: &RuleSet, idx: u32, depth: u8) -> computed::LengthContext {
        computed::LengthContext {
            font_size: self.font_size_at_cascade(dom, rules, idx, depth),
            root_font_size: self.root_font_size(dom, rules),
            viewport_width: self.viewport.width as f64,
            viewport_height: self.viewport.height as f64,
            line_height: None,
        }
    }

    fn root_font_size(&self, dom: &Dom, rules: &RuleSet) -> f64 {
        let root = dom.document_element(self.doc).ok().flatten().and_then(|r| dom.check(r).ok());
        root.map_or(DEFAULT_FONT_SIZE, |r| {
            self.font_size_from_parent(dom, rules, r, DEFAULT_FONT_SIZE, DEFAULT_FONT_SIZE)
        })
    }

    /// The computed font-size of `idx` from the cascade: inherited; keywords,
    /// `em`/`%` against the parent's size, `rem` against the root's.
    fn font_size_at_cascade(&self, dom: &Dom, rules: &RuleSet, idx: u32, depth: u8) -> f64 {
        if depth >= MAX_COMPUTED_DEPTH {
            return DEFAULT_FONT_SIZE;
        }
        if let Some(v) =
            PASS_MEMO.with(|m| m.borrow().as_ref().and_then(|m| m.font_size.get(&(dom_key(dom), idx)).copied()))
        {
            return v;
        }
        let v = self.font_size_at_cascade_uncached(dom, rules, idx, depth);
        PASS_MEMO.with(|m| {
            if let Some(m) = m.borrow_mut().as_mut() {
                m.font_size.insert((dom_key(dom), idx), v);
            }
        });
        v
    }

    fn font_size_at_cascade_uncached(&self, dom: &Dom, rules: &RuleSet, idx: u32, depth: u8) -> f64 {
        let Some(parent) = flat_tree_parent_element(dom, idx) else {
            return self.font_size_from_parent(dom, rules, idx, DEFAULT_FONT_SIZE, DEFAULT_FONT_SIZE);
        };
        let parent_size = self.font_size_at_cascade(dom, rules, parent, depth + 1);
        let root = self.root_font_size(dom, rules);
        self.font_size_from_parent(dom, rules, idx, parent_size, root)
    }

    fn font_size_from_parent(&self, dom: &Dom, rules: &RuleSet, idx: u32, parent: f64, root: f64) -> f64 {
        let id = dom.id_of(idx);
        let Some(raw) = self.declared_value(dom, rules, id, "font-size") else { return parent };
        let mut lookup = |var: &str| custom_property_source(dom, rules, id, var).map(|(_, value)| value);
        let Some(value) = computed::substitute_vars(&raw, &mut lookup, 0) else { return parent };
        let keyword = match value.trim().to_ascii_lowercase().as_str() {
            "inherit" | "unset" | "revert" | "revert-layer" => return parent,
            "initial" | "medium" => Some(16.0),
            "xx-small" => Some(9.0),
            "x-small" => Some(10.0),
            "small" => Some(13.0),
            "large" => Some(18.0),
            "x-large" => Some(24.0),
            "xx-large" => Some(32.0),
            "xxx-large" => Some(48.0),
            "smaller" => Some(parent / 1.2),
            "larger" => Some(parent * 1.2),
            _ => None,
        };
        if let Some(px) = keyword {
            return px;
        }
        let cx = computed::LengthContext {
            font_size: parent,
            root_font_size: root,
            viewport_width: self.viewport.width as f64,
            viewport_height: self.viewport.height as f64,
            line_height: None,
        };
        match computed::resolve_len(&value, &cx) {
            Some((px, pct)) => px + pct * parent / 100.0,
            None => parent,
        }
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

#[cfg(test)]
mod has_anchor_tests {
    use super::{collect_has_anchors, collect_sibling_keys, collect_validity_keys, AnchorKey, ChildKey, HasAnchor};
    use crate::select::{Impl, PseudoClass};
    use selectors::parser::Component;

    fn siblings(text: &str) -> Vec<AnchorKey> {
        let list = crate::select::parse_selectors(text).expect("selector");
        let (mut out, mut structural) = (Vec::new(), Vec::new());
        for sel in list.slice() {
            collect_sibling_keys(sel, &mut out, &mut structural);
        }
        out
    }

    fn structural(text: &str) -> Vec<ChildKey> {
        let list = crate::select::parse_selectors(text).expect("selector");
        let (mut out, mut structural) = (Vec::new(), Vec::new());
        for sel in list.slice() {
            collect_sibling_keys(sel, &mut out, &mut structural);
        }
        structural
    }

    #[test]
    fn collects_structural_sibling_and_pseudo_keys() {
        // Insertions change adjacency for type-only compounds too.
        let ck = |key: AnchorKey, parent: Option<AnchorKey>| ChildKey { key, parent };
        assert_eq!(
            structural(".x>*+*, li+li"),
            vec![ck(AnchorKey::Any, Some(AnchorKey::Class("x".into()))), ck(AnchorKey::Tag("li".into()), None)]
        );
        let keys = |text: &str, hit: &dyn Fn(&Component<Impl>) -> bool| {
            let list = crate::select::parse_selectors(text).expect("selector");
            let mut out = Vec::new();
            for sel in list.slice() {
                super::collect_keys(sel, hit, &mut out);
            }
            out
        };
        let hover = |c: &Component<Impl>| matches!(c, Component::NonTSPseudoClass(PseudoClass::Hover));
        // Tailwind group-hover: the group ancestor; hover: the element.
        assert_eq!(keys(r".x:is(:where(.group):hover *)", &hover), vec![AnchorKey::Class("group".into())]);
        assert_eq!(keys(r".hover\:bg:hover", &hover), vec![AnchorKey::Class("hover:bg".into())]);
        // :not(:hover) keys the outer compound, not Any.
        assert_eq!(keys(".y:not(:hover)", &hover), vec![AnchorKey::Class("y".into())]);
        // Inside :has(): the anchor walk's business.
        assert_eq!(keys(".g:has(:hover)", &hover), vec![]);
        let nth = |text: &str| {
            let list = crate::select::parse_selectors(text).expect("selector");
            let mut out = Vec::new();
            for sel in list.slice() {
                super::collect_nth_keys(sel, &mut out, false);
            }
            out
        };
        // TOS space-y/divide: any child, of a .space-y-4 parent.
        assert_eq!(
            nth(r":where(.space-y-4>:not(:last-child))"),
            vec![ck(AnchorKey::Any, Some(AnchorKey::Class("space-y-4".into())))]
        );
        assert_eq!(
            nth(".x>div:last-child>*"),
            vec![ck(AnchorKey::Tag("div".into()), Some(AnchorKey::Class("x".into())))]
        );
        assert_eq!(nth(r".even\:bg:nth-child(2n)"), vec![ck(AnchorKey::Class("even:bg".into()), None)]);
        assert_eq!(nth(".a .b"), vec![]);
    }

    #[test]
    fn collects_sibling_keys() {
        // TOS [&>*+*], [&>div+div]: type-only left compounds.
        assert_eq!(siblings(r".\[\&\>\*\+\*\]\:border-l>*+*, .x>div+div, .d li+li"), vec![]);
        assert_eq!(
            siblings("[data-pinned-position=start]+[data-pinned-position=center]"),
            vec![AnchorKey::Attr("data-pinned-position".into())]
        );
        assert_eq!(siblings(".peer:checked ~ .x"), vec![AnchorKey::Class("peer".into())]);
        assert_eq!(siblings("input:checked + label"), vec![AnchorKey::Tag("input".into())]);
        assert_eq!(siblings(".y:is(.a + *)"), vec![AnchorKey::Class("a".into())]);
        assert_eq!(siblings("li:nth-child(2 of .x)"), vec![AnchorKey::Any]);
        // Descendant/child only: none.
        assert_eq!(siblings(".a .b > .c"), vec![]);
    }

    fn validity(text: &str) -> Option<Vec<AnchorKey>> {
        let list = crate::select::parse_selectors(text).expect("selector");
        let mut out = Vec::new();
        for sel in list.slice() {
            if !collect_validity_keys(sel, &mut out) {
                return None;
            }
        }
        Some(out)
    }

    #[test]
    fn collects_validity_keys() {
        // TOS (Tailwind invalid:*): the subject compound's class.
        assert_eq!(
            validity(
                r".invalid\:border:where([data-rac])[data-invalid],.invalid\:border:where(:not([data-rac])):invalid"
            ),
            Some(vec![AnchorKey::Class("invalid:border".into())])
        );
        // group-invalid via [data-invalid] tests no validity.
        assert_eq!(validity(r".x:is(:where(.group):where([data-rac])[data-invalid] *)"), Some(vec![]));
        assert_eq!(validity("form:invalid .btn"), Some(vec![AnchorKey::Tag("form".into())]));
        assert_eq!(validity(":is(:valid)"), Some(vec![AnchorKey::Any]));
        // Inside :has(): unbounded, so full invalidation.
        assert_eq!(validity(".card:has(input:invalid)"), None);
    }

    fn anchors(text: &str) -> Option<Vec<HasAnchor>> {
        let list = crate::select::parse_selectors(text).expect("selector");
        let mut out = Vec::new();
        for sel in list.slice() {
            if !collect_has_anchors(sel, &mut out) {
                return None;
            }
        }
        Some(out)
    }

    fn one(key: AnchorKey, depth: Option<u32>) -> Option<Vec<HasAnchor>> {
        Some(vec![HasAnchor { key, depth, context: None }])
    }

    #[test]
    fn classifies_tailwind_has_selectors() {
        // group-has-*: descendants of the group anchor, any depth.
        assert_eq!(
            anchors(r".x:is(:where(.group\/input-content):has(:is(input:disabled)) *)"),
            one(AnchorKey::Class("group/input-content".into()), None)
        );
        // Arbitrary-value class names (serialized unescaped by the selector
        // crate) are read from the syntax tree, not text.
        assert_eq!(
            anchors(r".\[\&\:has\(\[data-is-filled\]\)\]\:bg-card:has([data-is-filled])"),
            one(AnchorKey::Class("[&:has([data-is-filled])]:bg-card".into()), None)
        );
        assert_eq!(anchors("td:has(input)"), one(AnchorKey::Tag("td".into()), None));
        // Universal anchors are bounded only by child-only relative selectors.
        // TOS: a universal anchor below a [data-is-empty] ancestor (never
        // html or body).
        assert_eq!(
            anchors("[data-is-empty=true] :has(> [data-static-segment-type]) > .text-inherit"),
            Some(vec![HasAnchor {
                key: AnchorKey::Any,
                depth: Some(1),
                context: Some((AnchorKey::Attr("data-is-empty".into()), false)),
            }])
        );
        assert_eq!(
            anchors(".p > :has(> b)"),
            Some(vec![HasAnchor {
                key: AnchorKey::Any,
                depth: Some(1),
                context: Some((AnchorKey::Class("p".into()), true))
            }])
        );
        assert_eq!(anchors(":has(> div > [x])"), one(AnchorKey::Any, Some(2)));
        assert_eq!(anchors(":has(.x)"), None, "unbounded universal anchor");
        assert_eq!(anchors(".a:has(+ .b)"), None, "sibling-relative");
        assert_eq!(anchors(".a:has(~ .b)"), None, "sibling-relative");
        assert_eq!(anchors(".plain .selector"), Some(vec![]));
    }
}
