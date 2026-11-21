//! Script handles for tree-abiding pseudo-elements. Rendering does not create the handle.
use crate::lazy_module::LazyModule;
use pyro_js::{JsResult, Object, Scope};

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "cssPseudoElement",
    source: include_str!("pseudo_element.js"),
    url: "pyro://internal/pseudo_element.js",
    natives: Some(natives),
};

fn selector(text: &str) -> Option<(String, Option<String>)> {
    use cssparser::{ParseError, Parser, Token};
    let mut input = Parser::new(text.trim());
    if !matches!(input.next_including_whitespace().ok()?, Token::Colon) { return None; }
    let next = input.next_including_whitespace().ok()?.clone();
    let double = matches!(next, Token::Colon);
    let token = if double { input.next_including_whitespace().ok()?.clone() } else { next };
    let (name, arg) = match token {
        Token::Ident(name) => {
            let name = name.to_ascii_lowercase();
            if !double && name != "before" && name != "after" { return None; }
            if !matches!(name.as_str(), "before" | "after" | "marker" | "backdrop" | "scroll-marker" | "view-transition") { return None; }
            (name, None)
        }
        Token::Function(name) if double => {
            let name = name.to_ascii_lowercase();
            let arg = input.parse_nested_block(|p| -> Result<_, ParseError<()>> {
                let ident = p.expect_ident_cloned()?;
                p.expect_exhausted()?;
                Ok(ident)
            }).ok()?;
            let lower = arg.to_ascii_lowercase();
            let serialized = if name == "scroll-button" {
                if !matches!(lower.as_str(), "up" | "right" | "down" | "left" | "block-start" | "block-end" | "inline-start" | "inline-end") { return None; }
                lower
            } else {
                if !matches!(name.as_str(), "view-transition-group" | "view-transition-image-pair" | "view-transition-old" | "view-transition-new")
                    || matches!(lower.as_str(), "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default") { return None; }
                let mut serialized = String::new();
                cssparser::serialize_identifier(&arg, &mut serialized).ok()?;
                serialized
            };
            (name, Some(serialized))
        }
        _ => return None,
    };
    input.expect_exhausted().ok()?;
    Some((format!("::{name}"), arg))
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let api = s.object();
    let parse = s.function("parse", |s, _, args| {
        let text = crate::util::dom_string(args.get(0))?;
        let Some((kind, argument)) = selector(&text) else { return Ok(s.null()) };
        Ok(s.array(&[s.string(&kind), argument.as_deref().map_or_else(|| s.null(), |v| s.string(v))])?.value())
    })?;
    api.set_atom("parse", parse.value())?;
    Ok(api)
}
