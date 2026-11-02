//! Computed values of the few properties lp-dom reads that Stylo's servo
//! build (Blitz's, so the page's cascade) does not parse: they are
//! `engine = "gecko"` in Stylo's `longhands.toml`, so their declarations are
//! dropped with the sheet. `overflow-anchor` (scroll anchoring), `text-anchor`
//! (SVG text boxes) and `line-clamp` (its `getComputedStyle` value) are
//! resolved here from the same sheets: style rules (inside `@media` that
//! matches, and inside `@supports`/`@layer`/`@container`/`@scope` as if they
//! applied) and the `style` attribute, matched with Stylo's selector
//! matching, in cascade order (`!important` first, then specificity, then
//! source order) with `inherit`/`initial`/`unset` and inheritance where the
//! property inherits. Nothing else reads this: upstreaming servo support for
//! these properties to Stylo would let the page's cascade answer and remove
//! this module.

use std::rc::Rc;

use cssparser::{ParseError, Parser, Token};

use crate::dom::Dom;
use crate::media::Viewport;
use crate::node::NodeId;
use crate::stylo::select::List;

/// (name, inherited, initial value).
const PROPERTIES: [(&str, bool, &str); 3] =
    [("overflow-anchor", false, "auto"), ("text-anchor", true, "start"), ("line-clamp", false, "none")];

/// Whether this module answers `name`.
pub(crate) fn handles(name: &str) -> bool {
    PROPERTIES.iter().any(|p| p.0 == name)
}

/// A style rule declaring one of [`PROPERTIES`].
pub(crate) struct Rule {
    selectors: List,
    /// (property, value, important).
    declarations: Vec<(&'static str, String, bool)>,
}

/// The rules of `sheets` (texts in cascade order) that declare one of the
/// properties, in source order.
pub(crate) fn rules<'a>(sheets: impl Iterator<Item = &'a str>, viewport: &Viewport) -> Rc<Vec<Rule>> {
    let mut out = Vec::new();
    for text in sheets {
        if PROPERTIES.iter().any(|p| text.contains(p.0)) {
            rule_list(&mut Parser::new(text), text, viewport, &mut out);
        }
    }
    Rc::new(out)
}

fn next_tok<'i>(p: &mut Parser<'i>) -> Option<Token<'i>> {
    p.next_including_whitespace_and_comments().ok().cloned()
}

/// Consumes the contents of the block `t` opened, if it opened one.
fn skip_block(p: &mut Parser<'_>, t: &Token<'_>) {
    if matches!(t, Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock)
    {
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok(())
        });
    }
}

/// The rules of a rule list (`src` is the whole sheet: positions index it).
fn rule_list(p: &mut Parser<'_>, src: &str, viewport: &Viewport, out: &mut Vec<Rule>) {
    loop {
        let start = p.position().byte_index();
        let Some(first) = next_tok(p) else { return };
        let at = match &first {
            Token::WhiteSpace(_) | Token::Comment(_) | Token::CDO | Token::CDC => continue,
            Token::AtKeyword(name) => Some(name.to_ascii_lowercase()),
            _ => None,
        };
        // The prelude, up to the rule's block (an at-rule may end at `;`).
        let prelude_start = if at.is_some() { p.position().byte_index() } else { start };
        let mut prelude_end = prelude_start;
        let mut block = matches!(first, Token::CurlyBracketBlock);
        if !block {
            if at.is_none() {
                skip_block(p, &first);
            }
            loop {
                let before = p.position().byte_index();
                match next_tok(p) {
                    None => return,
                    Some(Token::CurlyBracketBlock) => {
                        prelude_end = before;
                        block = true;
                        break;
                    }
                    Some(Token::Semicolon) if at.is_some() => break,
                    Some(t) => skip_block(p, &t),
                }
            }
        }
        if !block {
            continue;
        }
        let prelude = src.get(prelude_start..prelude_end).unwrap_or("").trim();
        let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
            match at.as_deref() {
                Some("media") if crate::media::matches(prelude, viewport) => rule_list(n, src, viewport, out),
                Some("supports" | "layer" | "container" | "scope") => rule_list(n, src, viewport, out),
                Some(_) => {}
                None => {
                    let body_start = n.position().byte_index();
                    while n.next_including_whitespace_and_comments().is_ok() {}
                    let body_end = n.position().byte_index();
                    style_rule(prelude, src.get(body_start..body_end).unwrap_or(""), out);
                }
            }
            while n.next_including_whitespace_and_comments().is_ok() {}
            Ok(())
        });
    }
}

fn style_rule(selectors: &str, body: &str, out: &mut Vec<Rule>) {
    if !PROPERTIES.iter().any(|p| body.contains(p.0)) {
        return;
    }
    let declarations: Vec<_> = crate::style::parse_declarations(body)
        .into_iter()
        .filter_map(|d| {
            let name = PROPERTIES.iter().find(|p| d.name.eq_ignore_ascii_case(p.0))?.0;
            Some((name, d.value.trim().to_owned(), d.important))
        })
        .collect();
    if declarations.is_empty() {
        return;
    }
    if let Some(selectors) = crate::stylo::select::parse(selectors) {
        out.push(Rule { selectors, declarations });
    }
}

/// `el`'s computed `name` (one of [`PROPERTIES`]): its own cascaded value,
/// else inherited or initial; `inline` gives an element's `style` attribute
/// declaration of a property.
pub(crate) fn value(
    dom: &Dom,
    rules: &[Rule],
    el: NodeId,
    name: &str,
    inline: &dyn Fn(NodeId) -> Option<(String, bool)>,
) -> String {
    let (_, inherited, initial) = *PROPERTIES.iter().find(|p| p.0 == name).expect("a handled property");
    let mut cur = Some(el);
    while let Some(e) = cur {
        let parent =
            dom.check(e).ok().and_then(|i| crate::style::flat_tree_parent_element(dom, i)).map(|p| dom.id_of(p));
        match cascaded(dom, rules, e, name, inline).as_deref() {
            Some(v) if v.eq_ignore_ascii_case("inherit") => cur = parent,
            Some(v) if v.eq_ignore_ascii_case("initial") => return initial.to_owned(),
            Some(v) if v.eq_ignore_ascii_case("unset") || v.eq_ignore_ascii_case("revert") => {
                if !inherited {
                    return initial.to_owned();
                }
                cur = parent;
            }
            Some(v) => return v.to_ascii_lowercase(),
            None if inherited => cur = parent,
            None => return initial.to_owned(),
        }
    }
    initial.to_owned()
}

/// The winning declaration of `name` on `el`: `!important` over normal,
/// inline over rules within each, then specificity, then source order.
fn cascaded(
    dom: &Dom,
    rules: &[Rule],
    el: NodeId,
    name: &str,
    inline: &dyn Fn(NodeId) -> Option<(String, bool)>,
) -> Option<String> {
    let idx = dom.check(el).ok()?;
    if !dom.n(idx).is_element() {
        return None;
    }
    let inline = inline(el);
    if let Some((v, true)) = &inline {
        return Some(v.clone());
    }
    // (important, specificity, order) of the best rule declaration so far.
    let mut best: Option<((bool, u32, usize), &str)> = None;
    for (order, rule) in rules.iter().enumerate() {
        let Some(&(_, ref value, important)) = rule.declarations.iter().rev().find(|d| d.0 == name) else {
            continue;
        };
        let specificity = rule
            .selectors
            .slice()
            .iter()
            .filter(|s| crate::stylo::select::matches(dom, idx, &List::from_one((*s).clone())))
            .map(|s| s.specificity())
            .max();
        let Some(specificity) = specificity else { continue };
        let key = (important, specificity, order);
        if best.is_none_or(|(k, _)| key >= k) {
            best = Some((key, value));
        }
    }
    match (best, inline) {
        (Some(((true, ..), v)), _) => Some(v.to_owned()),
        (_, Some((v, _))) => Some(v),
        (Some((_, v)), None) => Some(v.to_owned()),
        (None, None) => None,
    }
}
