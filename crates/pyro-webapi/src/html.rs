//! HTML element interfaces, attribute reflection and document helpers.

use ::url::quirks;
use std::rc::Rc;

use pyro_dom::events::TargetKey;
use pyro_dom::forms::{parse_html_int, InputType};
use pyro_dom::{Dom, NodeId};
use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::collections::{self, CollectionKind};
use crate::event::{EventCore, EventInterface};
use crate::node::{node_value, wrap_node, Element, NodeData};
use crate::page::PageDom;
use crate::util::*;

macro_rules! reflect_str {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, $attr).js()?.unwrap_or(""))),
            |_s, n, v| {
                let arg0 = dom_string(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, $attr, &arg0).js()
            },
        );
    };
}

macro_rules! reflect_bool {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.boolean(n.page().dom.borrow().has_attribute(n.id, $attr).js()?)),
            |_s, n, v| {
                n.page().dom.borrow_mut().toggle_attribute(n.id, $attr, Some(v.to_bool())).js()?;
                crate::input::blur_if_focus_disabled(n.page(), n.id, $attr);
                Ok(())
            },
        );
    };
}

/// A USVString attribute reflecting a URL. The getter is pure (it reads the
/// attribute and the document's base URL): `a.href` and `link.href` are read
/// in loops (Vite's preload helper compares every `<link>`'s `href` with each
/// module it preloads).
macro_rules! reflect_url {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.pure_accessor_typed($prop, pyro_js::GetterResult::String, reflect_url!(@get $attr), reflect_url!(@set $attr));
    };
    (@get $attr:literal) => {
        |s, n| {
            let dom = n.page().dom.borrow();
            let Some(v) = dom.get_attribute(n.id, $attr).js()? else { return Ok(s.string("")) };
            Ok(s.string(&resolve_reflected_url(n.page(), &dom, n.id, v)))
        }
    };
    (@set $attr:literal) => {
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, $attr, &arg0).js()
        }
    };
}

/// A DOMString attribute with `[LegacyNullToEmptyString]`.
macro_rules! reflect_str_nn {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, $attr).js()?.unwrap_or(""))),
            |_s, n, v| {
                let arg0 = null_to_empty(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, $attr, &arg0).js()
            },
        );
    };
}

/// `long`: parsed with the rules for parsing integers, `$default` when absent,
/// unparseable or out of range.
macro_rules! reflect_long {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(int_attr(n, $attr, i32::MIN as i64, $default)?)),
            |_s, n, v| set_attr(n, $attr, &to_i32(v)?.to_string()),
        );
    };
}

/// `long` limited to only non-negative numbers (default -1).
macro_rules! reflect_limited_long {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(int_attr(n, $attr, 0, -1)?)),
            |_s, n, v| {
                let x = to_i32(v)?;
                if x < 0 {
                    return Err(index_size_error($prop));
                }
                set_attr(n, $attr, &x.to_string())
            },
        );
    };
}

/// `unsigned long`: values above 2147483647 read and write as `$default`.
macro_rules! reflect_ulong {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(int_attr(n, $attr, 0, $default)?)),
            |_s, n, v| set_ulong(n, $attr, to_u32(v)?, $default),
        );
    };
}

/// `unsigned long` limited to only positive numbers: setting 0 throws.
macro_rules! reflect_positive_ulong {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(int_attr(n, $attr, 1, $default)?)),
            |_s, n, v| {
                let x = to_u32(v)?;
                if x == 0 {
                    return Err(index_size_error($prop));
                }
                set_ulong(n, $attr, x, $default)
            },
        );
    };
}

/// `unsigned long` limited to only positive numbers with fallback: setting 0
/// writes `$default`.
macro_rules! reflect_positive_ulong_fallback {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(int_attr(n, $attr, 1, $default)?)),
            |_s, n, v| {
                let x = to_u32(v)?;
                set_ulong(n, $attr, if x == 0 { $default } else { x }, $default)
            },
        );
    };
}

/// `unsigned long` clamped to the range `[$min, $max]`.
macro_rules! reflect_clamped_ulong {
    ($def:expr, $prop:literal, $attr:literal, $default:expr, $min:expr, $max:expr) => {
        $def.accessor(
            $prop,
            |s, n| {
                let v = parsed_attr(n, $attr, parse_html_int)?.filter(|&v| v >= 0);
                Ok(s.number(v.map_or($default as f64, |v| v.clamp($min, $max) as f64)))
            },
            |_s, n, v| set_ulong(n, $attr, to_u32(v)?, $default),
        );
    };
}

/// `double` limited to only positive numbers: setting a non-positive value
/// is ignored.
macro_rules! reflect_positive_double {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| Ok(s.number(parsed_attr(n, $attr, parse_html_float)?.filter(|&v| v > 0.0).unwrap_or($default))),
            |s, n, v| set_double(s, n, $attr, v, true),
        );
    };
}

/// An enumerated attribute limited to only known values; the content attribute
/// keeps whatever string was set.
macro_rules! reflect_enum {
    ($def:expr, $prop:literal, $attr:literal, $table:expr) => {
        $def.accessor(
            $prop,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(s.string($table.state(dom.get_attribute(n.id, $attr).js()?)))
            },
            |_s, n, v| set_attr(n, $attr, &dom_string(v)?),
        );
    };
}

/// A nullable CORS settings attribute: null when absent, null removes it.
macro_rules! reflect_cors {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(dom.get_attribute(n.id, $attr).js()?.map_or(s.null(), |v| s.string(CORS.state(Some(v)))))
            },
            |_s, n, v| {
                if v.is_nullish() {
                    n.page().dom.borrow_mut().remove_attribute(n.id, $attr).js()?;
                    return Ok(());
                }
                set_attr(n, $attr, &dom_string(v)?)
            },
        );
    };
}

/// A DOMTokenList attribute with `[PutForwards=value]`: assignment sets the
/// list's `value` (an observable `[[Set]]` on the list object).
macro_rules! reflect_tokens {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.put_forwards($prop, |s, n| Ok(collections::token_list(s, n.page(), n.id, $attr)?.value()), "value");
    };
}

/// `formAction`-style URL: the document URL when absent or empty.
macro_rules! reflect_action {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| {
                let dom = n.page().dom.borrow();
                let doc = dom.node_document(n.id).js()?;
                Ok(s.string(&match dom.get_attribute(n.id, $attr).js()? {
                    Some(a) if !a.is_empty() => resolve_url(&dom, n.id, a),
                    _ => dom.document_data(doc).js()?.url.to_string(),
                }))
            },
            |_s, n, v| set_attr(n, $attr, &dom_string(v)?),
        );
    };
}

macro_rules! simple_class {
    ($name:ident, $js:literal, $parent:ty) => {
        simple_class!($name, $js, $parent, |_def| {});
    };
    ($name:ident, $js:literal, $parent:ty, $body:expr) => {
        pub struct $name;
        impl JsClass for $name {
            const NAME: &'static str = $js;
            type Data = NodeData;
            type Parent = $parent;
            fn define(def: &mut ClassDef<Self>) {
                // HTML element interfaces are `[HTMLConstructor]`: constructible by a customized
                // built-in element's class (`class extends HTMLButtonElement`).
                if $js.starts_with("HTML") {
                    def.constructor(0, |s, nt, _a| crate::custom::construct_html_element(s, nt, $js));
                }
                let f: fn(&mut ClassDef<Self>) = $body;
                f(def);
            }
        }
    };
}

/// Resolves a URL attribute against the node document's inherited base or URL,
/// encoding the query in the document's encoding ("encoding-parse a URL").
pub fn resolve_url(dom: &Dom, node: NodeId, value: &str) -> String {
    let base = dom.node_document(node).ok().and_then(|d| dom.document_base_url(d).ok());
    let parsed = base.and_then(|b| pyro_net::Url::parse(b).ok());
    resolve_against(parsed.as_ref(), value, document_output_encoding(dom, node))
}

/// `resolve_url` with a UTF-8 query, for APIs specified to ignore the document
/// encoding (`fetch`).
pub fn resolve_url_utf8(dom: &Dom, node: NodeId, value: &str) -> String {
    let base = dom.node_document(node).ok().and_then(|d| dom.document_base_url(d).ok());
    let parsed = base.and_then(|b| pyro_net::Url::parse(b).ok());
    resolve_against(parsed.as_ref(), value, encoding_rs::UTF_8)
}

/// The output encoding of `node`'s document: UTF-8 stands in for UTF-16 and
/// `replacement` (Encoding "get an output encoding").
pub(crate) fn document_output_encoding(dom: &Dom, node: NodeId) -> &'static encoding_rs::Encoding {
    dom.node_document(node)
        .ok()
        .and_then(|d| dom.document_data(d).ok())
        .filter(|d| d.charset != "UTF-8")
        .and_then(|d| encoding_rs::Encoding::for_label(d.charset.as_bytes()))
        .map_or(encoding_rs::UTF_8, |e| e.output_encoding())
}

fn resolve_against(base: Option<&pyro_net::Url>, value: &str, enc: &'static encoding_rs::Encoding) -> String {
    let Some(b) = base else { return value.to_owned() };
    let joined = if enc == encoding_rs::UTF_8 {
        b.join(value)
    } else {
        // The query percent-encode set keeps `%`, so the escaped reference
        // for an unmappable character survives the parser untouched.
        let encode: &dyn Fn(&str) -> std::borrow::Cow<'_, [u8]> =
            &|q| std::borrow::Cow::Owned(crate::encoding::encode_with_ncr(enc, q, "%26%23", "%3B"));
        pyro_net::Url::options().base_url(Some(b)).encoding_override(Some(encode)).parse(value)
    };
    joined.map(String::from).unwrap_or_else(|_| value.to_owned())
}

/// `value` resolved against `base` when that is a concatenation: `base` is an
/// http(s) URL and `value` a path-absolute reference, or `base`'s own origin
/// followed by one, whose path holds only characters the URL parser keeps as
/// they are (no percent-encoding, no query or fragment, no backslash) and no
/// `.` or `..` segment. The parser returns the same string for these
/// (`simple_paths_resolve_as_the_parser_does`); `None` sends the rest to it.
fn resolve_simple(base: &pyro_net::Url, value: &str) -> Option<String> {
    if !matches!(base.scheme(), "http" | "https") {
        return None;
    }
    let origin = &base[..url::Position::BeforePath];
    let (path, absolute) = match value.strip_prefix(origin) {
        Some(rest) => (rest, true),
        None => (value, false),
    };
    let rest = path.strip_prefix('/')?;
    if rest.starts_with('/') {
        return None;
    }
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b);
    if !rest.bytes().all(plain) || rest.split('/').any(|seg| seg == "." || seg == "..") {
        return None;
    }
    if absolute {
        return Some(value.to_owned());
    }
    let mut out = String::with_capacity(origin.len() + path.len());
    out.push_str(origin);
    out.push_str(path);
    Some(out)
}

/// One bounded entry, with no node or JavaScript references. The actual base
/// string is the key: document URL/base setters do not increment DOM version,
/// and a retained wrapper can be adopted into a different owner document.
pub(crate) struct ReflectedUrlBase {
    source: String,
    parsed: Option<pyro_net::Url>,
}

fn resolve_reflected_url(page: &PageDom, dom: &Dom, node: NodeId, value: &str) -> String {
    const MAX_BASE_BYTES: usize = 4096;
    let Some(base) = dom.node_document(node).ok().and_then(|d| dom.document_base_url(d).ok()) else {
        return value.to_owned();
    };
    if page.is_disposed() || base.len() > MAX_BASE_BYTES {
        return resolve_url(dom, node, value);
    }
    let mut cache = page.reflected_url_base.borrow_mut();
    if cache.as_ref().is_none_or(|entry| entry.source != base) {
        let parsed = pyro_net::Url::parse(base).ok();
        // Canonicalization can expand non-ASCII input. Oversized bases remain
        // fully supported, but neither source nor parsed strings are retained.
        if parsed.as_ref().is_some_and(|url| url.as_str().len() > MAX_BASE_BYTES) {
            return resolve_against(parsed.as_ref(), value, document_output_encoding(dom, node));
        }
        *cache = Some(ReflectedUrlBase { source: base.to_owned(), parsed });
    }
    let parsed = cache.as_ref().and_then(|entry| entry.parsed.as_ref());
    if let Some(url) = parsed.and_then(|b| resolve_simple(b, value)) {
        return url;
    }
    resolve_against(parsed, value, document_output_encoding(dom, node))
}

// ----- attribute reflection helpers -------------------------------------------

/// Keywords in canonical case, plus the states for a missing attribute and for
/// a value that matches no keyword (`""` where the state has no keyword).
struct EnumAttr {
    keywords: &'static [&'static str],
    missing: &'static str,
    invalid: &'static str,
}

impl EnumAttr {
    fn state(&self, value: Option<&str>) -> &'static str {
        let Some(value) = value else { return self.missing };
        self.keywords.iter().find(|k| k.eq_ignore_ascii_case(value)).copied().unwrap_or(self.invalid)
    }
}

const DIR: EnumAttr = EnumAttr { keywords: &["ltr", "rtl", "auto"], missing: "", invalid: "" };
const CORS: EnumAttr = EnumAttr { keywords: &["anonymous", "use-credentials"], missing: "", invalid: "anonymous" };
const REFERRER_POLICY: EnumAttr = EnumAttr {
    keywords: &[
        "",
        "no-referrer",
        "no-referrer-when-downgrade",
        "same-origin",
        "origin",
        "strict-origin",
        "origin-when-cross-origin",
        "strict-origin-when-cross-origin",
        "unsafe-url",
    ],
    missing: "",
    invalid: "",
};
const LOADING: EnumAttr = EnumAttr { keywords: &["lazy", "eager"], missing: "eager", invalid: "eager" };
/// HTML fetch priority attribute (`<img>`, `<link>`, `<script>`).
const FETCH_PRIORITY: EnumAttr = EnumAttr { keywords: &["high", "low", "auto"], missing: "auto", invalid: "auto" };
const DECODING: EnumAttr = EnumAttr { keywords: &["async", "sync", "auto"], missing: "auto", invalid: "auto" };
const INPUT_MODE: EnumAttr = EnumAttr {
    keywords: &["none", "text", "tel", "url", "email", "numeric", "decimal", "search"],
    missing: "",
    invalid: "",
};
const ENTER_KEY_HINT: EnumAttr =
    EnumAttr { keywords: &["enter", "done", "go", "next", "previous", "search", "send"], missing: "", invalid: "" };
const ENCTYPE_KEYWORDS: &[&str] = &["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"];
const FORM_ENCTYPE: EnumAttr = EnumAttr {
    keywords: ENCTYPE_KEYWORDS,
    missing: "application/x-www-form-urlencoded",
    invalid: "application/x-www-form-urlencoded",
};
/// The submitter override has no missing value default: it defers to the form.
const FORM_ENCTYPE_OVERRIDE: EnumAttr =
    EnumAttr { keywords: ENCTYPE_KEYWORDS, missing: "", invalid: "application/x-www-form-urlencoded" };
const METHOD_KEYWORDS: &[&str] = &["get", "post", "dialog"];
const FORM_METHOD: EnumAttr = EnumAttr { keywords: METHOD_KEYWORDS, missing: "get", invalid: "get" };
const FORM_METHOD_OVERRIDE: EnumAttr = EnumAttr { keywords: METHOD_KEYWORDS, missing: "", invalid: "get" };
const COLOR_SPACE: EnumAttr =
    EnumAttr { keywords: &["limited-srgb", "display-p3"], missing: "limited-srgb", invalid: "limited-srgb" };
const FORM_AUTOCOMPLETE: EnumAttr = EnumAttr { keywords: &["on", "off"], missing: "on", invalid: "on" };
const CELL_SCOPE: EnumAttr = EnumAttr { keywords: &["row", "col", "rowgroup", "colgroup"], missing: "", invalid: "" };
const LINK_AS: EnumAttr = EnumAttr {
    keywords: &[
        "fetch",
        "audio",
        "document",
        "embed",
        "font",
        "image",
        "manifest",
        "object",
        "report",
        "script",
        "sharedworker",
        "style",
        "track",
        "video",
        "worker",
        "xslt",
    ],
    missing: "",
    invalid: "",
};
const TRACK_KIND: EnumAttr = EnumAttr {
    keywords: &["subtitles", "captions", "descriptions", "chapters", "metadata"],
    missing: "subtitles",
    invalid: "metadata",
};
const MARQUEE_BEHAVIOR: EnumAttr =
    EnumAttr { keywords: &["scroll", "slide", "alternate"], missing: "scroll", invalid: "scroll" };
const MARQUEE_DIRECTION: EnumAttr =
    EnumAttr { keywords: &["up", "right", "down", "left"], missing: "left", invalid: "left" };

/// Largest value a reflected `unsigned long` reads or writes.
const MAX_REFLECTED_INT: i64 = i32::MAX as i64;

fn skip_html_whitespace(input: &str) -> &str {
    input.trim_start_matches([' ', '\t', '\n', '\x0c', '\r'])
}

/// HTML "rules for parsing floating-point number values"; trailing garbage is
/// ignored and values too large for a double fail.
fn parse_html_float(input: &str) -> Option<f64> {
    let text = skip_html_whitespace(input);
    let bytes = text.as_bytes();
    let digit_at = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let digits_from = |mut i: usize| {
        while digit_at(i) {
            i += 1;
        }
        i
    };
    let start = usize::from(matches!(bytes.first(), Some(b'-' | b'+')));
    let mut end = digits_from(start);
    if end == start && !(bytes.get(end) == Some(&b'.') && digit_at(end + 1)) {
        return None;
    }
    if bytes.get(end) == Some(&b'.') {
        end = digits_from(end + 1);
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = end + 1 + usize::from(matches!(bytes.get(end + 1), Some(b'-' | b'+')));
        if digit_at(sign) {
            end = digits_from(sign);
        }
    }
    let value = text[..end].parse::<f64>().ok().filter(|v| v.is_finite())?;
    Some(if value == 0.0 { 0.0 } else { value })
}

fn parsed_attr<T>(n: &NodeData, attr: &str, parse: fn(&str) -> Option<T>) -> JsResult<Option<T>> {
    Ok(n.page().dom.borrow().get_attribute(n.id, attr).js()?.and_then(parse))
}

/// An integer attribute in `min..=2147483647`, else `default`.
/// `img.width` (`width`) or `img.height`: see the accessors.
fn img_dimension(n: &NodeData, width: bool) -> JsResult<f64> {
    let page = n.page();
    // pyro: a source in the list of available images has its size at once (`image_complete`).
    page.image_complete(n.id);
    let connected = page.dom.borrow().is_connected(n.id).unwrap_or(false);
    if connected {
        crate::animation_events::layout_flush(page, n.id);
        let rendered = page.layout().rendered_content_size(&page.dom.borrow(), n.id);
        if let Some((w, h)) = rendered {
            return Ok((if width { w } else { h }).trunc());
        }
    }
    let attr = int_attr(n, if width { "width" } else { "height" }, 0, -1.0)?;
    if attr >= 0.0 {
        return Ok(attr);
    }
    // Without a source (its `src` removed, no `srcset`) the image has none: 0 at once.
    let dom = page.dom.borrow();
    let sourceless = dom.get_attribute(n.id, "src").ok().flatten().is_none()
        && dom.get_attribute(n.id, "srcset").ok().flatten().is_none()
        && dom.parent(n.id).ok().flatten().and_then(|p| dom.element(p).ok()).is_none_or(|p| &*p.name.local != "picture");
    if sourceless {
        return Ok(0.0);
    }
    let natural = dom.reported_natural_size(n.id);
    Ok(natural.map_or(0.0, |(w, h)| (if width { w } else { h }).trunc()))
}

/// A canvas dimension attribute's value: HTML's rules for parsing non-negative integers (`100px`
/// is 100), the reflected `unsigned long`'s range, else `default`. One rule for `canvas.width`, the
/// bitmap (`canvas::canvas_size`) and layout (blitz-dom `parse_non_negative_integer`).
pub(crate) fn canvas_dimension(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(parse_html_int)
        .filter(|v| (0..=MAX_REFLECTED_INT).contains(v))
        .map_or(default, |v| v as u32)
}

fn int_attr(n: &NodeData, attr: &str, min: i64, default: impl Into<f64>) -> JsResult<f64> {
    let value = parsed_attr(n, attr, parse_html_int)?.filter(|v| (min..=MAX_REFLECTED_INT).contains(v));
    Ok(value.map_or_else(|| default.into(), |v| v as f64))
}

fn set_attr(n: &NodeData, attr: &str, value: &str) -> JsResult<()> {
    n.page().dom.borrow_mut().set_attribute(n.id, attr, value).js()
}

/// The element a legacy factory function (`new Image()`) creates: an HTML
/// element `local_name` of the realm's current document, before its wrapper.
fn create_factory_element(s: &Scope<'_>, local_name: &str) -> JsResult<(Rc<PageDom>, NodeId)> {
    let page = crate::target::page_of(s)?;
    let doc = page.document();
    let el = page.dom.borrow_mut().create_element(doc, local_name).js()?;
    Ok((page, el))
}

fn set_ulong(n: &NodeData, attr: &str, value: u32, default: u32) -> JsResult<()> {
    let value = if value as i64 > MAX_REFLECTED_INT { default } else { value };
    set_attr(n, attr, &value.to_string())
}

/// A WebIDL `double` write: non-finite values throw a TypeError.
fn set_double<'s>(s: &Scope<'s>, n: &NodeData, attr: &str, v: Value<'s>, positive: bool) -> JsResult<()> {
    let x = v.to_number()?;
    if !x.is_finite() {
        return Err(JsError::Type("The provided double value is non-finite.".into()));
    }
    if positive && x <= 0.0 {
        return Ok(());
    }
    // The JS number serialization is the "best representation" the spec asks for.
    set_attr(n, attr, &s.number(x).to_string()?)
}

fn index_size_error(prop: &str) -> JsError {
    JsError::dom("IndexSizeError", format!("The value provided for '{prop}' is out of range."))
}

/// `<meter>` values with the ordering constraints of HTML §4.10.14.
fn meter_value(n: &NodeData, which: &str) -> JsResult<f64> {
    let attr = |name| parsed_attr(n, name, parse_html_float);
    let min = attr("min")?.unwrap_or(0.0);
    let max = attr("max")?.unwrap_or(1.0).max(min);
    Ok(match which {
        "min" => min,
        "max" => max,
        "value" => attr("value")?.unwrap_or(0.0).clamp(min, max),
        "low" => attr("low")?.unwrap_or(min).clamp(min, max),
        "high" => {
            let low = attr("low")?.unwrap_or(min).clamp(min, max);
            attr("high")?.unwrap_or(max).clamp(low, max)
        }
        _ => attr("optimum")?.unwrap_or((min + max) / 2.0).clamp(min, max),
    })
}

/// The Document color attributes and `dir` (HTML §3.1.3, obsolete features).
pub(crate) fn define_document_reflection(def: &mut ClassDef<crate::node::Document>) {
    fn body_attr(n: &NodeData, attr: &str) -> JsResult<String> {
        let dom = n.page().dom.borrow();
        let Some(body) = dom.body(n.id).js()?.filter(|&b| is_html(&dom, b, &["body"])) else {
            return Ok(String::new());
        };
        Ok(dom.get_attribute(body, attr).js()?.unwrap_or("").to_owned())
    }
    fn set_body_attr(n: &NodeData, attr: &str, value: &str) -> JsResult<()> {
        let mut dom = n.page().dom.borrow_mut();
        let body = dom.body(n.id).js()?.filter(|&b| is_html(&dom, b, &["body"]));
        match body {
            Some(body) => dom.set_attribute(body, attr, value).js(),
            None => Ok(()),
        }
    }
    macro_rules! color {
        ($prop:literal, $attr:literal) => {
            def.accessor(
                $prop,
                |s, n| Ok(s.string(&body_attr(n, $attr)?)),
                |_s, n, v| set_body_attr(n, $attr, &null_to_empty(v)?),
            );
        };
    }
    color!("fgColor", "text");
    color!("linkColor", "link");
    color!("vlinkColor", "vlink");
    color!("alinkColor", "alink");
    color!("bgColor", "bgcolor");
    def.accessor(
        "dir",
        |s, n| {
            let dom = n.page().dom.borrow();
            let root = dom.document_element(n.id).js()?.filter(|&r| is_html(&dom, r, &["html"]));
            let value = root.map(|r| dom.get_attribute(r, "dir")).transpose().js()?.flatten();
            Ok(s.string(DIR.state(value)))
        },
        |_s, n, v| {
            let value = dom_string(v)?;
            let mut dom = n.page().dom.borrow_mut();
            let root = dom.document_element(n.id).js()?.filter(|&r| is_html(&dom, r, &["html"]));
            match root {
                Some(root) => dom.set_attribute(root, "dir", &value).js(),
                None => Ok(()),
            }
        },
    );
}

// ----- class selection --------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElClass {
    Html,
    Head,
    Body,
    Title,
    Meta,
    Link,
    Style,
    Script,
    Div,
    Span,
    Paragraph,
    Heading,
    Anchor,
    Image,
    Input,
    TextArea,
    Select,
    Option,
    OptGroup,
    Button,
    Form,
    Label,
    FieldSet,
    Legend,
    UList,
    OList,
    Li,
    Table,
    TableSection,
    TableRow,
    TableCell,
    TableCaption,
    TableCol,
    IFrame,
    Template,
    Slot,
    Canvas,
    Video,
    Audio,
    Br,
    Hr,
    Pre,
    Quote,
    DList,
    Dialog,
    Details,
    Progress,
    Meter,
    Output,
    DataList,
    Picture,
    Source,
    Time,
    Data,
    Base,
    Object,
    Embed,
    Area,
    Map,
    Mod,
    Menu,
    Marquee,
    FrameSet,
    Frame,
    Font,
    Directory,
    Param,
    Track,
    Unknown,
    Generic,
    Svg,
    SvgGraphics,
    SvgImage,
    SvgForeignObject,
    SvgUse,
    SvgText,
    SvgTSpan,
    SvgTextPath,
    SvgA,
    SvgRoot,
    SvgSymbol,
    SvgView,
    SvgClipPath,
    SvgMask,
    SvgPattern,
    SvgLinearGradient,
    SvgRadialGradient,
    SvgFilter,
    SvgMarker,
    SvgStop,
    SvgPath,
    SvgPolyline,
    SvgPolygon,
    SvgRect,
    SvgCircle,
    SvgEllipse,
    SvgLine,
    SvgFeBlend,
    SvgFeColorMatrix,
    SvgFeComponentTransfer,
    SvgFeFuncR,
    SvgFeFuncG,
    SvgFeFuncB,
    SvgFeFuncA,
    SvgFeComposite,
    SvgFeConvolveMatrix,
    SvgFeDiffuseLighting,
    SvgFeDistantLight,
    SvgFePointLight,
    SvgFeSpotLight,
    SvgFeDisplacementMap,
    SvgFeDropShadow,
    SvgFeFlood,
    SvgFeGaussianBlur,
    SvgFeImage,
    SvgFeMerge,
    SvgFeMergeNode,
    SvgFeMorphology,
    SvgFeOffset,
    SvgFeSpecularLighting,
    SvgFeTile,
    SvgFeTurbulence,
    MathMl,
    Other,
}

pub fn class_for(dom: &Dom, id: NodeId) -> ElClass {
    let Ok(e) = dom.element(id) else { return ElClass::Other };
    if e.namespace() == &markup5ever::ns!(svg) {
        return match &*e.name.local {
            "svg" => ElClass::SvgRoot,
            "symbol" => ElClass::SvgSymbol,
            "view" => ElClass::SvgView,
            "a" => ElClass::SvgA,
            // SVGGraphicsElement interfaces (SVG 2).
            "stop" => ElClass::SvgStop,
            "path" => ElClass::SvgPath,
            "polyline" => ElClass::SvgPolyline,
            "polygon" => ElClass::SvgPolygon,
            "rect" => ElClass::SvgRect,
            "circle" => ElClass::SvgCircle,
            "ellipse" => ElClass::SvgEllipse,
            "line" => ElClass::SvgLine,
            "g" | "switch" | "defs" => ElClass::SvgGraphics,
            "image" => ElClass::SvgImage,
            "foreignObject" => ElClass::SvgForeignObject,
            "use" => ElClass::SvgUse,
            "text" => ElClass::SvgText,
            "tspan" => ElClass::SvgTSpan,
            "textPath" => ElClass::SvgTextPath,
            "clipPath" => ElClass::SvgClipPath,
            "mask" => ElClass::SvgMask,
            "pattern" => ElClass::SvgPattern,
            "linearGradient" => ElClass::SvgLinearGradient,
            "radialGradient" => ElClass::SvgRadialGradient,
            "filter" => ElClass::SvgFilter,
            "marker" => ElClass::SvgMarker,
            "feBlend" => ElClass::SvgFeBlend,
            "feColorMatrix" => ElClass::SvgFeColorMatrix,
            "feComponentTransfer" => ElClass::SvgFeComponentTransfer,
            "feFuncR" => ElClass::SvgFeFuncR,
            "feFuncG" => ElClass::SvgFeFuncG,
            "feFuncB" => ElClass::SvgFeFuncB,
            "feFuncA" => ElClass::SvgFeFuncA,
            "feComposite" => ElClass::SvgFeComposite,
            "feConvolveMatrix" => ElClass::SvgFeConvolveMatrix,
            "feDiffuseLighting" => ElClass::SvgFeDiffuseLighting,
            "feDistantLight" => ElClass::SvgFeDistantLight,
            "fePointLight" => ElClass::SvgFePointLight,
            "feSpotLight" => ElClass::SvgFeSpotLight,
            "feDisplacementMap" => ElClass::SvgFeDisplacementMap,
            "feDropShadow" => ElClass::SvgFeDropShadow,
            "feFlood" => ElClass::SvgFeFlood,
            "feGaussianBlur" => ElClass::SvgFeGaussianBlur,
            "feImage" => ElClass::SvgFeImage,
            "feMerge" => ElClass::SvgFeMerge,
            "feMergeNode" => ElClass::SvgFeMergeNode,
            "feMorphology" => ElClass::SvgFeMorphology,
            "feOffset" => ElClass::SvgFeOffset,
            "feSpecularLighting" => ElClass::SvgFeSpecularLighting,
            "feTile" => ElClass::SvgFeTile,
            "feTurbulence" => ElClass::SvgFeTurbulence,
            _ => ElClass::Svg,
        };
    }
    if e.namespace() == &markup5ever::ns!(mathml) {
        return ElClass::MathMl;
    }
    if !e.is_html() {
        return ElClass::Other;
    }
    html_class_for_local(&e.name.local)
}

/// The interface class of an HTML element named `local` (HTML "element interface").
pub fn html_class_for_local(local: &str) -> ElClass {
    use ElClass::*;
    match local {
        "html" => Html,
        "head" => Head,
        "body" => Body,
        "title" => Title,
        "meta" => Meta,
        "link" => Link,
        "style" => Style,
        "script" => Script,
        "div" => Div,
        "span" => Span,
        "p" => Paragraph,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Heading,
        "a" => Anchor,
        "img" => Image,
        "input" => Input,
        "textarea" => TextArea,
        "select" => Select,
        "option" => Option,
        "optgroup" => OptGroup,
        "button" => Button,
        "form" => Form,
        "label" => Label,
        "fieldset" => FieldSet,
        "legend" => Legend,
        "ul" => UList,
        "ol" => OList,
        "li" => Li,
        "table" => Table,
        "thead" | "tbody" | "tfoot" => TableSection,
        "tr" => TableRow,
        "td" | "th" => TableCell,
        "caption" => TableCaption,
        "col" | "colgroup" => TableCol,
        "iframe" => IFrame,
        "template" => Template,
        "slot" => Slot,
        "canvas" => Canvas,
        "video" => Video,
        "audio" => Audio,
        "br" => Br,
        "hr" => Hr,
        "pre" | "listing" | "xmp" => Pre,
        "blockquote" | "q" => Quote,
        "dl" => DList,
        "dialog" => Dialog,
        "details" => Details,
        "progress" => Progress,
        "meter" => Meter,
        "output" => Output,
        "datalist" => DataList,
        "picture" => Picture,
        "source" => Source,
        "time" => Time,
        "data" => Data,
        "base" => Base,
        "object" => Object,
        "embed" => Embed,
        "area" => Area,
        "map" => Map,
        "ins" | "del" => Mod,
        "menu" => Menu,
        "marquee" => Marquee,
        "frameset" => FrameSet,
        "frame" => Frame,
        "font" => Font,
        "dir" => Directory,
        "param" => Param,
        "track" => Track,
        "applet" | "bgsound" | "blink" | "isindex" | "keygen" | "multicol" | "nextid" | "spacer" => Unknown,
        "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code" | "dd" | "dfn" | "dt"
        | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i" | "kbd" | "main" | "mark" | "nav"
        | "noscript" | "rp" | "rt" | "ruby" | "s" | "samp" | "search" | "section" | "small" | "strong" | "sub"
        | "summary" | "sup" | "u" | "var" | "wbr" | "center" | "nobr" | "noembed" | "noframes" | "plaintext"
        | "big" | "strike" | "tt" | "acronym" | "basefont" => Generic,
        l if l.contains('-') => Generic,
        _ => Unknown,
    }
}

pub fn wrap_element<'s>(s: &Scope<'s>, class: ElClass, d: NodeData, k: Option<IdentityKey>) -> JsResult<Object<'s>> {
    use ElClass::*;
    match class {
        Html => s.wrap::<HtmlHtmlElement>(d, k),
        Head => s.wrap::<HtmlHeadElement>(d, k),
        Body => s.wrap::<HtmlBodyElement>(d, k),
        Title => s.wrap::<HtmlTitleElement>(d, k),
        Meta => s.wrap::<HtmlMetaElement>(d, k),
        Link => s.wrap::<HtmlLinkElement>(d, k),
        Style => s.wrap::<HtmlStyleElement>(d, k),
        Script => s.wrap::<HtmlScriptElement>(d, k),
        Div => s.wrap::<HtmlDivElement>(d, k),
        Span => s.wrap::<HtmlSpanElement>(d, k),
        Paragraph => s.wrap::<HtmlParagraphElement>(d, k),
        Heading => s.wrap::<HtmlHeadingElement>(d, k),
        Anchor => s.wrap::<HtmlAnchorElement>(d, k),
        Image => s.wrap::<HtmlImageElement>(d, k),
        Input => s.wrap::<HtmlInputElement>(d, k),
        TextArea => s.wrap::<HtmlTextAreaElement>(d, k),
        Select => s.wrap::<HtmlSelectElement>(d, k),
        Option => s.wrap::<HtmlOptionElement>(d, k),
        OptGroup => s.wrap::<HtmlOptGroupElement>(d, k),
        Button => s.wrap::<HtmlButtonElement>(d, k),
        Form => s.wrap::<HtmlFormElement>(d, k),
        Label => s.wrap::<HtmlLabelElement>(d, k),
        FieldSet => s.wrap::<HtmlFieldSetElement>(d, k),
        Legend => s.wrap::<HtmlLegendElement>(d, k),
        UList => s.wrap::<HtmlUListElement>(d, k),
        OList => s.wrap::<HtmlOListElement>(d, k),
        Li => s.wrap::<HtmlLiElement>(d, k),
        Table => s.wrap::<HtmlTableElement>(d, k),
        TableSection => s.wrap::<HtmlTableSectionElement>(d, k),
        TableRow => s.wrap::<HtmlTableRowElement>(d, k),
        TableCell => s.wrap::<HtmlTableCellElement>(d, k),
        TableCaption => s.wrap::<HtmlTableCaptionElement>(d, k),
        TableCol => s.wrap::<HtmlTableColElement>(d, k),
        IFrame => s.wrap::<HtmlIFrameElement>(d, k),
        Template => s.wrap::<HtmlTemplateElement>(d, k),
        Slot => s.wrap::<HtmlSlotElement>(d, k),
        Canvas => s.wrap::<HtmlCanvasElement>(d, k),
        Video => s.wrap::<HtmlVideoElement>(d, k),
        Audio => s.wrap::<HtmlAudioElement>(d, k),
        Br => s.wrap::<HtmlBrElement>(d, k),
        Hr => s.wrap::<HtmlHrElement>(d, k),
        Pre => s.wrap::<HtmlPreElement>(d, k),
        Quote => s.wrap::<HtmlQuoteElement>(d, k),
        DList => s.wrap::<HtmlDListElement>(d, k),
        Dialog => s.wrap::<HtmlDialogElement>(d, k),
        Details => s.wrap::<HtmlDetailsElement>(d, k),
        Progress => s.wrap::<HtmlProgressElement>(d, k),
        Meter => s.wrap::<HtmlMeterElement>(d, k),
        Output => s.wrap::<HtmlOutputElement>(d, k),
        DataList => s.wrap::<HtmlDataListElement>(d, k),
        Picture => s.wrap::<HtmlPictureElement>(d, k),
        Source => s.wrap::<HtmlSourceElement>(d, k),
        Time => s.wrap::<HtmlTimeElement>(d, k),
        Data => s.wrap::<HtmlDataElement>(d, k),
        Base => s.wrap::<HtmlBaseElement>(d, k),
        Object => s.wrap::<HtmlObjectElement>(d, k),
        Embed => s.wrap::<HtmlEmbedElement>(d, k),
        Area => s.wrap::<HtmlAreaElement>(d, k),
        Map => s.wrap::<HtmlMapElement>(d, k),
        Mod => s.wrap::<HtmlModElement>(d, k),
        Menu => s.wrap::<HtmlMenuElement>(d, k),
        Marquee => s.wrap::<HtmlMarqueeElement>(d, k),
        FrameSet => s.wrap::<HtmlFrameSetElement>(d, k),
        Frame => s.wrap::<HtmlFrameElement>(d, k),
        Font => s.wrap::<HtmlFontElement>(d, k),
        Directory => s.wrap::<HtmlDirectoryElement>(d, k),
        Param => s.wrap::<HtmlParamElement>(d, k),
        Track => s.wrap::<HtmlTrackElement>(d, k),
        Unknown => s.wrap::<HtmlUnknownElement>(d, k),
        Generic => s.wrap::<HtmlElement>(d, k),
        Svg => s.wrap::<SvgElement>(d, k),
        SvgGraphics => s.wrap::<SvgGraphicsElement>(d, k),
        SvgImage => s.wrap::<SvgImageElement>(d, k),
        SvgForeignObject => s.wrap::<SvgForeignObjectElement>(d, k),
        SvgUse => s.wrap::<SvgUseElement>(d, k),
        SvgText => s.wrap::<SvgTextElement>(d, k),
        SvgTSpan => s.wrap::<SvgTSpanElement>(d, k),
        SvgTextPath => s.wrap::<SvgTextPathElement>(d, k),
        SvgA => s.wrap::<SvgAElement>(d, k),
        SvgRoot => s.wrap::<SvgSvgElement>(d, k),
        SvgSymbol => s.wrap::<SvgSymbolElement>(d, k),
        SvgView => s.wrap::<SvgViewElement>(d, k),
        SvgClipPath => s.wrap::<SvgClipPathElement>(d, k),
        SvgMask => s.wrap::<SvgMaskElement>(d, k),
        SvgPattern => s.wrap::<SvgPatternElement>(d, k),
        SvgLinearGradient => s.wrap::<SvgLinearGradientElement>(d, k),
        SvgRadialGradient => s.wrap::<SvgRadialGradientElement>(d, k),
        SvgFilter => s.wrap::<SvgFilterElement>(d, k),
        SvgMarker => s.wrap::<SvgMarkerElement>(d, k),
        SvgStop => s.wrap::<SvgStopElement>(d, k),
        SvgPath => s.wrap::<SvgPathElement>(d, k),
        SvgPolyline => s.wrap::<SvgPolylineElement>(d, k),
        SvgPolygon => s.wrap::<SvgPolygonElement>(d, k),
        SvgRect => s.wrap::<SvgRectElement>(d, k),
        SvgCircle => s.wrap::<SvgCircleElement>(d, k),
        SvgEllipse => s.wrap::<SvgEllipseElement>(d, k),
        SvgLine => s.wrap::<SvgLineElement>(d, k),
        SvgFeBlend => s.wrap::<SvgFeBlendElement>(d, k),
        SvgFeColorMatrix => s.wrap::<SvgFeColorMatrixElement>(d, k),
        SvgFeComponentTransfer => s.wrap::<SvgFeComponentTransferElement>(d, k),
        SvgFeFuncR => s.wrap::<SvgFeFuncRElement>(d, k),
        SvgFeFuncG => s.wrap::<SvgFeFuncGElement>(d, k),
        SvgFeFuncB => s.wrap::<SvgFeFuncBElement>(d, k),
        SvgFeFuncA => s.wrap::<SvgFeFuncAElement>(d, k),
        SvgFeComposite => s.wrap::<SvgFeCompositeElement>(d, k),
        SvgFeConvolveMatrix => s.wrap::<SvgFeConvolveMatrixElement>(d, k),
        SvgFeDiffuseLighting => s.wrap::<SvgFeDiffuseLightingElement>(d, k),
        SvgFeDistantLight => s.wrap::<SvgFeDistantLightElement>(d, k),
        SvgFePointLight => s.wrap::<SvgFePointLightElement>(d, k),
        SvgFeSpotLight => s.wrap::<SvgFeSpotLightElement>(d, k),
        SvgFeDisplacementMap => s.wrap::<SvgFeDisplacementMapElement>(d, k),
        SvgFeDropShadow => s.wrap::<SvgFeDropShadowElement>(d, k),
        SvgFeFlood => s.wrap::<SvgFeFloodElement>(d, k),
        SvgFeGaussianBlur => s.wrap::<SvgFeGaussianBlurElement>(d, k),
        SvgFeImage => s.wrap::<SvgFeImageElement>(d, k),
        SvgFeMerge => s.wrap::<SvgFeMergeElement>(d, k),
        SvgFeMergeNode => s.wrap::<SvgFeMergeNodeElement>(d, k),
        SvgFeMorphology => s.wrap::<SvgFeMorphologyElement>(d, k),
        SvgFeOffset => s.wrap::<SvgFeOffsetElement>(d, k),
        SvgFeSpecularLighting => s.wrap::<SvgFeSpecularLightingElement>(d, k),
        SvgFeTile => s.wrap::<SvgFeTileElement>(d, k),
        SvgFeTurbulence => s.wrap::<SvgFeTurbulenceElement>(d, k),
        MathMl => s.wrap::<MathMlElement>(d, k),
        Other => s.wrap::<Element>(d, k),
    }
}


/// The name of the interface of `class` (`HTMLButtonElement`, ...).
pub fn interface_name(class: ElClass) -> &'static str {
    use ElClass::*;
    match class {
        Html => HtmlHtmlElement::NAME,
        Head => HtmlHeadElement::NAME,
        Body => HtmlBodyElement::NAME,
        Title => HtmlTitleElement::NAME,
        Meta => HtmlMetaElement::NAME,
        Link => HtmlLinkElement::NAME,
        Style => HtmlStyleElement::NAME,
        Script => HtmlScriptElement::NAME,
        Div => HtmlDivElement::NAME,
        Span => HtmlSpanElement::NAME,
        Paragraph => HtmlParagraphElement::NAME,
        Heading => HtmlHeadingElement::NAME,
        Anchor => HtmlAnchorElement::NAME,
        Image => HtmlImageElement::NAME,
        Input => HtmlInputElement::NAME,
        TextArea => HtmlTextAreaElement::NAME,
        Select => HtmlSelectElement::NAME,
        Option => HtmlOptionElement::NAME,
        OptGroup => HtmlOptGroupElement::NAME,
        Button => HtmlButtonElement::NAME,
        Form => HtmlFormElement::NAME,
        Label => HtmlLabelElement::NAME,
        FieldSet => HtmlFieldSetElement::NAME,
        Legend => HtmlLegendElement::NAME,
        UList => HtmlUListElement::NAME,
        OList => HtmlOListElement::NAME,
        Li => HtmlLiElement::NAME,
        Table => HtmlTableElement::NAME,
        TableSection => HtmlTableSectionElement::NAME,
        TableRow => HtmlTableRowElement::NAME,
        TableCell => HtmlTableCellElement::NAME,
        TableCaption => HtmlTableCaptionElement::NAME,
        TableCol => HtmlTableColElement::NAME,
        IFrame => HtmlIFrameElement::NAME,
        Template => HtmlTemplateElement::NAME,
        Slot => HtmlSlotElement::NAME,
        Canvas => HtmlCanvasElement::NAME,
        Video => HtmlVideoElement::NAME,
        Audio => HtmlAudioElement::NAME,
        Br => HtmlBrElement::NAME,
        Hr => HtmlHrElement::NAME,
        Pre => HtmlPreElement::NAME,
        Quote => HtmlQuoteElement::NAME,
        DList => HtmlDListElement::NAME,
        Dialog => HtmlDialogElement::NAME,
        Details => HtmlDetailsElement::NAME,
        Progress => HtmlProgressElement::NAME,
        Meter => HtmlMeterElement::NAME,
        Output => HtmlOutputElement::NAME,
        DataList => HtmlDataListElement::NAME,
        Picture => HtmlPictureElement::NAME,
        Source => HtmlSourceElement::NAME,
        Time => HtmlTimeElement::NAME,
        Data => HtmlDataElement::NAME,
        Base => HtmlBaseElement::NAME,
        Object => HtmlObjectElement::NAME,
        Embed => HtmlEmbedElement::NAME,
        Area => HtmlAreaElement::NAME,
        Map => HtmlMapElement::NAME,
        Mod => HtmlModElement::NAME,
        Menu => HtmlMenuElement::NAME,
        Marquee => HtmlMarqueeElement::NAME,
        FrameSet => HtmlFrameSetElement::NAME,
        Frame => HtmlFrameElement::NAME,
        Font => HtmlFontElement::NAME,
        Directory => HtmlDirectoryElement::NAME,
        Param => HtmlParamElement::NAME,
        Track => HtmlTrackElement::NAME,
        Unknown => HtmlUnknownElement::NAME,
        Generic => HtmlElement::NAME,
        Svg => SvgElement::NAME,
        SvgGraphics => SvgGraphicsElement::NAME,
        SvgImage => SvgImageElement::NAME,
        SvgForeignObject => SvgForeignObjectElement::NAME,
        SvgUse => SvgUseElement::NAME,
        SvgText => SvgTextElement::NAME,
        SvgTSpan => SvgTSpanElement::NAME,
        SvgTextPath => SvgTextPathElement::NAME,
        SvgA => SvgAElement::NAME,
        SvgRoot => SvgSvgElement::NAME,
        SvgSymbol => SvgSymbolElement::NAME,
        SvgView => SvgViewElement::NAME,
        SvgClipPath => SvgClipPathElement::NAME,
        SvgMask => SvgMaskElement::NAME,
        SvgPattern => SvgPatternElement::NAME,
        SvgLinearGradient => SvgLinearGradientElement::NAME,
        SvgRadialGradient => SvgRadialGradientElement::NAME,
        SvgFilter => SvgFilterElement::NAME,
        SvgMarker => SvgMarkerElement::NAME,
        SvgStop => SvgStopElement::NAME,
        SvgPath => SvgPathElement::NAME,
        SvgPolyline => SvgPolylineElement::NAME,
        SvgPolygon => SvgPolygonElement::NAME,
        SvgRect => SvgRectElement::NAME,
        SvgCircle => SvgCircleElement::NAME,
        SvgEllipse => SvgEllipseElement::NAME,
        SvgLine => SvgLineElement::NAME,
        SvgFeBlend => SvgFeBlendElement::NAME,
        SvgFeColorMatrix => SvgFeColorMatrixElement::NAME,
        SvgFeComponentTransfer => SvgFeComponentTransferElement::NAME,
        SvgFeFuncR => SvgFeFuncRElement::NAME,
        SvgFeFuncG => SvgFeFuncGElement::NAME,
        SvgFeFuncB => SvgFeFuncBElement::NAME,
        SvgFeFuncA => SvgFeFuncAElement::NAME,
        SvgFeComposite => SvgFeCompositeElement::NAME,
        SvgFeConvolveMatrix => SvgFeConvolveMatrixElement::NAME,
        SvgFeDiffuseLighting => SvgFeDiffuseLightingElement::NAME,
        SvgFeDistantLight => SvgFeDistantLightElement::NAME,
        SvgFePointLight => SvgFePointLightElement::NAME,
        SvgFeSpotLight => SvgFeSpotLightElement::NAME,
        SvgFeDisplacementMap => SvgFeDisplacementMapElement::NAME,
        SvgFeDropShadow => SvgFeDropShadowElement::NAME,
        SvgFeFlood => SvgFeFloodElement::NAME,
        SvgFeGaussianBlur => SvgFeGaussianBlurElement::NAME,
        SvgFeImage => SvgFeImageElement::NAME,
        SvgFeMerge => SvgFeMergeElement::NAME,
        SvgFeMergeNode => SvgFeMergeNodeElement::NAME,
        SvgFeMorphology => SvgFeMorphologyElement::NAME,
        SvgFeOffset => SvgFeOffsetElement::NAME,
        SvgFeSpecularLighting => SvgFeSpecularLightingElement::NAME,
        SvgFeTile => SvgFeTileElement::NAME,
        SvgFeTurbulence => SvgFeTurbulenceElement::NAME,
        MathMl => MathMlElement::NAME,
        Other => Element::NAME,
    }
}

/// The interface name of an HTML element named `local`.
pub fn interface_name_for_html(local: &str) -> &'static str {
    interface_name(html_class_for_local(local))
}

/// Whether `local` names an HTML element whose interface is not `HTMLUnknownElement` (what
/// `customElements.define`'s `extends` may name).
pub fn is_known_html_element(local: &str) -> bool {
    !matches!(html_class_for_local(local), ElClass::Unknown)
}

/// The wrapper of `el`, made by `new C()` of a customized built-in element: its element
/// interface with `new_target`'s prototype.
pub(crate) fn wrap_new_html_element<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: NodeId,
    new_target: Object<'s>,
) -> JsResult<Object<'s>> {
    let class = class_for(&page.dom.borrow(), el);
    use ElClass::*;
    match class {
        Html => crate::node::wrap_new_node::<HtmlHtmlElement>(s, page, el, new_target),
        Head => crate::node::wrap_new_node::<HtmlHeadElement>(s, page, el, new_target),
        Body => crate::node::wrap_new_node::<HtmlBodyElement>(s, page, el, new_target),
        Title => crate::node::wrap_new_node::<HtmlTitleElement>(s, page, el, new_target),
        Meta => crate::node::wrap_new_node::<HtmlMetaElement>(s, page, el, new_target),
        Link => crate::node::wrap_new_node::<HtmlLinkElement>(s, page, el, new_target),
        Style => crate::node::wrap_new_node::<HtmlStyleElement>(s, page, el, new_target),
        Script => crate::node::wrap_new_node::<HtmlScriptElement>(s, page, el, new_target),
        Div => crate::node::wrap_new_node::<HtmlDivElement>(s, page, el, new_target),
        Span => crate::node::wrap_new_node::<HtmlSpanElement>(s, page, el, new_target),
        Paragraph => crate::node::wrap_new_node::<HtmlParagraphElement>(s, page, el, new_target),
        Heading => crate::node::wrap_new_node::<HtmlHeadingElement>(s, page, el, new_target),
        Anchor => crate::node::wrap_new_node::<HtmlAnchorElement>(s, page, el, new_target),
        Image => crate::node::wrap_new_node::<HtmlImageElement>(s, page, el, new_target),
        Input => crate::node::wrap_new_node::<HtmlInputElement>(s, page, el, new_target),
        TextArea => crate::node::wrap_new_node::<HtmlTextAreaElement>(s, page, el, new_target),
        Select => crate::node::wrap_new_node::<HtmlSelectElement>(s, page, el, new_target),
        Option => crate::node::wrap_new_node::<HtmlOptionElement>(s, page, el, new_target),
        OptGroup => crate::node::wrap_new_node::<HtmlOptGroupElement>(s, page, el, new_target),
        Button => crate::node::wrap_new_node::<HtmlButtonElement>(s, page, el, new_target),
        Form => crate::node::wrap_new_node::<HtmlFormElement>(s, page, el, new_target),
        Label => crate::node::wrap_new_node::<HtmlLabelElement>(s, page, el, new_target),
        FieldSet => crate::node::wrap_new_node::<HtmlFieldSetElement>(s, page, el, new_target),
        Legend => crate::node::wrap_new_node::<HtmlLegendElement>(s, page, el, new_target),
        UList => crate::node::wrap_new_node::<HtmlUListElement>(s, page, el, new_target),
        OList => crate::node::wrap_new_node::<HtmlOListElement>(s, page, el, new_target),
        Li => crate::node::wrap_new_node::<HtmlLiElement>(s, page, el, new_target),
        Table => crate::node::wrap_new_node::<HtmlTableElement>(s, page, el, new_target),
        TableSection => crate::node::wrap_new_node::<HtmlTableSectionElement>(s, page, el, new_target),
        TableRow => crate::node::wrap_new_node::<HtmlTableRowElement>(s, page, el, new_target),
        TableCell => crate::node::wrap_new_node::<HtmlTableCellElement>(s, page, el, new_target),
        TableCaption => crate::node::wrap_new_node::<HtmlTableCaptionElement>(s, page, el, new_target),
        TableCol => crate::node::wrap_new_node::<HtmlTableColElement>(s, page, el, new_target),
        IFrame => crate::node::wrap_new_node::<HtmlIFrameElement>(s, page, el, new_target),
        Template => crate::node::wrap_new_node::<HtmlTemplateElement>(s, page, el, new_target),
        Slot => crate::node::wrap_new_node::<HtmlSlotElement>(s, page, el, new_target),
        Canvas => crate::node::wrap_new_node::<HtmlCanvasElement>(s, page, el, new_target),
        Video => crate::node::wrap_new_node::<HtmlVideoElement>(s, page, el, new_target),
        Audio => crate::node::wrap_new_node::<HtmlAudioElement>(s, page, el, new_target),
        Br => crate::node::wrap_new_node::<HtmlBrElement>(s, page, el, new_target),
        Hr => crate::node::wrap_new_node::<HtmlHrElement>(s, page, el, new_target),
        Pre => crate::node::wrap_new_node::<HtmlPreElement>(s, page, el, new_target),
        Quote => crate::node::wrap_new_node::<HtmlQuoteElement>(s, page, el, new_target),
        DList => crate::node::wrap_new_node::<HtmlDListElement>(s, page, el, new_target),
        Dialog => crate::node::wrap_new_node::<HtmlDialogElement>(s, page, el, new_target),
        Details => crate::node::wrap_new_node::<HtmlDetailsElement>(s, page, el, new_target),
        Progress => crate::node::wrap_new_node::<HtmlProgressElement>(s, page, el, new_target),
        Meter => crate::node::wrap_new_node::<HtmlMeterElement>(s, page, el, new_target),
        Output => crate::node::wrap_new_node::<HtmlOutputElement>(s, page, el, new_target),
        DataList => crate::node::wrap_new_node::<HtmlDataListElement>(s, page, el, new_target),
        Picture => crate::node::wrap_new_node::<HtmlPictureElement>(s, page, el, new_target),
        Source => crate::node::wrap_new_node::<HtmlSourceElement>(s, page, el, new_target),
        Time => crate::node::wrap_new_node::<HtmlTimeElement>(s, page, el, new_target),
        Data => crate::node::wrap_new_node::<HtmlDataElement>(s, page, el, new_target),
        Base => crate::node::wrap_new_node::<HtmlBaseElement>(s, page, el, new_target),
        Object => crate::node::wrap_new_node::<HtmlObjectElement>(s, page, el, new_target),
        Embed => crate::node::wrap_new_node::<HtmlEmbedElement>(s, page, el, new_target),
        Area => crate::node::wrap_new_node::<HtmlAreaElement>(s, page, el, new_target),
        Map => crate::node::wrap_new_node::<HtmlMapElement>(s, page, el, new_target),
        Mod => crate::node::wrap_new_node::<HtmlModElement>(s, page, el, new_target),
        Menu => crate::node::wrap_new_node::<HtmlMenuElement>(s, page, el, new_target),
        Marquee => crate::node::wrap_new_node::<HtmlMarqueeElement>(s, page, el, new_target),
        FrameSet => crate::node::wrap_new_node::<HtmlFrameSetElement>(s, page, el, new_target),
        Frame => crate::node::wrap_new_node::<HtmlFrameElement>(s, page, el, new_target),
        Font => crate::node::wrap_new_node::<HtmlFontElement>(s, page, el, new_target),
        Directory => crate::node::wrap_new_node::<HtmlDirectoryElement>(s, page, el, new_target),
        Param => crate::node::wrap_new_node::<HtmlParamElement>(s, page, el, new_target),
        Track => crate::node::wrap_new_node::<HtmlTrackElement>(s, page, el, new_target),
        Unknown => crate::node::wrap_new_node::<HtmlUnknownElement>(s, page, el, new_target),
        Generic => crate::node::wrap_new_node::<HtmlElement>(s, page, el, new_target),
        Svg => crate::node::wrap_new_node::<SvgElement>(s, page, el, new_target),
        SvgGraphics => crate::node::wrap_new_node::<SvgGraphicsElement>(s, page, el, new_target),
        SvgImage => crate::node::wrap_new_node::<SvgImageElement>(s, page, el, new_target),
        SvgForeignObject => crate::node::wrap_new_node::<SvgForeignObjectElement>(s, page, el, new_target),
        SvgUse => crate::node::wrap_new_node::<SvgUseElement>(s, page, el, new_target),
        SvgText => crate::node::wrap_new_node::<SvgTextElement>(s, page, el, new_target),
        SvgTSpan => crate::node::wrap_new_node::<SvgTSpanElement>(s, page, el, new_target),
        SvgTextPath => crate::node::wrap_new_node::<SvgTextPathElement>(s, page, el, new_target),
        SvgA => crate::node::wrap_new_node::<SvgAElement>(s, page, el, new_target),
        SvgRoot => crate::node::wrap_new_node::<SvgSvgElement>(s, page, el, new_target),
        SvgSymbol => crate::node::wrap_new_node::<SvgSymbolElement>(s, page, el, new_target),
        SvgView => crate::node::wrap_new_node::<SvgViewElement>(s, page, el, new_target),
        SvgClipPath => crate::node::wrap_new_node::<SvgClipPathElement>(s, page, el, new_target),
        SvgMask => crate::node::wrap_new_node::<SvgMaskElement>(s, page, el, new_target),
        SvgPattern => crate::node::wrap_new_node::<SvgPatternElement>(s, page, el, new_target),
        SvgLinearGradient => crate::node::wrap_new_node::<SvgLinearGradientElement>(s, page, el, new_target),
        SvgRadialGradient => crate::node::wrap_new_node::<SvgRadialGradientElement>(s, page, el, new_target),
        SvgFilter => crate::node::wrap_new_node::<SvgFilterElement>(s, page, el, new_target),
        SvgMarker => crate::node::wrap_new_node::<SvgMarkerElement>(s, page, el, new_target),
        SvgStop => crate::node::wrap_new_node::<SvgStopElement>(s, page, el, new_target),
        SvgPath => crate::node::wrap_new_node::<SvgPathElement>(s, page, el, new_target),
        SvgPolyline => crate::node::wrap_new_node::<SvgPolylineElement>(s, page, el, new_target),
        SvgPolygon => crate::node::wrap_new_node::<SvgPolygonElement>(s, page, el, new_target),
        SvgRect => crate::node::wrap_new_node::<SvgRectElement>(s, page, el, new_target),
        SvgCircle => crate::node::wrap_new_node::<SvgCircleElement>(s, page, el, new_target),
        SvgEllipse => crate::node::wrap_new_node::<SvgEllipseElement>(s, page, el, new_target),
        SvgLine => crate::node::wrap_new_node::<SvgLineElement>(s, page, el, new_target),
        SvgFeBlend => crate::node::wrap_new_node::<SvgFeBlendElement>(s, page, el, new_target),
        SvgFeColorMatrix => crate::node::wrap_new_node::<SvgFeColorMatrixElement>(s, page, el, new_target),
        SvgFeComponentTransfer => crate::node::wrap_new_node::<SvgFeComponentTransferElement>(s, page, el, new_target),
        SvgFeFuncR => crate::node::wrap_new_node::<SvgFeFuncRElement>(s, page, el, new_target),
        SvgFeFuncG => crate::node::wrap_new_node::<SvgFeFuncGElement>(s, page, el, new_target),
        SvgFeFuncB => crate::node::wrap_new_node::<SvgFeFuncBElement>(s, page, el, new_target),
        SvgFeFuncA => crate::node::wrap_new_node::<SvgFeFuncAElement>(s, page, el, new_target),
        SvgFeComposite => crate::node::wrap_new_node::<SvgFeCompositeElement>(s, page, el, new_target),
        SvgFeConvolveMatrix => crate::node::wrap_new_node::<SvgFeConvolveMatrixElement>(s, page, el, new_target),
        SvgFeDiffuseLighting => crate::node::wrap_new_node::<SvgFeDiffuseLightingElement>(s, page, el, new_target),
        SvgFeDistantLight => crate::node::wrap_new_node::<SvgFeDistantLightElement>(s, page, el, new_target),
        SvgFePointLight => crate::node::wrap_new_node::<SvgFePointLightElement>(s, page, el, new_target),
        SvgFeSpotLight => crate::node::wrap_new_node::<SvgFeSpotLightElement>(s, page, el, new_target),
        SvgFeDisplacementMap => crate::node::wrap_new_node::<SvgFeDisplacementMapElement>(s, page, el, new_target),
        SvgFeDropShadow => crate::node::wrap_new_node::<SvgFeDropShadowElement>(s, page, el, new_target),
        SvgFeFlood => crate::node::wrap_new_node::<SvgFeFloodElement>(s, page, el, new_target),
        SvgFeGaussianBlur => crate::node::wrap_new_node::<SvgFeGaussianBlurElement>(s, page, el, new_target),
        SvgFeImage => crate::node::wrap_new_node::<SvgFeImageElement>(s, page, el, new_target),
        SvgFeMerge => crate::node::wrap_new_node::<SvgFeMergeElement>(s, page, el, new_target),
        SvgFeMergeNode => crate::node::wrap_new_node::<SvgFeMergeNodeElement>(s, page, el, new_target),
        SvgFeMorphology => crate::node::wrap_new_node::<SvgFeMorphologyElement>(s, page, el, new_target),
        SvgFeOffset => crate::node::wrap_new_node::<SvgFeOffsetElement>(s, page, el, new_target),
        SvgFeSpecularLighting => crate::node::wrap_new_node::<SvgFeSpecularLightingElement>(s, page, el, new_target),
        SvgFeTile => crate::node::wrap_new_node::<SvgFeTileElement>(s, page, el, new_target),
        SvgFeTurbulence => crate::node::wrap_new_node::<SvgFeTurbulenceElement>(s, page, el, new_target),
        MathMl => crate::node::wrap_new_node::<MathMlElement>(s, page, el, new_target),
        Other => crate::node::wrap_new_node::<Element>(s, page, el, new_target),
    }
}

// ----- Element extras (layout-dependent members) --------------------------------

pub(crate) fn define_element_extras(def: &mut ClassDef<Element>) {
    def.method("getBoundingClientRect", 0, |s, n, _a| crate::layout_api::bounding_rect(s, n));
    def.method("getClientRects", 0, |s, n, _a| crate::layout_api::client_rects(s, n));
    def.method("scrollIntoView", 0, |s, n, a| crate::layout_api::scroll_into_view(s, n, a));
    def.method("scrollIntoViewIfNeeded", 0, |s, n, a| crate::layout_api::scroll_into_view_if_needed(s, n, a));
    def.method("checkVisibility", 0, |s, n, a| crate::layout_api::check_visibility(s, n, a));
    def.method("scroll", 0, |s, n, a| crate::layout_api::scroll_to(s, n, a, false));
    def.method("scrollTo", 0, |s, n, a| crate::layout_api::scroll_to(s, n, a, false));
    def.method("scrollBy", 0, |s, n, a| crate::layout_api::scroll_to(s, n, a, true));
    def.accessor(
        "scrollTop",
        |s, n| crate::layout_api::scroll_get(s, n, true),
        |s, n, v| crate::layout_api::scroll_set(s, n, v, true),
    );
    def.accessor(
        "scrollLeft",
        |s, n| crate::layout_api::scroll_get(s, n, false),
        |s, n, v| crate::layout_api::scroll_set(s, n, v, false),
    );
    def.getter("scrollWidth", |s, n| crate::layout_api::metric(s, n, "scrollWidth"));
    def.getter("scrollHeight", |s, n| crate::layout_api::metric(s, n, "scrollHeight"));
    def.getter("clientWidth", |s, n| crate::layout_api::metric(s, n, "clientWidth"));
    def.getter("clientHeight", |s, n| crate::layout_api::metric(s, n, "clientHeight"));
    def.getter("clientTop", |s, n| {
        let page = n.page();
        Ok(s.number(page.layout().client_top(&page.dom.borrow(), n.id)))
    });
    def.getter("clientLeft", |s, n| {
        let page = n.page();
        Ok(s.number(page.layout().client_left(&page.dom.borrow(), n.id)))
    });
    def.method("setPointerCapture", 1, crate::pointer_capture::set_pointer_capture);
    def.method("releasePointerCapture", 1, crate::pointer_capture::release_pointer_capture);
    def.pure_method("hasPointerCapture", 1, crate::pointer_capture::has_pointer_capture);
    def.method("animate", 1, |s, n, a| {
        let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
        crate::animations::animate(s, n.page(), el, &a)
    });
    def.method("getAnimations", 0, |s, n, a| {
        let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
        crate::animations::element_animations(s, n.page(), el, &a)
    });
    def.getter("activeViewTransition", crate::view_transition::active);
    def.method("startViewTransition", 0, |s, n, a| {
        let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
        let scope = crate::node::wrap_node(s, n.page(), crate::view_transition::scope_id(n))?.value();
        crate::view_transition::start(s, n.page(), scope, el, a.get(0))
    });
    // ARIA reflection (ARIAMixin) for the attributes Playwright/Testing
    // Library read through properties.
    reflect_aria(def);
    reflect_aria_elements(def);
}

/// ARIA element reflection state of one element's attribute (see `PageDom::aria_refs`).
#[derive(Default)]
pub(crate) struct AriaRefs {
    /// The elements an IDL setter set (the content attribute then reads `""`).
    explicit: Option<Vec<NodeId>>,
    /// The frozen array last returned, per realm, with the elements it holds.
    cache: Option<(pyro_js::RealmId, Vec<NodeId>, pyro_js::Persistent)>,
}

/// Whether `target` is a descendant of one of `el`'s shadow-including ancestors (HTML "reflect"
/// for element references): its tree is `el`'s, or that of a shadow host around `el`.
fn aria_reference_valid(dom: &Dom, el: NodeId, target: NodeId) -> bool {
    let (Ok(target_root), Ok(mut root)) = (dom.root_node(target, false), dom.root_node(el, false)) else {
        return false;
    };
    loop {
        if root == target_root {
            return true;
        }
        match dom.fragment_host(root) {
            Ok(Some(host)) => match dom.root_node(host, false) {
                Ok(r) => root = r,
                Err(_) => return false,
            },
            _ => return false,
        }
    }
}

/// The elements `attr` of `el` refers to: the ones set explicitly while the attribute still reads
/// `""`, else the first element of `el`'s tree with each id the attribute lists (`single`: the
/// whole value is one id).
fn aria_elements(page: &PageDom, el: NodeId, attr: &'static str, single: bool) -> Vec<NodeId> {
    let dom = page.dom.borrow();
    let Some(value) = dom.get_attribute(el, attr).ok().flatten().map(str::to_owned) else { return Vec::new() };
    if value.is_empty() {
        if let Some(explicit) = page.aria_refs.borrow().get(&(el, attr)).and_then(|r| r.explicit.clone()) {
            return explicit
                .into_iter()
                .filter(|&t| dom.element(t).is_ok() && aria_reference_valid(&dom, el, t))
                .collect();
        }
    }
    let Ok(root) = dom.root_node(el, false) else { return Vec::new() };
    let lookup = |id: &str| dom.get_element_by_id(root, id).ok().flatten();
    if single {
        lookup(&value).into_iter().collect()
    } else {
        value.split_ascii_whitespace().filter_map(lookup).collect()
    }
}

fn set_aria_explicit(page: &PageDom, el: NodeId, attr: &'static str, elements: Option<Vec<NodeId>>) -> JsResult<()> {
    let present = elements.is_some();
    page.aria_refs.borrow_mut().entry((el, attr)).or_default().explicit = elements;
    let mut dom = page.dom.borrow_mut();
    if present {
        dom.set_attribute(el, attr, "").js()
    } else {
        dom.remove_attribute(el, attr).js().map(|_| ())
    }
}

/// An Element argument of an element-reflecting setter.
fn aria_element_arg(s: &Scope<'_>, v: Value<'_>, page: &PageDom) -> JsResult<NodeId> {
    let id = crate::node::node_arg(s, v, "set")?;
    if page.dom.borrow().element(id).is_err() {
        return Err(JsError::Type("Failed to set the property: the value is not of type 'Element'.".into()));
    }
    Ok(id)
}

fn reflect_aria_elements(def: &mut ClassDef<Element>) {
    def.accessor(
        "ariaActiveDescendantElement",
        |s, n| {
            let page = n.page();
            let found = aria_elements(page, n.id, "aria-activedescendant", true).into_iter().next();
            node_value(s, page, found)
        },
        |s, n, v| {
            let page = n.page();
            let elements = if v.is_null() || v.is_undefined() { None } else { Some(vec![aria_element_arg(s, v, page)?]) };
            set_aria_explicit(page, n.id, "aria-activedescendant", elements)
        },
    );
    macro_rules! aria_elements {
        ($prop:literal, $attr:literal) => {
            def.accessor(
                $prop,
                |s, n| aria_elements_get(s, n.page(), n.id, $attr),
                |s, n, v| aria_elements_set(s, n.page(), n.id, $attr, v),
            );
        };
    }
    aria_elements!("ariaControlsElements", "aria-controls");
    aria_elements!("ariaDescribedByElements", "aria-describedby");
    aria_elements!("ariaDetailsElements", "aria-details");
    aria_elements!("ariaErrorMessageElements", "aria-errormessage");
    aria_elements!("ariaFlowToElements", "aria-flowto");
    aria_elements!("ariaLabelledByElements", "aria-labelledby");
    aria_elements!("ariaOwnsElements", "aria-owns");
}

/// `FrozenArray<Element>?` getter: null without the attribute; the same frozen array while the
/// elements stay the same.
fn aria_elements_get<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, attr: &'static str) -> JsResult<Value<'s>> {
    if page.dom.borrow().get_attribute(el, attr).ok().flatten().is_none() {
        return Ok(s.null());
    }
    let elements = aria_elements(page, el, attr, false);
    let realm = s.realm_id();
    if let Some((r, cached, array)) = page.aria_refs.borrow().get(&(el, attr)).and_then(|a| a.cache.as_ref()) {
        if *r == realm && *cached == elements {
            return Ok(array.get(s));
        }
    }
    let mut items = Vec::with_capacity(elements.len());
    for &e in &elements {
        items.push(crate::node::wrap_node(s, page, e)?.value());
    }
    let array = s.array(&items)?.value();
    let freeze = s.global().get_atom("Object")?.as_object().expect("Object").get_atom("freeze")?;
    let array = freeze.as_object().expect("Object.freeze").call(None, &[array])?;
    page.aria_refs.borrow_mut().entry((el, attr)).or_default().cache =
        Some((realm, elements, pyro_js::Persistent::new(s, array)));
    Ok(array)
}

fn aria_elements_set(s: &Scope<'_>, page: &Rc<PageDom>, el: NodeId, attr: &'static str, v: Value<'_>) -> JsResult<()> {
    if v.is_null() || v.is_undefined() {
        return set_aria_explicit(page, el, attr, None);
    }
    let list = v
        .as_object()
        .ok_or_else(|| JsError::Type("Failed to set the property: the value is not a sequence.".into()))?;
    let len = to_u32(list.get_atom("length")?)?;
    let mut elements = Vec::with_capacity(len as usize);
    for i in 0..len {
        elements.push(aria_element_arg(s, list.get_index(i)?, page)?);
    }
    set_aria_explicit(page, el, attr, Some(elements))
}

fn reflect_aria(def: &mut ClassDef<Element>) {
    // ARIAMixin: nullable string reflection (Chrome reflects every ARIA
    // attribute as a plain string; null or undefined removes it).
    macro_rules! aria {
        ($prop:literal, $attr:literal) => {
            def.accessor(
                $prop,
                |s, n| Ok(n.page().dom.borrow().get_attribute(n.id, $attr).js()?.map_or(s.null(), |v| s.string(v))),
                |_s, n, v| {
                    if v.is_null() || v.is_undefined() {
                        n.page().dom.borrow_mut().remove_attribute(n.id, $attr).js()?;
                        Ok(())
                    } else {
                        let value = dom_string(v)?;
                        n.page().dom.borrow_mut().set_attribute(n.id, $attr, &value).js()
                    }
                },
            );
        };
    }
    aria!("role", "role");
    aria!("ariaAtomic", "aria-atomic");
    aria!("ariaAutoComplete", "aria-autocomplete");
    aria!("ariaBrailleLabel", "aria-braillelabel");
    aria!("ariaBrailleRoleDescription", "aria-brailleroledescription");
    aria!("ariaBusy", "aria-busy");
    aria!("ariaChecked", "aria-checked");
    aria!("ariaColCount", "aria-colcount");
    aria!("ariaColIndex", "aria-colindex");
    aria!("ariaColIndexText", "aria-colindextext");
    aria!("ariaColSpan", "aria-colspan");
    aria!("ariaCurrent", "aria-current");
    aria!("ariaDescription", "aria-description");
    aria!("ariaDisabled", "aria-disabled");
    aria!("ariaExpanded", "aria-expanded");
    aria!("ariaHasPopup", "aria-haspopup");
    aria!("ariaHidden", "aria-hidden");
    aria!("ariaInvalid", "aria-invalid");
    aria!("ariaKeyShortcuts", "aria-keyshortcuts");
    aria!("ariaLabel", "aria-label");
    aria!("ariaLevel", "aria-level");
    aria!("ariaLive", "aria-live");
    aria!("ariaModal", "aria-modal");
    aria!("ariaMultiLine", "aria-multiline");
    aria!("ariaMultiSelectable", "aria-multiselectable");
    aria!("ariaOrientation", "aria-orientation");
    aria!("ariaPlaceholder", "aria-placeholder");
    aria!("ariaPosInSet", "aria-posinset");
    aria!("ariaPressed", "aria-pressed");
    aria!("ariaReadOnly", "aria-readonly");
    aria!("ariaRelevant", "aria-relevant");
    aria!("ariaRequired", "aria-required");
    aria!("ariaRoleDescription", "aria-roledescription");
    aria!("ariaRowCount", "aria-rowcount");
    aria!("ariaRowIndex", "aria-rowindex");
    aria!("ariaRowIndexText", "aria-rowindextext");
    aria!("ariaRowSpan", "aria-rowspan");
    aria!("ariaSelected", "aria-selected");
    aria!("ariaSetSize", "aria-setsize");
    aria!("ariaSort", "aria-sort");
    aria!("ariaValueMax", "aria-valuemax");
    aria!("ariaValueMin", "aria-valuemin");
    aria!("ariaValueNow", "aria-valuenow");
    aria!("ariaValueText", "aria-valuetext");
}

// ----- HTMLElement ------------------------------------------------------------

pub struct HtmlElement;
impl JsClass for HtmlElement {
    const NAME: &'static str = "HTMLElement";
    type Data = NodeData;
    type Parent = Element;
    fn define(def: &mut ClassDef<Self>) {
        crate::handlers::define_global(def);
        def.constructor(0, |s, nt, _a| crate::custom::construct_html_element(s, nt, "HTMLElement"));
        reflect_str!(def, "title", "title");
        reflect_str!(def, "lang", "lang");
        reflect_enum!(def, "dir", "dir", DIR);
        reflect_str!(def, "accessKey", "accesskey");
        reflect_enum!(def, "inputMode", "inputmode", INPUT_MODE);
        reflect_enum!(def, "enterKeyHint", "enterkeyhint", ENTER_KEY_HINT);
        // The nonce IDL attribute reads the [[CryptographicNonce]] slot; setting
        // it never touches the content attribute.
        def.accessor(
            "nonce",
            |s, n| {
                let dom = n.page().dom.borrow();
                let attr = dom.get_attribute(n.id, "nonce").js()?;
                if let Some(nonce) = n.page().nonce(n.id, attr) {
                    return Ok(s.string(&nonce));
                }
                // Connecting to a document with a header-delivered CSP emptied
                // the attribute: the slot still holds the nonce it had.
                if attr == Some("") {
                    if let Some(hidden) = dom.hidden_nonce(n.id) {
                        return Ok(s.string(&n.page().nonce(n.id, Some(hidden)).unwrap_or_else(|| hidden.to_owned())));
                    }
                }
                Ok(s.string(attr.unwrap_or("")))
            },
            |_s, n, v| {
                let value = dom_string(v)?;
                let dom = n.page().dom.borrow();
                n.page().set_nonce(n.id, dom.get_attribute(n.id, "nonce").js()?, value);
                Ok(())
            },
        );
        // `hidden` is (boolean or unrestricted double or DOMString)?: the
        // "until-found" state reads back as that string.
        def.accessor(
            "hidden",
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(match dom.get_attribute(n.id, "hidden").js()? {
                    Some(v) if v.eq_ignore_ascii_case("until-found") => s.string("until-found"),
                    Some(_) => s.boolean(true),
                    None => s.boolean(false),
                })
            },
            |_s, n, v| {
                let until_found = v.is_string() && v.to_string()?.eq_ignore_ascii_case("until-found");
                let mut dom = n.page().dom.borrow_mut();
                if until_found {
                    dom.set_attribute(n.id, "hidden", "until-found").js()
                } else if v.is_nullish() || !v.to_bool() {
                    dom.remove_attribute(n.id, "hidden").js().map(|_| ())
                } else {
                    dom.set_attribute(n.id, "hidden", "").js()
                }
            },
        );
        // HTML "computed writing suggestions value": "false" when the
        // attribute says so, or is in its default state under an element
        // whose computed value is "false".
        def.accessor(
            "writingSuggestions",
            |s, n| {
                let dom = n.page().dom.borrow();
                let mut el = Some(n.id);
                let mut value = "true";
                while let Some(e) = el {
                    match dom.get_attribute(e, "writingsuggestions").js()? {
                        Some(v) if v.eq_ignore_ascii_case("false") => {
                            value = "false";
                            break;
                        }
                        Some(v) if v.is_empty() || v.eq_ignore_ascii_case("true") => break,
                        _ => el = dom.parent_element(e).js()?,
                    }
                }
                Ok(s.string(value))
            },
            |_s, n, v| {
                let value = v.to_string()?;
                n.page().dom.borrow_mut().set_attribute(n.id, "writingsuggestions", &value).js()
            },
        );
        reflect_bool!(def, "inert", "inert");
        def.accessor(
            "popover",
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(dom.get_attribute(n.id, "popover").js()?.map_or(s.null(), |value| {
                    s.string(pyro_dom::PopoverType::from_value(value).keyword())
                }))
            },
            |_s, n, value| {
                if value.is_nullish() {
                    n.page().dom.borrow_mut().remove_attribute(n.id, "popover").js()?;
                    return Ok(());
                }
                set_attr(n, "popover", &dom_string(value)?)
            },
        );
        define_focus_attributes(def);
        def.accessor(
            "draggable",
            |s, n| Ok(s.boolean(n.page().dom.borrow().get_attribute(n.id, "draggable").js()? == Some("true"))),
            |_s, n, v| {
                n.page()
                    .dom
                    .borrow_mut()
                    .set_attribute(n.id, "draggable", if v.to_bool() { "true" } else { "false" })
                    .js()
            },
        );
        def.accessor(
            "spellcheck",
            |s, n| Ok(s.boolean(n.page().dom.borrow().get_attribute(n.id, "spellcheck").js()? != Some("false"))),
            |_s, n, v| {
                n.page()
                    .dom
                    .borrow_mut()
                    .set_attribute(n.id, "spellcheck", if v.to_bool() { "true" } else { "false" })
                    .js()
            },
        );
        def.accessor(
            "contentEditable",
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(s.string(match dom.get_attribute(n.id, "contenteditable").js()? {
                    Some("" | "true") => "true",
                    Some("false") => "false",
                    Some("plaintext-only") => "plaintext-only",
                    _ => "inherit",
                }))
            },
            |_s, n, v| {
                let v = dom_string(v)?.to_ascii_lowercase();
                let mut dom = n.page().dom.borrow_mut();
                match v.as_str() {
                    "inherit" => {
                        dom.remove_attribute(n.id, "contenteditable").js()?;
                    }
                    "true" | "false" | "plaintext-only" => dom.set_attribute(n.id, "contenteditable", &v).js()?,
                    _ => {
                        return Err(JsError::dom(
                            "SyntaxError",
                            "The value provided is not 'true', 'false', 'plaintext-only', or 'inherit'.",
                        ))
                    }
                }
                Ok(())
            },
        );
        def.getter("isContentEditable", |s, n| {
            let dom = n.page().dom.borrow();
            let mut cur = Some(n.id);
            while let Some(c) = cur {
                match dom.get_attribute(c, "contenteditable").ok().flatten() {
                    Some("" | "true" | "plaintext-only") => return Ok(s.boolean(true)),
                    Some("false") => return Ok(s.boolean(false)),
                    _ => cur = dom.parent_element(c).js()?,
                }
            }
            // `designMode` makes the body's content editable.
            Ok(s.boolean(crate::editing::in_design_mode_content(&dom, n.id)))
        });
        def.getter("dataset", |s, n| Ok(collections::dataset(s, n.page(), n.id)?.value()));
        // [PutForwards=cssText]
        def.put_forwards("style", |s, n| Ok(crate::css::inline_style(s, n.page(), n.id)?.value()), "cssText");
        // CSS Typed OM: the inline declarations as a StylePropertyMap.
        def.getter("attributeStyleMap", |s, n| {
            let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
            crate::typed_om::attribute_style_map(s, n.page(), el)
        });
        def.accessor("innerText", crate::layout_api::inner_text, crate::layout_api::set_inner_text);
        def.accessor("outerText", crate::layout_api::inner_text, crate::layout_api::set_outer_text);
        def.getter("offsetParent", crate::layout_api::offset_parent);
        def.getter("offsetTop", |s, n| crate::layout_api::metric(s, n, "offsetTop"));
        def.getter("offsetLeft", |s, n| crate::layout_api::metric(s, n, "offsetLeft"));
        def.getter("offsetWidth", |s, n| crate::layout_api::metric(s, n, "offsetWidth"));
        def.getter("offsetHeight", |s, n| crate::layout_api::metric(s, n, "offsetHeight"));
        def.method("click", 0, |s, n, _a| {
            crate::input::element_click(n.page(), n.id);
            Ok(s.undefined())
        });
        def.method("focus", 0, focus_element);
        def.method("blur", 0, |s, n, _a| {
            crate::input::blur_element(n.page(), n.id);
            Ok(s.undefined())
        });
        def.method("attachInternals", 0, |s, n, _a| crate::internals::attach(s, n.page(), n.id));
        def.method("showPopover", 0, |s, n, a| {
            let source = crate::popover::source_option(s, a.get(0))?;
            crate::popover::show_popover(n.page(), n.id, true, source)?;
            Ok(s.undefined())
        });
        def.method("hidePopover", 0, |s, n, _a| {
            crate::popover::hide_popover(n.page(), n.id, true, true, true, None)?;
            Ok(s.undefined())
        });
        def.method("togglePopover", 0, |s, n, a| {
            // `force` is a boolean or a dictionary with `force` and `source`.
            let (force, source) = match a.get(0) {
                v if v.is_undefined() => (None, None),
                v => match v.as_object() {
                    Some(options) => (
                        Some(options.get_atom("force")?).filter(|f| !f.is_undefined()).map(|f| f.to_bool()),
                        crate::popover::source_option(s, v)?,
                    ),
                    None => (Some(v.to_bool()), None),
                },
            };
            let page = n.page();
            let showing = page.dom.borrow().is_popover_showing(n.id);
            match force {
                // Already in the wanted state: only the validity errors.
                Some(want) if want == showing => {
                    crate::popover::check_popover_validity(page, n.id, showing, true)?;
                }
                _ if showing => crate::popover::hide_popover(page, n.id, true, true, true, source)?,
                _ => crate::popover::show_popover(page, n.id, true, source)?,
            }
            Ok(s.boolean(page.dom.borrow().is_popover_showing(n.id)))
        });
    }
}

simple_class!(HtmlUnknownElement, "HTMLUnknownElement", HtmlElement);
simple_class!(HtmlHtmlElement, "HTMLHtmlElement", HtmlElement, |def| {
    reflect_str!(def, "version", "version");
});
simple_class!(HtmlHeadElement, "HTMLHeadElement", HtmlElement);
simple_class!(HtmlBodyElement, "HTMLBodyElement", HtmlElement, |def| {
    crate::handlers::define_window_reflecting(def);
    reflect_str_nn!(def, "text", "text");
    reflect_str_nn!(def, "link", "link");
    reflect_str_nn!(def, "vLink", "vlink");
    reflect_str_nn!(def, "aLink", "alink");
    reflect_str_nn!(def, "bgColor", "bgcolor");
    reflect_str!(def, "background", "background");
});
simple_class!(HtmlTitleElement, "HTMLTitleElement", HtmlElement, |def| {
    def.accessor(
        "text",
        |s, n| Ok(s.string(&n.page().dom.borrow().text_content(n.id).js()?.unwrap_or_default())),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
});
simple_class!(HtmlMetaElement, "HTMLMetaElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    reflect_str!(def, "content", "content");
    reflect_str!(def, "httpEquiv", "http-equiv");
    reflect_str!(def, "media", "media");
    reflect_str!(def, "scheme", "scheme");
});
simple_class!(HtmlLinkElement, "HTMLLinkElement", HtmlElement, |def| {
    reflect_url!(def, "href", "href");
    reflect_str!(def, "rel", "rel");
    reflect_str!(def, "media", "media");
    reflect_str!(def, "type", "type");
    reflect_enum!(def, "as", "as", LINK_AS);
    reflect_enum!(def, "fetchPriority", "fetchpriority", FETCH_PRIORITY);
    reflect_cors!(def, "crossOrigin", "crossorigin");
    reflect_enum!(def, "referrerPolicy", "referrerpolicy", REFERRER_POLICY);
    reflect_str!(def, "integrity", "integrity");
    reflect_str!(def, "hreflang", "hreflang");
    reflect_str!(def, "charset", "charset");
    reflect_str!(def, "rev", "rev");
    reflect_str!(def, "target", "target");
    reflect_bool!(def, "disabled", "disabled");
    reflect_tokens!(def, "relList", "rel");
    reflect_tokens!(def, "sizes", "sizes");
    reflect_tokens!(def, "blocking", "blocking");
    def.getter("sheet", crate::css::element_sheet);
});
simple_class!(HtmlStyleElement, "HTMLStyleElement", HtmlElement, |def| {
    reflect_str!(def, "media", "media");
    reflect_str!(def, "type", "type");
    // Not a reflected attribute: its sheet's disabled flag (HTML § 4.2.6; a `<link>`'s is).
    def.accessor(
        "disabled",
        // pyro: the same flag as its sheet object's `disabled` (one flag per sheet object).
        |s, n| Ok(s.boolean(crate::css::current_sheet_disabled(n.page(), n.id))),
        |_s, n, v| {
            crate::css::set_current_sheet_disabled(n.page(), n.id, v.to_bool());
            Ok(())
        },
    );
    reflect_tokens!(def, "blocking", "blocking");
    def.getter("sheet", crate::css::element_sheet);
});
simple_class!(HtmlScriptElement, "HTMLScriptElement", HtmlElement, |def| {
    reflect_tokens!(def, "blocking", "blocking");
    reflect_enum!(def, "fetchPriority", "fetchpriority", FETCH_PRIORITY);
    // `src` reflects its URL; setting it is a Trusted Types sink (TrustedScriptURL).
    def.pure_accessor_typed("src", pyro_js::GetterResult::String, reflect_url!(@get "src"), |s, n, v| {
        let url = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::ScriptUrl, v, "HTMLScriptElement src", false)?;
        n.page().dom.borrow_mut().set_attribute(n.id, "src", &url).js()
    });
    reflect_str!(def, "type", "type");
    reflect_str!(def, "charset", "charset");
    reflect_cors!(def, "crossOrigin", "crossorigin");
    reflect_str!(def, "integrity", "integrity");
    reflect_enum!(def, "referrerPolicy", "referrerpolicy", REFERRER_POLICY);
    reflect_str!(def, "event", "event");
    reflect_str!(def, "htmlFor", "for");
    reflect_bool!(def, "defer", "defer");
    reflect_bool!(def, "noModule", "nomodule");
    def.accessor(
        "async",
        |s, n| {
            let dom = n.page().dom.borrow();
            let info = dom.script_info(n.id).js()?;
            // Non-parser scripts are async unless explicitly set otherwise.
            Ok(s.boolean(info.async_ || !info.parser_inserted && !dom.has_attribute(n.id, "async").js()? && false))
        },
        |_s, n, v| {
            n.page().dom.borrow_mut().toggle_attribute(n.id, "async", Some(v.to_bool())).js()?;
            Ok(())
        },
    );
    def.accessor(
        "text",
        |s, n| Ok(s.string(&n.page().dom.borrow().script_info(n.id).js()?.inline_text)),
        |s, n, v| {
            let arg0 = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Script, v, "HTMLScriptElement text", false)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
    def.static_method("supports", 1, |s, a| {
        let t = dom_string(a.get(0))?;
        Ok(s.boolean(matches!(t.as_str(), "classic" | "module" | "importmap")))
    });
});
simple_class!(HtmlDivElement, "HTMLDivElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
});
simple_class!(HtmlSpanElement, "HTMLSpanElement", HtmlElement);
simple_class!(HtmlParagraphElement, "HTMLParagraphElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
});
simple_class!(HtmlHeadingElement, "HTMLHeadingElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
});

/// URL decomposition shared by `<a>` and `<area>` (HTMLHyperlinkElementUtils).
fn define_hyperlink<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    reflect_url!(def, "href", "href");
    reflect_str!(def, "target", "target");
    reflect_str!(def, "download", "download");
    reflect_str!(def, "rel", "rel");
    reflect_str!(def, "hreflang", "hreflang");
    reflect_str!(def, "type", "type");
    reflect_enum!(def, "referrerPolicy", "referrerpolicy", REFERRER_POLICY);
    reflect_str!(def, "ping", "ping");
    reflect_tokens!(def, "relList", "rel");
    def.pure_method("toString", 0, |s, n, _a| {
        let dom = n.page().dom.borrow();
        Ok(s.string(&dom.get_attribute(n.id, "href").js()?.map(|h| resolve_url(&dom, n.id, h)).unwrap_or_default()))
    });
    fn link_url(dom: &Dom, n: &NodeData) -> JsResult<Option<pyro_net::Url>> {
        let href = dom.get_attribute(n.id, "href").js()?.map(|h| resolve_url(dom, n.id, h));
        Ok(href.and_then(|h| pyro_net::Url::parse(&h).ok()))
    }
    def.getter("origin", |s, n| {
        let url = link_url(&n.page().dom.borrow(), n)?;
        Ok(s.string(&url.map(|u| quirks::origin(&u)).unwrap_or_default()))
    });
    // A null URL reads as ":" for protocol and "" otherwise, and ignores writes.
    macro_rules! part {
        ($prop:literal, $get:ident, $set:ident, $null:literal) => {
            def.accessor(
                $prop,
                |s, n| {
                    let url = link_url(&n.page().dom.borrow(), n)?;
                    Ok(s.string(url.as_ref().map_or($null, |u| quirks::$get(u))))
                },
                |_s, n, v| {
                    let value = dom_string(v)?;
                    let mut dom = n.page().dom.borrow_mut();
                    let Some(mut url) = link_url(&dom, n)? else { return Ok(()) };
                    let _ = quirks::$set(&mut url, &value);
                    dom.set_attribute(n.id, "href", url.as_str()).js()
                },
            );
        };
    }
    part!("protocol", protocol, set_protocol, ":");
    part!("host", host, set_host, "");
    part!("hostname", hostname, set_hostname, "");
    part!("port", port, set_port, "");
    part!("pathname", pathname, set_pathname, "");
    part!("search", search, set_search, "");
    part!("hash", hash, set_hash, "");
    part!("username", username, set_username, "");
    part!("password", password, set_password, "");
}

simple_class!(HtmlAnchorElement, "HTMLAnchorElement", HtmlElement, |def| {
    define_hyperlink(def);
    reflect_str!(def, "coords", "coords");
    reflect_str!(def, "charset", "charset");
    reflect_str!(def, "name", "name");
    reflect_str!(def, "rev", "rev");
    reflect_str!(def, "shape", "shape");
    def.accessor(
        "text",
        |s, n| Ok(s.string(&n.page().dom.borrow().text_content(n.id).js()?.unwrap_or_default())),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
});
simple_class!(HtmlAreaElement, "HTMLAreaElement", HtmlElement, |def| {
    define_hyperlink(def);
    reflect_str!(def, "alt", "alt");
    reflect_str!(def, "coords", "coords");
    reflect_str!(def, "shape", "shape");
    reflect_bool!(def, "noHref", "nohref");
});
simple_class!(HtmlImageElement, "HTMLImageElement", HtmlElement, |def| {
    // [LegacyFactoryFunction=Image(optional unsigned long width, optional unsigned long height)]
    def.legacy_factory("Image", 0, |s, new_target, a| {
        let (page, el) = create_factory_element(s, "img")?;
        for (i, attr) in [(0, "width"), (1, "height")] {
            if !a.get(i).is_undefined() {
                // `unsigned long`, reflected: values above 2147483647 read and write as 0.
                let v = to_u32(a.get(i))?;
                let v = if v as i64 > MAX_REFLECTED_INT { 0 } else { v };
                page.dom.borrow_mut().set_attribute(el, attr, &v.to_string()).js()?;
            }
        }
        crate::node::wrap_new_node::<Self>(s, &page, el, new_target)
    });
    reflect_url!(def, "src", "src");
    reflect_str!(def, "alt", "alt");
    reflect_str!(def, "srcset", "srcset");
    reflect_str!(def, "sizes", "sizes");
    reflect_cors!(def, "crossOrigin", "crossorigin");
    reflect_str!(def, "useMap", "usemap");
    reflect_enum!(def, "loading", "loading", LOADING);
    reflect_enum!(def, "decoding", "decoding", DECODING);
    reflect_enum!(def, "fetchPriority", "fetchpriority", FETCH_PRIORITY);
    reflect_enum!(def, "referrerPolicy", "referrerpolicy", REFERRER_POLICY);
    reflect_bool!(def, "isMap", "ismap");
    // HTML: the rendered width and height when the image is being rendered, else (as browsers
    // do) the content attribute when it parses, else the density-corrected natural size, else 0.
    def.accessor("width", |s, n| Ok(s.number(img_dimension(n, true)?)), |_s, n, v| set_ulong(n, "width", to_u32(v)?, 0));
    def.accessor("height", |s, n| Ok(s.number(img_dimension(n, false)?)), |_s, n, v| set_ulong(n, "height", to_u32(v)?, 0));
    reflect_str!(def, "name", "name");
    reflect_url!(def, "lowsrc", "lowsrc");
    reflect_str!(def, "align", "align");
    reflect_ulong!(def, "hspace", "hspace", 0);
    reflect_ulong!(def, "vspace", "vspace", 0);
    reflect_url!(def, "longDesc", "longdesc");
    reflect_str_nn!(def, "border", "border");
    // Natural size from the loaded file's header (no pixel decoding); 0
    // while loading, for broken images and for unparsed formats.
    // `unsigned long`: a density-corrected size below 1 px reads 0.
    // pyro: a source in the list of available images is adopted first (`image_complete`).
    def.getter("naturalWidth", |s, n| Ok(s.number({ n.page().image_complete(n.id); n.page().dom.borrow() }.reported_natural_size(n.id).map_or(0.0, |(w, _)| w.trunc()))));
    def.getter("naturalHeight", |s, n| Ok(s.number({ n.page().image_complete(n.id); n.page().dom.borrow() }.reported_natural_size(n.id).map_or(0.0, |(_, h)| h.trunc()))));
    def.getter("complete", |s, n| Ok(s.boolean(n.page().image_complete(n.id))));
    def.getter("currentSrc", |s, n| {
        let viewport = n.page().viewport();
        let dom = n.page().dom.borrow();
        // The source the loader selected for the current request (a lazy image's `sizes=auto`
        // needs its layout), else a `<picture>` parent's matching `<source>`.
        // The loader's selection when `sizes=auto` took part (it needs the layout the image had
        // when selected): a lazy image, or one whose `sizes` (or a `<picture>` `<source>`'s) says
        // `auto`.
        let lazy = dom.get_attribute(n.id, "loading").js()?.is_some_and(|l| l.eq_ignore_ascii_case("lazy"));
        let auto = |el: NodeId| {
            dom.get_attribute(el, "sizes").ok().flatten().is_some_and(|s| s.split(',').next().is_some_and(|f| f.trim().eq_ignore_ascii_case("auto")))
        };
        let in_picture_auto = dom.parent(n.id).ok().flatten().is_some_and(|p| {
            dom.element(p).is_ok_and(|e| &*e.name.local == "picture")
                && dom.children(p).is_ok_and(|mut kids| kids.any(|k| dom.element(k).is_ok_and(|e| &*e.name.local == "source") && auto(k)))
        });
        let stored = if lazy || auto(n.id) || in_picture_auto { dom.image_source(n.id).map(str::to_owned) } else { None };
        let picture = stored.or_else(|| dom.picture_source(n.id, &viewport));
        let selected = picture.as_deref().or(dom.get_attribute(n.id, "src").js()?);
        Ok(s.string(&selected.map(|v| resolve_url(&dom, n.id, v)).unwrap_or_default()))
    });
    def.method("decode", 0, |s, n, _a| crate::image_decode::decode(s, n.page(), n.id));
});

// ----- forms --------------------------------------------------------------------

fn form_value<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let f = n.page().dom.borrow().form_owner(n.id).js()?;
    node_value(s, n.page(), f)
}

fn define_labels<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("labels", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::Labels)?.value()));
}

fn define_validation<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("validity", crate::validity::validity_state);
    def.getter("validationMessage", |s, n| Ok(s.string(&crate::validity::message(n.page(), n.id))));
    def.getter("willValidate", |s, n| Ok(s.boolean(crate::validity::will_validate(n.page(), n.id))));
    def.method("checkValidity", 0, |s, n, _a| Ok(s.boolean(crate::validity::check(n.page(), n.id, false))));
    def.method("reportValidity", 0, |s, n, _a| Ok(s.boolean(crate::validity::check(n.page(), n.id, true))));
    def.method("setCustomValidity", 1, |s, n, a| {
        crate::validity::set_custom(n.page(), n.id, dom_string(a.get(0))?);
        Ok(s.undefined())
    });
}

/// A text control's selection: start, end and direction.
fn selection_state(n: &NodeData) -> Option<(u32, u32, Option<bool>)> {
    let dom = n.page().dom.borrow();
    let (start, end) = dom.selection(n.id).ok()?;
    Some((start, end, dom.selection_direction(n.id).ok()?))
}

/// HTML "set the selection range": when the start, end or direction changed,
/// queue an element task that fires `select` (bubbling, not cancelable).
/// Setting the same range again fires nothing.
fn queue_selection_events_if_changed(n: &NodeData, before: Option<(u32, u32, Option<bool>)>) {
    if selection_state(n) != before {
        crate::editing::queue_control_selectionchange(n.page(), n.id);
        crate::input::queue_simple(n.page(), n.id, "select", true, false);
    }
}

fn define_text_selection<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.accessor(
        "selectionStart",
        |s, n| {
            if !has_selection(n) {
                return Ok(s.null());
            }
            Ok(s.number(n.page().dom.borrow().selection(n.id).js()?.0 as f64))
        },
        |_s, n, v| {
            let st = to_u32(v)?;
            if !has_selection(n) {
                return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
            }
            let before = selection_state(n);
            {
                let mut dom = n.page().dom.borrow_mut();
                let (_, e) = dom.selection(n.id).js()?;
                let direction = dom.selection_direction(n.id).js()?;
                dom.set_selection_range(n.id, st, e.max(st)).js()?;
                dom.set_selection_direction(n.id, direction).js()?;
            }
            queue_selection_events_if_changed(n, before);
            Ok(())
        },
    );
    def.accessor(
        "selectionEnd",
        |s, n| {
            if !has_selection(n) {
                return Ok(s.null());
            }
            Ok(s.number(n.page().dom.borrow().selection(n.id).js()?.1 as f64))
        },
        |_s, n, v| {
            let end = to_u32(v)?;
            if !has_selection(n) {
                return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
            }
            let before = selection_state(n);
            {
                let mut dom = n.page().dom.borrow_mut();
                let (st, _) = dom.selection(n.id).js()?;
                let direction = dom.selection_direction(n.id).js()?;
                dom.set_selection_range(n.id, st, end).js()?;
                dom.set_selection_direction(n.id, direction).js()?;
            }
            queue_selection_events_if_changed(n, before);
            Ok(())
        },
    );
    def.accessor(
        "selectionDirection",
        |s, n| {
            if !has_selection(n) {
                return Ok(s.null());
            }
            Ok(s.string(match n.page().dom.borrow().selection_direction(n.id).js()? {
                Some(true) => "backward",
                Some(false) => "forward",
                None => "none",
            }))
        },
        |_s, n, v| {
            if !has_selection(n) {
                return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
            }
            let direction = selection_direction(&dom_string(v)?);
            let before = selection_state(n);
            n.page().dom.borrow_mut().set_selection_direction(n.id, direction).js()?;
            queue_selection_events_if_changed(n, before);
            Ok(())
        },
    );
    def.method("setSelectionRange", 2, |s, n, a| {
        if !has_selection(n) {
            return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
        }
        let arg0 = to_u32(a.get(0))?;
        let arg1 = to_u32(a.get(1))?;
        let direction = if a.get(2).is_undefined() { None } else { selection_direction(&dom_string(a.get(2))?) };
        let before = selection_state(n);
        {
            let mut dom = n.page().dom.borrow_mut();
            dom.set_selection_range(n.id, arg0, arg1).js()?;
            dom.set_selection_direction(n.id, direction).js()?;
        }
        queue_selection_events_if_changed(n, before);
        Ok(s.undefined())
    });
    def.method("select", 0, |s, n, _a| {
        // select() also applies to number inputs, though the selection API
        // does not: Chromium selects their whole value, so the next edit
        // (Playwright fill: select(), then Input.insertText) replaces it.
        let number = n.page().dom.borrow().input_type(n.id).is_ok_and(|t| t == InputType::Number);
        if has_selection(n) || number {
            let before = selection_state(n);
            let mut dom = n.page().dom.borrow_mut();
            let len = pyro_dom::dom::utf16_len(&dom.value(n.id).js()?);
            dom.set_selection_range(n.id, 0, len).js()?;
            drop(dom);
            // Blink's `TextControlElement::select()` then focuses the control (without restoring
            // a cached selection or scrolling to it): the selection shows, with the focus ring.
            let page = n.page();
            let doc = page.document();
            if page.dom.borrow().focused(doc).ok().flatten() != Some(n.id) {
                crate::input::focus_element_with_options(page, n.id, true);
                let mut dom = page.dom.borrow_mut();
                dom.set_selection_range(n.id, 0, len).js()?;
            }
            queue_selection_events_if_changed(n, before);
        }
        Ok(s.undefined())
    });
    def.method("setRangeText", 1, |s, n, a| {
        let text = arg_string(&a, 0, "setRangeText")?;
        // The arguments convert (in order) before the type is checked.
        let range = if a.len() >= 2 {
            a.require(3, "setRangeText")?;
            let (start, end) = (to_u32(a.get(1))?, to_u32(a.get(2))?);
            let mode = match a.get(3) {
                v if v.is_undefined() => pyro_dom::forms::RangeMode::Preserve,
                v => match dom_string(v)?.as_str() {
                    "select" => pyro_dom::forms::RangeMode::Select,
                    "start" => pyro_dom::forms::RangeMode::Start,
                    "end" => pyro_dom::forms::RangeMode::End,
                    "preserve" => pyro_dom::forms::RangeMode::Preserve,
                    other => {
                        return Err(JsError::Type(format!(
                            "Failed to execute 'setRangeText': The provided value '{other}' is not a valid enum value of type SelectionMode."
                        ).into()))
                    }
                },
            };
            Some((start, end, mode))
        } else {
            None
        };
        if !has_selection(n) {
            return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
        }
        let (start, end, mode) = match range {
            Some(r) => r,
            None => {
                let (st, en) = n.page().dom.borrow().selection(n.id).js()?;
                (st, en, pyro_dom::forms::RangeMode::Preserve)
            }
        };
        if start > end {
            return Err(JsError::dom("IndexSizeError", "The provided start value is larger than the end value."));
        }
        let before = selection_state(n);
        n.page().dom.borrow_mut().set_range_text(n.id, &text, start, end, mode).js()?;
        queue_selection_events_if_changed(n, before);
        Ok(s.undefined())
    });
}

/// `autocomplete` of `input`, `select` and `textarea`: the attribute's
/// IDL-exposed autofill value on getting, the attribute on setting.
fn define_autocomplete<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.accessor(
        "autocomplete",
        |s, n| {
            let dom = n.page().dom.borrow();
            let anchor = dom.element(n.id).is_ok_and(|e| &*e.name.local == "input")
                && dom.input_type(n.id).is_ok_and(|t| t == InputType::Hidden);
            Ok(s.string(&pyro_dom::forms::autofill_idl_value(dom.get_attribute(n.id, "autocomplete").js()?, anchor)))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "autocomplete", &arg0).js()
        },
    );
}

/// A `selectionDirection` keyword: backward (`Some(true)`), forward, or
/// "none" for anything else.
fn selection_direction(keyword: &str) -> Option<bool> {
    match keyword {
        "backward" => Some(true),
        "forward" => Some(false),
        _ => None,
    }
}

fn has_selection(n: &NodeData) -> bool {
    let dom = n.page().dom.borrow();
    match dom.element(n.id) {
        Ok(e) if &*e.name.local == "textarea" => true,
        Ok(e) if &*e.name.local == "input" => dom.input_type(n.id).map(|t| t.is_textual()).unwrap_or(false),
        _ => false,
    }
}

simple_class!(HtmlInputElement, "HTMLInputElement", HtmlElement, |def| {
    crate::popover::define_popover_invoker(def);
    define_labels(def);
    define_validation(def);
    define_text_selection(def);
    reflect_str!(def, "name", "name");
    reflect_str!(def, "placeholder", "placeholder");
    reflect_str!(def, "min", "min");
    reflect_str!(def, "max", "max");
    reflect_str!(def, "step", "step");
    reflect_str!(def, "pattern", "pattern");
    reflect_str!(def, "accept", "accept");
    reflect_str!(def, "alt", "alt");
    define_autocomplete(def);
    reflect_str!(def, "dirName", "dirname");
    reflect_str!(def, "formTarget", "formtarget");
    reflect_str!(def, "align", "align");
    reflect_str!(def, "useMap", "usemap");
    reflect_url!(def, "src", "src");
    reflect_action!(def, "formAction", "formaction");
    reflect_enum!(def, "formEnctype", "formenctype", FORM_ENCTYPE_OVERRIDE);
    reflect_enum!(def, "formMethod", "formmethod", FORM_METHOD_OVERRIDE);
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "readOnly", "readonly");
    reflect_bool!(def, "required", "required");
    reflect_bool!(def, "multiple", "multiple");
    reflect_bool!(def, "formNoValidate", "formnovalidate");
    reflect_bool!(def, "alpha", "alpha");
    reflect_enum!(def, "colorSpace", "colorspace", COLOR_SPACE);
    reflect_limited_long!(def, "maxLength", "maxlength");
    reflect_limited_long!(def, "minLength", "minlength");
    reflect_positive_ulong!(def, "size", "size", 20);
    reflect_ulong!(def, "width", "width", 0);
    reflect_ulong!(def, "height", "height", 0);
    def.getter("form", form_value);
    def.accessor(
        "type",
        |s, n| Ok(s.string(n.page().dom.borrow().input_type(n.id).js()?.keyword())),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "type", &arg0).js()
        },
    );
    def.accessor(
        "value",
        |s, n| Ok(s.string(&n.page().dom.borrow().value(n.id).js()?)),
        |_s, n, v| {
            let arg0 = null_to_empty(v)?;
            let page = n.page();
            page.dom.borrow_mut().set_value(n.id, &arg0).js()?;
            // `value = ''` on a file input empties its selected files too.
            if page.dom.borrow().input_type(n.id).js()? == InputType::File {
                page.files.borrow_mut().remove(&n.id);
            }
            Ok(())
        },
    );
    def.accessor(
        "defaultValue",
        |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, "value").js()?.unwrap_or(""))),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "value", &arg0).js()
        },
    );
    def.accessor(
        "checked",
        |s, n| Ok(s.boolean(n.page().dom.borrow().checked(n.id).js()?)),
        |_s, n, v| n.page().dom.borrow_mut().set_checked(n.id, v.to_bool()).js(),
    );
    reflect_bool!(def, "defaultChecked", "checked");
    def.accessor(
        "indeterminate",
        |s, n| Ok(s.boolean(n.page().dom.borrow().control_state(n.id).js()?.is_some_and(|c| c.indeterminate))),
        |_s, n, v| {
            n.page().dom.borrow_mut().control_state_mut(n.id).js()?.indeterminate = v.to_bool();
            Ok(())
        },
    );
    def.accessor(
        "valueAsNumber",
        |s, n| Ok(s.number(n.page().dom.borrow().value_as_number(n.id).js()?)),
        |_s, n, v| {
            let x = v.to_number()?;
            if x.is_infinite() {
                return Err(JsError::Type(
                    "Failed to set the 'valueAsNumber' property on 'HTMLInputElement': The value provided is infinite."
                        .into(),
                ));
            }
            n.page().dom.borrow_mut().set_value_as_number(n.id, x).js()
        },
    );
    def.accessor(
        "valueAsDate",
        |s, n| {
            let time = n.page().dom.borrow().value_as_date(n.id).js()?;
            match time {
                Some(t) => Ok(s.date(t)?.value()),
                None => Ok(s.null()),
            }
        },
        |_s, n, v| {
            let time = if v.is_null() {
                None
            } else if v.is_date() {
                Some(v.to_number()?)
            } else {
                return Err(JsError::Type(
                    "Failed to set the 'valueAsDate' property on 'HTMLInputElement': The provided value is not a Date."
                        .into(),
                ));
            };
            n.page().dom.borrow_mut().set_value_as_date(n.id, time).js()
        },
    );
    def.accessor("files", crate::file::input_files, crate::file::set_input_files);
    def.getter("list", |s, n| {
        let dom = n.page().dom.borrow();
        let id = dom.get_attribute(n.id, "list").js()?.map(str::to_owned);
        let doc = dom.node_document(n.id).js()?;
        let target = match id {
            Some(id) => dom.get_element_by_id(doc, &id).js()?,
            None => None,
        };
        drop(dom);
        node_value(s, n.page(), target)
    });
    def.method("stepUp", 0, |s, n, a| step(s, n, a, true));
    def.method("stepDown", 0, |s, n, a| step(s, n, a, false));
    def.method("showPicker", 0, |s, _n, _a| Ok(s.undefined()));
});

/// `stepUp(n)` / `stepDown(n)`.
fn step<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, up: bool) -> JsResult<Value<'s>> {
    let k = if a.get(0).is_undefined() { 1 } else { to_i32(a.get(0))? };
    n.page().dom.borrow_mut().step_value(n.id, f64::from(k), up).js()?;
    Ok(s.undefined())
}

simple_class!(HtmlTextAreaElement, "HTMLTextAreaElement", HtmlElement, |def| {
    define_labels(def);
    define_validation(def);
    define_text_selection(def);
    reflect_str!(def, "name", "name");
    reflect_str!(def, "placeholder", "placeholder");
    define_autocomplete(def);
    reflect_str!(def, "wrap", "wrap");
    reflect_str!(def, "dirName", "dirname");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "readOnly", "readonly");
    reflect_bool!(def, "required", "required");
    reflect_limited_long!(def, "maxLength", "maxlength");
    reflect_limited_long!(def, "minLength", "minlength");
    reflect_positive_ulong_fallback!(def, "rows", "rows", 2);
    reflect_positive_ulong_fallback!(def, "cols", "cols", 20);
    def.getter("form", form_value);
    def.getter("type", |s, _n| Ok(s.string("textarea")));
    def.accessor(
        "value",
        |s, n| Ok(s.string(&n.page().dom.borrow().value(n.id).js()?)),
        |_s, n, v| {
            let arg0 = null_to_empty(v)?;
            n.page().dom.borrow_mut().set_value(n.id, &arg0).js()
        },
    );
    def.accessor(
        "defaultValue",
        |s, n| Ok(s.string(&n.page().dom.borrow().text_content(n.id).js()?.unwrap_or_default())),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
    def.getter("textLength", |s, n| {
        Ok(s.number(pyro_dom::dom::utf16_len(&n.page().dom.borrow().value(n.id).js()?) as f64))
    });
});

simple_class!(HtmlSelectElement, "HTMLSelectElement", HtmlElement, |def| {
    define_labels(def);
    define_validation(def);
    reflect_str!(def, "name", "name");
    define_autocomplete(def);
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "required", "required");
    reflect_bool!(def, "multiple", "multiple");
    reflect_ulong!(def, "size", "size", 0);
    def.getter("form", form_value);
    def.getter("type", |s, n| {
        Ok(s.string(if n.page().dom.borrow().has_attribute(n.id, "multiple").js()? {
            "select-multiple"
        } else {
            "select-one"
        }))
    });
    def.getter("options", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::Options)?.value()));
    def.getter("selectedOptions", |s, n| {
        Ok(collections::collection(s, n.page(), n.id, CollectionKind::SelectedOptions)?.value())
    });
    def.getter("length", |s, n| Ok(s.number(n.page().dom.borrow().select_options(n.id).js()?.len() as f64)));
    def.accessor(
        "selectedIndex",
        |s, n| Ok(s.number(n.page().dom.borrow().selected_index(n.id).js()? as f64)),
        |_s, n, v| {
            let arg0 = to_i32(v)?;
            n.page().dom.borrow_mut().set_selected_index(n.id, arg0).js()
        },
    );
    def.accessor(
        "value",
        |s, n| Ok(s.string(&n.page().dom.borrow().value(n.id).js()?)),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_value(n.id, &arg0).js()
        },
    );
    def.pure_method("item", 1, |s, n, a| {
        let i = to_u32(a.get(0))? as usize;
        let o = n.page().dom.borrow().select_options(n.id).js()?.get(i).copied();
        node_value(s, n.page(), o)
    });
    // Indexed properties make it iterable (Web IDL).
    def.indexed_iterator();
    def.indexed(|s, n, i| {
        let o = n.page().dom.borrow().select_options(n.id).js()?.get(i as usize).copied();
        match o {
            Some(o) => Ok(Some(wrap_node(s, n.page(), o)?.value())),
            None => Ok(None),
        }
    });
    def.pure_method("namedItem", 1, |s, n, a| {
        let name = dom_string(a.get(0))?;
        let dom = n.page().dom.borrow();
        let found = dom.select_options(n.id).js()?.into_iter().find(|&o| {
            dom.element(o).is_ok_and(|e| e.id() == Some(name.as_str()) || e.attr("name") == Some(name.as_str()))
        });
        drop(dom);
        node_value(s, n.page(), found)
    });
    def.method("add", 1, |s, n, a| {
        collections::options_add(s, n.page(), n.id, a, "HTMLSelectElement")?;
        Ok(s.undefined())
    });
    def.method("remove", 0, |s, n, a| {
        let mut dom = n.page().dom.borrow_mut();
        if a.is_empty() {
            dom.remove_node(n.id).js()?;
        } else {
            let i = to_i32(a.get(0))?;
            let opts = dom.select_options(n.id).js()?;
            if let Some(&o) = usize::try_from(i).ok().and_then(|i| opts.get(i)) {
                dom.remove_node(o).js()?;
            }
        }
        Ok(s.undefined())
    });
    def.method("showPicker", 0, |s, _n, _a| Ok(s.undefined()));
});

simple_class!(HtmlOptionElement, "HTMLOptionElement", HtmlElement, |def| {
    // [LegacyFactoryFunction=Option(optional DOMString text = "", optional DOMString value,
    //                               optional boolean defaultSelected = false, optional boolean selected = false)]
    def.legacy_factory("Option", 0, |s, new_target, a| {
        let (page, el) = create_factory_element(s, "option")?;
        let text = if a.get(0).is_undefined() { String::new() } else { dom_string(a.get(0))? };
        let value = if a.get(1).is_undefined() { None } else { Some(dom_string(a.get(1))?) };
        let default_selected = a.get(2).to_bool();
        let selected = a.get(3).to_bool();
        {
            let mut dom = page.dom.borrow_mut();
            if !text.is_empty() {
                dom.set_text_content(el, &text).js()?;
            }
            if let Some(value) = value {
                dom.set_attribute(el, "value", &value).js()?;
            }
            if default_selected {
                dom.set_attribute(el, "selected", "").js()?;
            }
            // The constructor sets the selectedness, not the dirtiness. When
            // the selectedness is what the attribute gives, the option stays
            // clean and keeps following `defaultSelected`; an explicit state
            // is only needed when the two differ (which pyro-dom can only
            // record as dirty).
            if selected != default_selected {
                dom.set_option_selected(el, selected).js()?;
            }
        }
        crate::node::wrap_new_node::<Self>(s, &page, el, new_target)
    });
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "defaultSelected", "selected");
    def.getter("form", |s, n| {
        let dom = n.page().dom.borrow();
        let mut p = dom.parent(n.id).js()?;
        while let Some(x) = p {
            if dom.element(x).is_ok_and(|e| &*e.name.local == "select") {
                let f = dom.form_owner(x).js()?;
                drop(dom);
                return node_value(s, n.page(), f);
            }
            p = dom.parent(x).js()?;
        }
        Ok(s.null())
    });
    def.accessor(
        "value",
        |s, n| Ok(s.string(&n.page().dom.borrow().option_value(n.id).js()?)),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "value", &arg0).js()
        },
    );
    def.accessor(
        "text",
        |s, n| Ok(s.string(&n.page().dom.borrow().option_text(n.id).js()?)),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
    def.accessor(
        "label",
        |s, n| {
            let dom = n.page().dom.borrow();
            Ok(s.string(&match dom.get_attribute(n.id, "label").js()? {
                Some(l) => l.to_owned(),
                None => dom.option_text(n.id).js()?,
            }))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "label", &arg0).js()
        },
    );
    def.accessor(
        "selected",
        |s, n| Ok(s.boolean(n.page().dom.borrow().option_selected(n.id).js()?)),
        |_s, n, v| n.page().dom.borrow_mut().set_option_selected(n.id, v.to_bool()).js(),
    );
    def.getter("index", |s, n| {
        let dom = n.page().dom.borrow();
        let mut p = dom.parent(n.id).js()?;
        while let Some(x) = p {
            if dom.element(x).is_ok_and(|e| &*e.name.local == "select") {
                let opts = dom.select_options(x).js()?;
                return Ok(s.number(opts.iter().position(|&o| o == n.id).unwrap_or(0) as f64));
            }
            p = dom.parent(x).js()?;
        }
        Ok(s.number(0.0))
    });
});

simple_class!(HtmlOptGroupElement, "HTMLOptGroupElement", HtmlElement, |def| {
    reflect_bool!(def, "disabled", "disabled");
    reflect_str!(def, "label", "label");
});

simple_class!(HtmlButtonElement, "HTMLButtonElement", HtmlElement, |def| {
    define_labels(def);
    define_validation(def);
    reflect_str!(def, "name", "name");
    reflect_str!(def, "value", "value");
    reflect_action!(def, "formAction", "formaction");
    reflect_enum!(def, "formEnctype", "formenctype", FORM_ENCTYPE_OVERRIDE);
    reflect_enum!(def, "formMethod", "formmethod", FORM_METHOD_OVERRIDE);
    reflect_str!(def, "formTarget", "formtarget");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "formNoValidate", "formnovalidate");
    def.getter("form", form_value);
    crate::popover::define_popover_invoker(def);
    crate::popover::define_command_invoker(def);
    def.accessor(
        "type",
        |s, n| {
            let dom = n.page().dom.borrow();
            let t = dom.get_attribute(n.id, "type").js()?.map(|t| t.to_ascii_lowercase());
            Ok(s.string(match t.as_deref() {
                Some("reset") => "reset",
                Some("button") => "button",
                // Without a valid type, a `command`/`commandfor` button is a plain button.
                _ if !crate::popover::button_is_submit(&dom, n.id) => "button",
                _ => "submit",
            }))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "type", &arg0).js()
        },
    );
});

simple_class!(HtmlFormElement, "HTMLFormElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    reflect_str!(def, "target", "target");
    reflect_str!(def, "rel", "rel");
    reflect_tokens!(def, "relList", "rel");
    reflect_enum!(def, "enctype", "enctype", FORM_ENCTYPE);
    reflect_enum!(def, "encoding", "enctype", FORM_ENCTYPE);
    reflect_str!(def, "acceptCharset", "accept-charset");
    reflect_enum!(def, "autocomplete", "autocomplete", FORM_AUTOCOMPLETE);
    reflect_bool!(def, "noValidate", "novalidate");
    reflect_action!(def, "action", "action");
    reflect_enum!(def, "method", "method", FORM_METHOD);
    def.getter(
        "elements",
        |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?.value()),
    );
    def.getter("length", |s, n| {
        let c = collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?;
        c.get_atom("length")
    });
    def.indexed(|s, n, i| {
        let c = collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?;
        let v = c.get_index(i)?;
        Ok(if v.is_undefined() { None } else { Some(v) })
    });
    // [LegacyOverrideBuiltIns]: a control named "submit" shadows the method.
    def.named(true, |s, n, name| collections::form_named(s, n.page(), n.id, name));
    def.method("submit", 0, |s, n, _a| {
        crate::input::submit_form(n.page(), n.id, None, false);
        Ok(s.undefined())
    });
    def.method("requestSubmit", 0, |s, n, a| {
        let submitter = crate::node::opt_node_arg(s, a.get(0), "requestSubmit")?;
        crate::input::submit_form(n.page(), n.id, submitter, true);
        Ok(s.undefined())
    });
    def.method("reset", 0, |s, n, _a| {
        crate::input::reset_form(n.page(), n.id);
        Ok(s.undefined())
    });
    def.method("checkValidity", 0, |s, n, _a| Ok(s.boolean(crate::validity::check_form(n.page(), n.id, false))));
    def.method("reportValidity", 0, |s, n, _a| Ok(s.boolean(crate::validity::check_form(n.page(), n.id, true))));
});

simple_class!(HtmlLabelElement, "HTMLLabelElement", HtmlElement, |def| {
    reflect_str!(def, "htmlFor", "for");
    def.getter("form", |s, n| {
        let c = labeled_control(&n.page().dom.borrow(), n.id);
        match c {
            Some(c) => {
                let f = n.page().dom.borrow().form_owner(c).js()?;
                node_value(s, n.page(), f)
            }
            None => Ok(s.null()),
        }
    });
    def.getter("control", |s, n| {
        let c = labeled_control(&n.page().dom.borrow(), n.id);
        node_value(s, n.page(), c)
    });
});

/// The labeled control of a `<label>` (HTML §4.10.4).
pub fn labeled_control(dom: &Dom, label: NodeId) -> Option<NodeId> {
    dom.labeled_control(label).ok().flatten()
}

/// `element.labels`: from the tree scope's index of labels by `for` and the
/// control's ancestors (pyro-dom), not a walk of the document.
pub(crate) fn labels_for(dom: &Dom, control: NodeId) -> Vec<NodeId> {
    dom.labels(control).unwrap_or_default()
}

simple_class!(HtmlFieldSetElement, "HTMLFieldSetElement", HtmlElement, |def| {
    define_validation(def);
    reflect_str!(def, "name", "name");
    reflect_bool!(def, "disabled", "disabled");
    def.getter("form", form_value);
    def.getter("type", |s, _n| Ok(s.string("fieldset")));
    def.getter(
        "elements",
        |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?.value()),
    );
});
simple_class!(HtmlLegendElement, "HTMLLegendElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
});
simple_class!(HtmlUListElement, "HTMLUListElement", HtmlElement, |def| {
    reflect_bool!(def, "compact", "compact");
    reflect_str!(def, "type", "type");
});
simple_class!(HtmlOListElement, "HTMLOListElement", HtmlElement, |def| {
    reflect_bool!(def, "reversed", "reversed");
    reflect_long!(def, "start", "start", 1);
    reflect_str!(def, "type", "type");
    reflect_bool!(def, "compact", "compact");
});
simple_class!(HtmlLiElement, "HTMLLIElement", HtmlElement, |def| {
    reflect_long!(def, "value", "value", 0);
    reflect_str!(def, "type", "type");
});

// ----- tables ---------------------------------------------------------------------

fn is_html(dom: &Dom, id: NodeId, names: &[&str]) -> bool {
    dom.element(id).is_ok_and(|e| e.is_html() && names.contains(&&*e.name.local))
}

pub(crate) fn table_collection(dom: &Dom, root: NodeId, kind: &CollectionKind) -> Vec<NodeId> {
    let kids = |p: NodeId| dom.element_children(p).map(|i| i.collect::<Vec<_>>()).unwrap_or_default();
    match kind {
        CollectionKind::TableRows => {
            // thead rows, then direct/tbody rows in order, then tfoot rows.
            let mut head = Vec::new();
            let mut body = Vec::new();
            let mut foot = Vec::new();
            for c in kids(root) {
                if is_html(dom, c, &["thead"]) {
                    head.extend(kids(c).into_iter().filter(|&r| is_html(dom, r, &["tr"])));
                } else if is_html(dom, c, &["tfoot"]) {
                    foot.extend(kids(c).into_iter().filter(|&r| is_html(dom, r, &["tr"])));
                } else if is_html(dom, c, &["tbody"]) {
                    body.extend(kids(c).into_iter().filter(|&r| is_html(dom, r, &["tr"])));
                } else if is_html(dom, c, &["tr"]) {
                    body.push(c);
                }
            }
            head.extend(body);
            head.extend(foot);
            head
        }
        CollectionKind::TableBodies => kids(root).into_iter().filter(|&c| is_html(dom, c, &["tbody"])).collect(),
        CollectionKind::SectionRows => kids(root).into_iter().filter(|&c| is_html(dom, c, &["tr"])).collect(),
        CollectionKind::RowCells => kids(root).into_iter().filter(|&c| is_html(dom, c, &["td", "th"])).collect(),
        _ => Vec::new(),
    }
}

fn table_section<'s>(s: &Scope<'s>, n: &NodeData, tag: &str) -> JsResult<Value<'s>> {
    let dom = n.page().dom.borrow();
    let found = dom.element_children(n.id).js()?.find(|&c| is_html(&dom, c, &[tag]));
    drop(dom);
    node_value(s, n.page(), found)
}

/// A "table-created" element: an HTML `tag` element of `parent`'s node document.
fn table_created(dom: &mut Dom, parent: NodeId, tag: &str) -> JsResult<NodeId> {
    let doc = dom.node_document(parent).js()?;
    dom.create_element_ns(doc, Some("http://www.w3.org/1999/xhtml"), tag).js()
}

/// `createCaption()` / `createTHead()` / `createTFoot()`: the table's first
/// `tag` child, created at the place HTML gives it when there is none.
fn table_ensure_child<'s>(s: &Scope<'s>, n: &NodeData, tag: &str) -> JsResult<Value<'s>> {
    let mut dom = n.page().dom.borrow_mut();
    let kids: Vec<NodeId> = dom.element_children(n.id).js()?.collect();
    let el = match kids.iter().copied().find(|&c| is_html(&dom, c, &[tag])) {
        Some(el) => el,
        None => {
            let el = table_created(&mut dom, n.id, tag)?;
            let before = match tag {
                // First child of the table.
                "caption" => dom.first_child(n.id).js()?,
                // Before the first child element that is neither a caption nor a colgroup.
                "thead" => kids.iter().copied().find(|&c| !is_html(&dom, c, &["caption", "colgroup"])),
                // At the end.
                _ => None,
            };
            dom.insert_before(n.id, el, before).js()?;
            el
        }
    };
    drop(dom);
    Ok(wrap_node(s, n.page(), el)?.value())
}

/// `deleteCaption()` / `deleteTHead()` / `deleteTFoot()`.
fn table_delete_child(n: &NodeData, tag: &str) -> JsResult<()> {
    let mut dom = n.page().dom.borrow_mut();
    let found = dom.element_children(n.id).js()?.find(|&c| is_html(&dom, c, &[tag]));
    if let Some(el) = found {
        dom.remove_node(el).js()?;
    }
    Ok(())
}

/// The `index` argument of `insertRow` / `insertCell` (default -1) against a
/// collection of `len` members: the member to insert before (`None`: at the
/// end), or an "IndexSizeError".
fn insertion_index(a: &Args<'_>, len: usize, what: &str) -> JsResult<Option<usize>> {
    let index = if a.get(0).is_undefined() { -1 } else { to_i32(a.get(0))? };
    match usize::try_from(index) {
        Ok(i) if i < len => Ok(Some(i)),
        Ok(i) if i == len => Ok(None),
        Err(_) if index == -1 => Ok(None),
        _ => Err(index_size_error(what)),
    }
}

/// `deleteRow(index)` / `deleteCell(index)` over `members`: -1 removes the
/// last one (nothing when there is none); other indices outside the
/// collection are an "IndexSizeError".
fn delete_member(n: &NodeData, a: &Args<'_>, kind: &CollectionKind, operation: &str) -> JsResult<()> {
    a.require(1, operation)?;
    let index = to_i32(a.get(0))?;
    let mut dom = n.page().dom.borrow_mut();
    let members = table_collection(&dom, n.id, kind);
    let member = match usize::try_from(index) {
        Ok(i) => Some(*members.get(i).ok_or_else(|| index_size_error("index"))?),
        Err(_) if index == -1 => members.last().copied(),
        Err(_) => return Err(index_size_error("index")),
    };
    if let Some(member) = member {
        dom.remove_node(member).js()?;
    }
    Ok(())
}

/// `insertRow(index)` on a section, `insertCell(index)` on a row: a new `tag`
/// child at `index` of the element's `kind` collection.
fn insert_member<'s>(
    s: &Scope<'s>,
    n: &NodeData,
    a: &Args<'_>,
    kind: &CollectionKind,
    tag: &str,
    what: &str,
) -> JsResult<Value<'s>> {
    let mut dom = n.page().dom.borrow_mut();
    let members = table_collection(&dom, n.id, kind);
    let before = insertion_index(a, members.len(), what)?.map(|i| members[i]);
    let el = table_created(&mut dom, n.id, tag)?;
    dom.insert_before(n.id, el, before).js()?;
    drop(dom);
    Ok(wrap_node(s, n.page(), el)?.value())
}

simple_class!(HtmlTableElement, "HTMLTableElement", HtmlElement, |def| {
    def.getter("rows", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::TableRows)?.value()));
    def.getter("tBodies", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::TableBodies)?.value()));
    def.getter("tHead", |s, n| table_section(s, n, "thead"));
    def.getter("tFoot", |s, n| table_section(s, n, "tfoot"));
    def.getter("caption", |s, n| table_section(s, n, "caption"));
    def.method("createCaption", 0, |s, n, _a| table_ensure_child(s, n, "caption"));
    def.method("deleteCaption", 0, |s, n, _a| table_delete_child(n, "caption").map(|()| s.undefined()));
    def.method("createTHead", 0, |s, n, _a| table_ensure_child(s, n, "thead"));
    def.method("deleteTHead", 0, |s, n, _a| table_delete_child(n, "thead").map(|()| s.undefined()));
    def.method("createTFoot", 0, |s, n, _a| table_ensure_child(s, n, "tfoot"));
    def.method("deleteTFoot", 0, |s, n, _a| table_delete_child(n, "tfoot").map(|()| s.undefined()));
    def.method("createTBody", 0, |s, n, _a| {
        let mut dom = n.page().dom.borrow_mut();
        // After the last tbody child, else at the end of the table.
        let last_body = dom.element_children(n.id).js()?.filter(|&c| is_html(&dom, c, &["tbody"])).last();
        let before = match last_body {
            Some(body) => dom.next_sibling(body).js()?,
            None => None,
        };
        let body = table_created(&mut dom, n.id, "tbody")?;
        dom.insert_before(n.id, body, before).js()?;
        drop(dom);
        Ok(wrap_node(s, n.page(), body)?.value())
    });
    def.method("insertRow", 0, |s, n, a| {
        let mut dom = n.page().dom.borrow_mut();
        let rows = table_collection(&dom, n.id, &CollectionKind::TableRows);
        let before = insertion_index(&a, rows.len(), "index")?.map(|i| rows[i]);
        let row = table_created(&mut dom, n.id, "tr")?;
        match (before, rows.last()) {
            // Before the row at that index, in that row's parent.
            (Some(before), _) => {
                let parent = dom.parent(before).js()?.unwrap_or(n.id);
                dom.insert_before(parent, row, Some(before)).js()?;
            }
            // At the end of the parent of the last row.
            (None, Some(&last)) => {
                let parent = dom.parent(last).js()?.unwrap_or(n.id);
                dom.insert_before(parent, row, None).js()?;
            }
            // No rows: into the last tbody, creating one when there is none.
            (None, None) => {
                let last_body = dom.element_children(n.id).js()?.filter(|&c| is_html(&dom, c, &["tbody"])).last();
                let body = match last_body {
                    Some(body) => body,
                    None => {
                        let body = table_created(&mut dom, n.id, "tbody")?;
                        dom.insert_before(n.id, body, None).js()?;
                        body
                    }
                };
                dom.insert_before(body, row, None).js()?;
            }
        }
        drop(dom);
        Ok(wrap_node(s, n.page(), row)?.value())
    });
    def.method("deleteRow", 1, |s, n, a| {
        delete_member(n, &a, &CollectionKind::TableRows, "deleteRow").map(|()| s.undefined())
    });
    reflect_str!(def, "border", "border");
    reflect_str!(def, "width", "width");
    reflect_str!(def, "align", "align");
    reflect_str!(def, "frame", "frame");
    reflect_str!(def, "rules", "rules");
    reflect_str!(def, "summary", "summary");
    reflect_str_nn!(def, "bgColor", "bgcolor");
    reflect_str_nn!(def, "cellPadding", "cellpadding");
    reflect_str_nn!(def, "cellSpacing", "cellspacing");
});
/// The obsolete cell alignment attributes shared by sections, rows, cells and
/// columns.
fn define_cell_alignment<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    reflect_str!(def, "align", "align");
    reflect_str!(def, "ch", "char");
    reflect_str!(def, "chOff", "charoff");
    reflect_str!(def, "vAlign", "valign");
}
simple_class!(HtmlTableSectionElement, "HTMLTableSectionElement", HtmlElement, |def| {
    define_cell_alignment(def);
    def.getter("rows", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::SectionRows)?.value()));
    def.method("insertRow", 0, |s, n, a| insert_member(s, n, &a, &CollectionKind::SectionRows, "tr", "index"));
    def.method("deleteRow", 1, |s, n, a| {
        delete_member(n, &a, &CollectionKind::SectionRows, "deleteRow").map(|()| s.undefined())
    });
});
simple_class!(HtmlTableRowElement, "HTMLTableRowElement", HtmlElement, |def| {
    define_cell_alignment(def);
    reflect_str_nn!(def, "bgColor", "bgcolor");
    def.getter("cells", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::RowCells)?.value()));
    def.method("insertCell", 0, |s, n, a| insert_member(s, n, &a, &CollectionKind::RowCells, "td", "index"));
    def.method("deleteCell", 1, |s, n, a| {
        delete_member(n, &a, &CollectionKind::RowCells, "deleteCell").map(|()| s.undefined())
    });

    def.getter("rowIndex", |s, n| {
        let dom = n.page().dom.borrow();
        let mut p = dom.parent(n.id).js()?;
        while let Some(x) = p {
            if is_html(&dom, x, &["table"]) {
                let rows = table_collection(&dom, x, &CollectionKind::TableRows);
                return Ok(s.number(rows.iter().position(|&r| r == n.id).map_or(-1.0, |i| i as f64)));
            }
            p = dom.parent(x).js()?;
        }
        Ok(s.number(-1.0))
    });
    def.getter("sectionRowIndex", |s, n| {
        let dom = n.page().dom.borrow();
        let Some(p) = dom.parent(n.id).js()? else { return Ok(s.number(-1.0)) };
        let rows = table_collection(&dom, p, &CollectionKind::SectionRows);
        Ok(s.number(rows.iter().position(|&r| r == n.id).map_or(-1.0, |i| i as f64)))
    });
});
simple_class!(HtmlTableCellElement, "HTMLTableCellElement", HtmlElement, |def| {
    define_cell_alignment(def);
    reflect_clamped_ulong!(def, "colSpan", "colspan", 1, 1, 1000);
    reflect_clamped_ulong!(def, "rowSpan", "rowspan", 1, 0, 65534);
    reflect_str!(def, "headers", "headers");
    reflect_enum!(def, "scope", "scope", CELL_SCOPE);
    reflect_str!(def, "abbr", "abbr");
    reflect_str!(def, "axis", "axis");
    reflect_str!(def, "height", "height");
    reflect_str!(def, "width", "width");
    reflect_bool!(def, "noWrap", "nowrap");
    reflect_str_nn!(def, "bgColor", "bgcolor");
    def.getter("cellIndex", |s, n| {
        let dom = n.page().dom.borrow();
        let Some(p) = dom.parent(n.id).js()? else { return Ok(s.number(-1.0)) };
        if !is_html(&dom, p, &["tr"]) {
            return Ok(s.number(-1.0));
        }
        let cells = table_collection(&dom, p, &CollectionKind::RowCells);
        Ok(s.number(cells.iter().position(|&c| c == n.id).map_or(-1.0, |i| i as f64)))
    });
});
simple_class!(HtmlTableCaptionElement, "HTMLTableCaptionElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
});
simple_class!(HtmlTableColElement, "HTMLTableColElement", HtmlElement, |def| {
    define_cell_alignment(def);
    reflect_clamped_ulong!(def, "span", "span", 1, 1, 1000);
    reflect_str!(def, "width", "width");
});

// ----- embedded / misc --------------------------------------------------------------

simple_class!(HtmlIFrameElement, "HTMLIFrameElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "srcdoc", "srcdoc");
    reflect_str!(def, "name", "name");
    reflect_str!(def, "allow", "allow");
    reflect_str!(def, "width", "width");
    reflect_str!(def, "height", "height");
    reflect_enum!(def, "loading", "loading", LOADING);
    reflect_enum!(def, "referrerPolicy", "referrerpolicy", REFERRER_POLICY);
    reflect_bool!(def, "allowFullscreen", "allowfullscreen");
    reflect_tokens!(def, "sandbox", "sandbox");
    reflect_str!(def, "align", "align");
    reflect_str!(def, "scrolling", "scrolling");
    reflect_str!(def, "frameBorder", "frameborder");
    reflect_url!(def, "longDesc", "longdesc");
    reflect_str_nn!(def, "marginHeight", "marginheight");
    reflect_str_nn!(def, "marginWidth", "marginwidth");
    def.getter("contentDocument", |s, n| {
        if let Some(document) = n.page().frame_document_value(s, n.id)? {
            return Ok(document);
        }
        let doc = n.page().frame_document(n.id);
        node_value(s, n.page(), doc)
    });
    def.getter("contentWindow", |s, n| {
        // Child browsing contexts are owned by pyro-browser; the hook returns
        // the child window proxy when one exists.
        match n.page().frame_window(s, n.id)? {
            Some(w) => Ok(w),
            None => Ok(s.null()),
        }
    });
});
simple_class!(HtmlTemplateElement, "HTMLTemplateElement", HtmlElement, |def| {
    def.getter("content", |s, n| {
        let c = n.page().dom.borrow().template_contents(n.id).js()?;
        node_value(s, n.page(), c)
    });
    def.accessor(
        "shadowRootMode",
        |s, n| {
            let dom = n.page().dom.borrow();
            let value = dom.get_attribute(n.id, "shadowrootmode").js()?.unwrap_or("");
            Ok(s.string(if value.eq_ignore_ascii_case("open") {
                "open"
            } else if value.eq_ignore_ascii_case("closed") {
                "closed"
            } else {
                ""
            }))
        },
        |_s, n, v| {
            let value = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "shadowrootmode", &value).js()
        },
    );
    def.accessor(
        "shadowRootSlotAssignment",
        |s, n| {
            let dom = n.page().dom.borrow();
            let value = dom.get_attribute(n.id, "shadowrootslotassignment").js()?.unwrap_or("");
            Ok(s.string(if value.eq_ignore_ascii_case("manual") { "manual" } else { "named" }))
        },
        |_s, n, v| {
            let value = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "shadowrootslotassignment", &value).js()
        },
    );
    reflect_bool!(def, "shadowRootDelegatesFocus", "shadowrootdelegatesfocus");
    reflect_bool!(def, "shadowRootClonable", "shadowrootclonable");
    reflect_bool!(def, "shadowRootSerializable", "shadowrootserializable");
});

fn slot_assigned<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, elements_only: bool) -> JsResult<Value<'s>> {
    let flatten = match a.get(0).as_object() {
        Some(o) => o.get_atom("flatten")?.to_bool(),
        None => false,
    };
    let nodes = n.page().dom.borrow().assigned_nodes(n.id, flatten).js()?;
    let mut items = Vec::with_capacity(nodes.len());
    for node in nodes {
        if !elements_only || n.page().dom.borrow().node_type(node).js()? == pyro_dom::NodeType::Element {
            items.push(wrap_node(s, n.page(), node)?.value());
        }
    }
    Ok(s.array(&items)?.value())
}
simple_class!(HtmlSlotElement, "HTMLSlotElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    def.method("assignedNodes", 0, |s, n, a| slot_assigned(s, n, a, false));
    def.method("assignedElements", 0, |s, n, a| slot_assigned(s, n, a, true));
    // `assign(...nodes)`: the slot's manually assigned nodes (Elements and Texts).
    def.method("assign", 0, |s, n, a| {
        let mut nodes = Vec::with_capacity(a.len());
        for i in 0..a.len() {
            let node = crate::node::node_arg(s, a.get(i), "HTMLSlotElement.assign")?;
            let kind = n.page().dom.borrow().node_type(node).js()?;
            if !matches!(kind, pyro_dom::NodeType::Element | pyro_dom::NodeType::Text) {
                return Err(JsError::Type(
                    "Failed to execute 'assign' on 'HTMLSlotElement': parameter is not of type '(Element or Text)'."
                        .into(),
                ));
            }
            nodes.push(node);
        }
        n.page().dom.borrow_mut().slot_assign(n.id, &nodes).js()?;
        Ok(s.undefined())
    });
});
simple_class!(HtmlCanvasElement, "HTMLCanvasElement", HtmlElement, |def| {
    // Setting either dimension clears the bitmap and resets the context.
    def.accessor(
        "width",
        |s, n| Ok(s.number(int_attr(n, "width", 0, 300)?)),
        |s, n, v| {
            set_ulong(n, "width", to_u32(v)?, 300)?;
            crate::canvas::canvas_resized(s, n.page(), n.id)
        },
    );
    def.accessor(
        "height",
        |s, n| Ok(s.number(int_attr(n, "height", 0, 150)?)),
        |s, n, v| {
            set_ulong(n, "height", to_u32(v)?, 150)?;
            crate::canvas::canvas_resized(s, n.page(), n.id)
        },
    );
    // "2d": a rasterized 2D context (canvas.rs);
    // other context types are unsupported (null).
    def.method("getContext", 1, |s, n, a| {
        let kind = arg_string(&a, 0, "HTMLCanvasElement.getContext")?;
        crate::canvas::get_context(s, n.page(), n.id, &kind)
    });
    def.method("toDataURL", 0, |s, n, a| crate::canvas::to_data_url(s, n.page(), n.id, a));
    def.method("toBlob", 1, |s, n, a| crate::canvas::to_blob(s, n.page(), n.id, a));
    def.method("transferControlToOffscreen", 0, |s, n, _a| {
        crate::canvas::transfer_control_to_offscreen(s, n.page(), n.id)
    });
});
simple_class!(HtmlMediaElement, "HTMLMediaElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_bool!(def, "autoplay", "autoplay");
    reflect_bool!(def, "loop", "loop");
    reflect_bool!(def, "controls", "controls");
    reflect_bool!(def, "defaultMuted", "muted");
    reflect_cors!(def, "crossOrigin", "crossorigin");
    reflect_enum!(def, "loading", "loading", LOADING);
    reflect_tokens!(def, "controlsList", "controlslist");
    def.accessor(
        "preload",
        |s, n| {
            // The empty string is a synonym of "auto"; the missing and invalid
            // value defaults are "metadata", as in Chromium.
            const PRELOAD: EnumAttr =
                EnumAttr { keywords: &["none", "metadata", "auto"], missing: "metadata", invalid: "metadata" };
            let dom = n.page().dom.borrow();
            let value = dom.get_attribute(n.id, "preload").js()?;
            Ok(s.string(if value == Some("") { "auto" } else { PRELOAD.state(value) }))
        },
        |_s, n, v| set_attr(n, "preload", &dom_string(v)?),
    );
    crate::media::define(def);
});
simple_class!(HtmlVideoElement, "HTMLVideoElement", HtmlMediaElement, |def| {
    reflect_ulong!(def, "width", "width", 0);
    reflect_ulong!(def, "height", "height", 0);
    reflect_url!(def, "poster", "poster");
    reflect_bool!(def, "playsInline", "playsinline");
    def.getter("videoWidth", |s, n| Ok(s.number(f64::from(crate::media::video_size(n).0))));
    def.getter("videoHeight", |s, n| Ok(s.number(f64::from(crate::media::video_size(n).1))));
    crate::media::define_video(def);
});
simple_class!(HtmlAudioElement, "HTMLAudioElement", HtmlMediaElement, |def| {
    // [LegacyFactoryFunction=Audio(optional DOMString src)]
    def.legacy_factory("Audio", 0, |s, new_target, a| {
        let (page, el) = create_factory_element(s, "audio")?;
        // Converted before the DOM is borrowed: `toString` is page script.
        let src = if a.get(0).is_undefined() { None } else { Some(dom_string(a.get(0))?) };
        {
            let mut dom = page.dom.borrow_mut();
            dom.set_attribute(el, "preload", "auto").js()?;
            if let Some(src) = src {
                dom.set_attribute(el, "src", &src).js()?;
            }
        }
        crate::node::wrap_new_node::<Self>(s, &page, el, new_target)
    });
});
simple_class!(HtmlBrElement, "HTMLBRElement", HtmlElement, |def| {
    reflect_str!(def, "clear", "clear");
});
simple_class!(HtmlHrElement, "HTMLHRElement", HtmlElement, |def| {
    reflect_str!(def, "align", "align");
    reflect_str!(def, "color", "color");
    reflect_bool!(def, "noShade", "noshade");
    reflect_str!(def, "size", "size");
    reflect_str!(def, "width", "width");
});
simple_class!(HtmlPreElement, "HTMLPreElement", HtmlElement, |def| {
    reflect_long!(def, "width", "width", 0);
});
simple_class!(HtmlQuoteElement, "HTMLQuoteElement", HtmlElement, |def| {
    reflect_url!(def, "cite", "cite");
});
simple_class!(HtmlDListElement, "HTMLDListElement", HtmlElement, |def| {
    reflect_bool!(def, "compact", "compact");
});
simple_class!(HtmlDialogElement, "HTMLDialogElement", HtmlElement, |def| {
    reflect_bool!(def, "open", "open");
    def.accessor(
        "returnValue",
        |s, n| Ok(s.string(&n.page().dialog_return(n.id))),
        |_s, n, v| {
            n.page().set_dialog_return(n.id, dom_string(v)?);
            Ok(())
        },
    );
    crate::popover::define_dialog(def);
});

simple_class!(HtmlDetailsElement, "HTMLDetailsElement", HtmlElement, |def| {
    reflect_bool!(def, "open", "open");
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlProgressElement, "HTMLProgressElement", HtmlElement, |def| {
    define_labels(def);
    def.accessor(
        "value",
        |s, n| {
            let max = parsed_attr(n, "max", parse_html_float)?.filter(|&m| m > 0.0).unwrap_or(1.0);
            Ok(s.number(parsed_attr(n, "value", parse_html_float)?.unwrap_or(0.0).clamp(0.0, max)))
        },
        |s, n, v| set_double(s, n, "value", v, false),
    );
    reflect_positive_double!(def, "max", "max", 1.0);
});
simple_class!(HtmlMeterElement, "HTMLMeterElement", HtmlElement, |def| {
    define_labels(def);
    macro_rules! meter {
        ($prop:literal) => {
            def.accessor(
                $prop,
                |s, n| Ok(s.number(meter_value(n, $prop)?)),
                |s, n, v| set_double(s, n, $prop, v, false),
            );
        };
    }
    meter!("value");
    meter!("min");
    meter!("max");
    meter!("low");
    meter!("high");
    meter!("optimum");
});
simple_class!(HtmlOutputElement, "HTMLOutputElement", HtmlElement, |def| {
    define_labels(def);
    reflect_str!(def, "name", "name");
    reflect_tokens!(def, "htmlFor", "for");
    def.getter("form", form_value);
    def.accessor(
        "value",
        |s, n| Ok(s.string(&n.page().dom.borrow().text_content(n.id).js()?.unwrap_or_default())),
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_text_content(n.id, &arg0).js()
        },
    );
});
simple_class!(HtmlDataListElement, "HTMLDataListElement", HtmlElement, |def| {
    def.getter("options", |s, n| {
        Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("option".into()))?.value())
    });
});
simple_class!(HtmlPictureElement, "HTMLPictureElement", HtmlElement);
simple_class!(HtmlSourceElement, "HTMLSourceElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "srcset", "srcset");
    reflect_str!(def, "sizes", "sizes");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "media", "media");
});
simple_class!(HtmlTimeElement, "HTMLTimeElement", HtmlElement, |def| {
    reflect_str!(def, "dateTime", "datetime");
});
simple_class!(HtmlDataElement, "HTMLDataElement", HtmlElement, |def| {
    reflect_str!(def, "value", "value");
});
simple_class!(HtmlBaseElement, "HTMLBaseElement", HtmlElement, |def| {
    // Resolved against the fallback base: this element may be the document's base.
    def.accessor(
        "href",
        |s, n| {
            let dom = n.page().dom.borrow();
            let href = dom.get_attribute(n.id, "href").js()?.unwrap_or("");
            let doc = dom.node_document(n.id).js()?;
            let base = pyro_net::Url::parse(dom.document_fallback_base_url(doc).js()?).ok();
            Ok(s.string(&resolve_against(base.as_ref(), href, document_output_encoding(&dom, n.id))))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "href", &arg0).js()
        },
    );
    reflect_str!(def, "target", "target");
});
simple_class!(HtmlObjectElement, "HTMLObjectElement", HtmlElement, |def| {
    reflect_url!(def, "data", "data");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "name", "name");
    reflect_str!(def, "useMap", "usemap");
    reflect_str!(def, "width", "width");
    reflect_str!(def, "height", "height");
    reflect_str!(def, "align", "align");
    reflect_str!(def, "archive", "archive");
    reflect_str!(def, "code", "code");
    reflect_bool!(def, "declare", "declare");
    reflect_ulong!(def, "hspace", "hspace", 0);
    reflect_str!(def, "standby", "standby");
    reflect_ulong!(def, "vspace", "vspace", 0);
    reflect_url!(def, "codeBase", "codebase");
    reflect_str!(def, "codeType", "codetype");
    reflect_str_nn!(def, "border", "border");
    // An object showing a document has a child navigable, as an iframe does (null otherwise).
    def.getter("contentDocument", |s, n| {
        if let Some(document) = n.page().frame_document_value(s, n.id)? {
            return Ok(document);
        }
        let doc = n.page().frame_document(n.id);
        node_value(s, n.page(), doc)
    });
    def.getter("contentWindow", |s, n| match n.page().frame_window(s, n.id)? {
        Some(w) => Ok(w),
        None => Ok(s.null()),
    });
});
simple_class!(HtmlEmbedElement, "HTMLEmbedElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "width", "width");
    reflect_str!(def, "height", "height");
    reflect_str!(def, "align", "align");
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlMapElement, "HTMLMapElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlModElement, "HTMLModElement", HtmlElement, |def| {
    reflect_url!(def, "cite", "cite");
    reflect_str!(def, "dateTime", "datetime");
});
simple_class!(HtmlMenuElement, "HTMLMenuElement", HtmlElement, |def| {
    reflect_bool!(def, "compact", "compact");
});
simple_class!(HtmlMarqueeElement, "HTMLMarqueeElement", HtmlElement, |def| {
    reflect_enum!(def, "behavior", "behavior", MARQUEE_BEHAVIOR);
    reflect_str!(def, "bgColor", "bgcolor");
    reflect_enum!(def, "direction", "direction", MARQUEE_DIRECTION);
    reflect_str!(def, "height", "height");
    reflect_ulong!(def, "hspace", "hspace", 0);
    reflect_ulong!(def, "scrollAmount", "scrollamount", 6);
    reflect_ulong!(def, "scrollDelay", "scrolldelay", 85);
    reflect_bool!(def, "trueSpeed", "truespeed");
    reflect_ulong!(def, "vspace", "vspace", 0);
    reflect_str!(def, "width", "width");
});
simple_class!(HtmlFrameSetElement, "HTMLFrameSetElement", HtmlElement, |def| {
    crate::handlers::define_window_reflecting(def);
    reflect_str!(def, "cols", "cols");
    reflect_str!(def, "rows", "rows");
});
simple_class!(HtmlFrameElement, "HTMLFrameElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    reflect_str!(def, "scrolling", "scrolling");
    reflect_url!(def, "src", "src");
    reflect_str!(def, "frameBorder", "frameborder");
    reflect_url!(def, "longDesc", "longdesc");
    reflect_bool!(def, "noResize", "noresize");
    reflect_str_nn!(def, "marginHeight", "marginheight");
    reflect_str_nn!(def, "marginWidth", "marginwidth");
    // A connected frame has a child navigable, as an iframe does.
    def.getter("contentDocument", |s, n| {
        if let Some(document) = n.page().frame_document_value(s, n.id)? {
            return Ok(document);
        }
        let doc = n.page().frame_document(n.id);
        node_value(s, n.page(), doc)
    });
    def.getter("contentWindow", |s, n| match n.page().frame_window(s, n.id)? {
        Some(w) => Ok(w),
        None => Ok(s.null()),
    });
});
simple_class!(HtmlFontElement, "HTMLFontElement", HtmlElement, |def| {
    reflect_str_nn!(def, "color", "color");
    reflect_str!(def, "face", "face");
    reflect_str!(def, "size", "size");
});
simple_class!(HtmlDirectoryElement, "HTMLDirectoryElement", HtmlElement, |def| {
    reflect_bool!(def, "compact", "compact");
});
simple_class!(HtmlParamElement, "HTMLParamElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    reflect_str!(def, "value", "value");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "valueType", "valuetype");
});
simple_class!(HtmlTrackElement, "HTMLTrackElement", HtmlElement, |def| {
    reflect_enum!(def, "kind", "kind", TRACK_KIND);
    reflect_url!(def, "src", "src");
    reflect_str!(def, "srclang", "srclang");
    reflect_str!(def, "label", "label");
    reflect_bool!(def, "default", "default");
});

/// HTMLOrSVGOrMathMLElement reflects both focus attributes in no namespace.
fn define_focus_attributes<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.accessor(
        "autofocus",
        |s, n| Ok(s.boolean(n.page().dom.borrow().get_attribute_ns(n.id, None, "autofocus").js()?.is_some())),
        |_s, n, value| {
            let mut dom = n.page().dom.borrow_mut();
            if value.to_bool() {
                dom.set_attribute_local(n.id, &markup5ever::local_name!("autofocus"), "").js()
            } else {
                dom.remove_attribute_ns(n.id, None, "autofocus").js().map(|_| ())
            }
        },
    );
    def.accessor(
        "tabIndex",
        |s, n| Ok(s.number(n.page().dom.borrow().tab_index(n.id).js()? as f64)),
        |_s, n, value| {
            let index = to_i32(value)?;
            n.page().dom.borrow_mut().set_attribute_local(n.id, &markup5ever::local_name!("tabindex"), &index.to_string()).js()
        },
    );
}

pub struct SvgElement;
impl JsClass for SvgElement {
    const NAME: &'static str = "SVGElement";
    type Data = NodeData;
    type Parent = Element;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("className", |s, n| {
            Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &SVG_CLASS)?.value())
        });
        crate::handlers::define_global(def);
        def.getter("dataset", |s, n| Ok(collections::dataset(s, n.page(), n.id)?.value()));
        // [PutForwards=cssText]
        def.put_forwards("style", |s, n| Ok(crate::css::inline_style(s, n.page(), n.id)?.value()), "cssText");
        // CSS Typed OM: the inline declarations as a StylePropertyMap.
        def.getter("attributeStyleMap", |s, n| {
            let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
            crate::typed_om::attribute_style_map(s, n.page(), el)
        });
        def.getter("ownerSVGElement", |s, n| {
            let dom = n.page().dom.borrow();
            let mut p = dom.parent_element(n.id).js()?;
            while let Some(x) = p {
                if dom.element(x).is_ok_and(|e| &*e.name.local == "svg") {
                    drop(dom);
                    return node_value(s, n.page(), Some(x));
                }
                p = dom.parent_element(x).js()?;
            }
            Ok(s.null())
        });
        def.method("focus", 0, focus_element);
        def.method("blur", 0, |s, n, _a| {
            crate::input::blur_element(n.page(), n.id);
            Ok(s.undefined())
        });
        define_focus_attributes(def);
    }
}
/// MathML Core `MathMLElement`: an element in the MathML namespace, with
/// `HTMLOrSVGElement`, `ElementCSSInlineStyle` and the event handlers.
pub struct MathMlElement;
impl JsClass for MathMlElement {
    const NAME: &'static str = "MathMLElement";
    type Data = NodeData;
    type Parent = Element;
    fn define(def: &mut ClassDef<Self>) {
        crate::handlers::define_global(def);
        def.getter("dataset", |s, n| Ok(collections::dataset(s, n.page(), n.id)?.value()));
        // [PutForwards=cssText]
        def.put_forwards("style", |s, n| Ok(crate::css::inline_style(s, n.page(), n.id)?.value()), "cssText");
        // CSS Typed OM: the inline declarations as a StylePropertyMap.
        def.getter("attributeStyleMap", |s, n| {
            let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
            crate::typed_om::attribute_style_map(s, n.page(), el)
        });
        define_focus_attributes(def);
        def.method("focus", 0, focus_element);
        def.method("blur", 0, |s, n, _a| {
            crate::input::blur_element(n.page(), n.id);
            Ok(s.undefined())
        });
    }
}
simple_class!(SvgGraphicsElement, "SVGGraphicsElement", SvgElement, |def| {
    use pyro_dom::svg_string_list::StringListAttribute;
    def.getter("requiredExtensions", |s, n| collections::svg_string_list(s, n, StringListAttribute::RequiredExtensions));
    def.getter("systemLanguage", |s, n| collections::svg_string_list(s, n, StringListAttribute::SystemLanguage));
    // pyro: `transform` (SVG 2 `SVGAnimatedTransformList`, the attribute's list).
    def.getter("transform", |s, n| Ok(collections::animated_transform_list(s, n.page(), n.id, "transform")?.value()));
    // No rasterizer, but geometry is attribute-defined: the user-space box
    // of shapes, paths, text and containers (pyro_dom::svg_geometry).
    def.method("getBBox", 0, |s, n, _a| {
        let page = n.page();
        let dom = page.dom.borrow();
        let layout = page.layout();
        crate::canvas::rect_value(s, page, false, pyro_dom::svg_geometry::bbox(&dom, layout.style(), n.id))
    });
});
fn svg_aspect_ratio<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("preserveAspectRatio", |s, n| collections::animated_aspect(s, n));
}

simple_class!(SvgSymbolElement, "SVGSymbolElement", SvgGraphicsElement, svg_aspect_ratio);
simple_class!(SvgViewElement, "SVGViewElement", SvgElement, svg_aspect_ratio);
simple_class!(SvgSvgElement, "SVGSVGElement", SvgGraphicsElement, |def| {
    svg_aspect_ratio(def);
    def.method("createSVGPoint", 0, |s, _, _| collections::detached_point(s, 0.0, 0.0));
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    // SVG time control (SVG 1.1 `SVGSVGElement`), of the document's SMIL clock (`smil`).
    def.method("setCurrentTime", 1, |s, n, a| {
        a.require(1, "SVGSVGElement.setCurrentTime")?;
        let seconds = a.get(0).to_number()?;
        if !seconds.is_finite() {
            return Err(JsError::Type(
                "Failed to execute 'setCurrentTime' on 'SVGSVGElement': The provided float value is non-finite.".into(),
            ));
        }
        crate::smil::set_current_time(n.page(), seconds);
        Ok(s.undefined())
    });
    def.method("getCurrentTime", 0, |s, n, _a| Ok(s.number(crate::smil::current_time(n.page()))));
    def.method("pauseAnimations", 0, |s, n, _a| {
        crate::smil::set_paused(n.page(), true);
        Ok(s.undefined())
    });
    def.method("unpauseAnimations", 0, |s, n, _a| {
        crate::smil::set_paused(n.page(), false);
        Ok(s.undefined())
    });
    def.method("animationsPaused", 0, |s, n, _a| Ok(s.boolean(crate::smil::paused(n.page()))));
    def.method("createSVGNumber", 0, |s, _n, _a| {
        collections::number_value(s, pyro_dom::svg_point_list::NumberItem::detached(0.0))
    });
    def.method("createSVGAngle", 0, |s, _n, _a| Ok(collections::create_svg_angle(s)?.value()));
    def.method("createSVGLength", 0, |s, n, _a| {
        Ok(collections::create_svg_length(s, n.id)?.value())
    });
    // pyro: the user agent's pan and zoom (SVG 2 `currentTranslate`, `currentScale`), which the
    // renderer applies to the `<svg>`'s content (pyro-dom `SVG_PAN_ATTR`).
    def.getter("currentTranslate", |s, n| {
        Ok(s.wrap::<collections::SvgPoint>(collections::SvgPointData::Translate(collections::NodeHold::new(n.page(), n.id)?), None)?.value())
    });
    def.accessor(
        "currentScale",
        |s, n| Ok(s.number(n.page().dom.borrow().svg_pan(n.id).2 as f32 as f64)),
        |_s, n, v| {
            let scale = v.to_number()?;
            if !scale.is_finite() {
                return Err(JsError::Type("The provided float value is non-finite.".into()));
            }
            let mut dom = n.page().dom.borrow_mut();
            let (tx, ty, _) = dom.svg_pan(n.id);
            dom.set_svg_pan(n.id, (tx, ty, scale as f32 as f64));
            Ok(())
        },
    );
});
// SVG 2 `<length>` attributes as `SVGAnimatedLength`s (`collections::SvgLengthAttr`: the attribute,
// its initial value and the direction its percentages resolve in).
static LEN_300: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x", initial: "0%", dir: collections::SvgLengthDir::Width, slot: 300 };
static LEN_303: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y", initial: "0%", dir: collections::SvgLengthDir::Height, slot: 303 };
static LEN_306: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "width", initial: "100%", dir: collections::SvgLengthDir::Width, slot: 306 };
static LEN_309: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "height", initial: "100%", dir: collections::SvgLengthDir::Height, slot: 309 };
static LEN_312: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x", initial: "-10%", dir: collections::SvgLengthDir::Width, slot: 312 };
static LEN_315: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y", initial: "-10%", dir: collections::SvgLengthDir::Height, slot: 315 };
static LEN_318: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "width", initial: "120%", dir: collections::SvgLengthDir::Width, slot: 318 };
static LEN_321: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "height", initial: "120%", dir: collections::SvgLengthDir::Height, slot: 321 };
static LEN_324: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x", initial: "0", dir: collections::SvgLengthDir::Width, slot: 324 };
static LEN_327: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y", initial: "0", dir: collections::SvgLengthDir::Height, slot: 327 };
static LEN_330: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "width", initial: "0", dir: collections::SvgLengthDir::Width, slot: 330 };
static LEN_333: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "height", initial: "0", dir: collections::SvgLengthDir::Height, slot: 333 };
static LEN_336: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x1", initial: "0%", dir: collections::SvgLengthDir::Width, slot: 336 };
static LEN_339: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y1", initial: "0%", dir: collections::SvgLengthDir::Height, slot: 339 };
static LEN_342: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x2", initial: "100%", dir: collections::SvgLengthDir::Width, slot: 342 };
static LEN_345: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y2", initial: "0%", dir: collections::SvgLengthDir::Height, slot: 345 };
static LEN_348: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "cx", initial: "50%", dir: collections::SvgLengthDir::Width, slot: 348 };
static LEN_351: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "cy", initial: "50%", dir: collections::SvgLengthDir::Height, slot: 351 };
static LEN_354: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "r", initial: "50%", dir: collections::SvgLengthDir::Other, slot: 354 };
static LEN_357: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "fx", initial: "50%", dir: collections::SvgLengthDir::Width, slot: 357 };
static LEN_360: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "fy", initial: "50%", dir: collections::SvgLengthDir::Height, slot: 360 };
static LEN_363: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "fr", initial: "0%", dir: collections::SvgLengthDir::Other, slot: 363 };
static LEN_366: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "refX", initial: "0", dir: collections::SvgLengthDir::Width, slot: 366 };
static LEN_369: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "refY", initial: "0", dir: collections::SvgLengthDir::Height, slot: 369 };
static LEN_372: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "markerWidth", initial: "3", dir: collections::SvgLengthDir::Width, slot: 372 };
static LEN_375: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "markerHeight", initial: "3", dir: collections::SvgLengthDir::Height, slot: 375 };
static LEN_378: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "width", initial: "auto", dir: collections::SvgLengthDir::Width, slot: 378 };
static LEN_381: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "height", initial: "auto", dir: collections::SvgLengthDir::Height, slot: 381 };
static LEN_384: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "rx", initial: "0", dir: collections::SvgLengthDir::Width, slot: 384 };
static LEN_387: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "ry", initial: "0", dir: collections::SvgLengthDir::Height, slot: 387 };
static LEN_390: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "cx", initial: "0", dir: collections::SvgLengthDir::Width, slot: 390 };
static LEN_393: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "cy", initial: "0", dir: collections::SvgLengthDir::Height, slot: 393 };
static LEN_396: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "r", initial: "0", dir: collections::SvgLengthDir::Other, slot: 396 };
static LEN_399: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x1", initial: "0", dir: collections::SvgLengthDir::Width, slot: 399 };
static LEN_402: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y1", initial: "0", dir: collections::SvgLengthDir::Height, slot: 402 };
static LEN_405: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "x2", initial: "0", dir: collections::SvgLengthDir::Width, slot: 405 };
static LEN_408: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "y2", initial: "0", dir: collections::SvgLengthDir::Height, slot: 408 };

fn svg_href<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("href", |s, n| {
        Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &SVG_HREF)?.value())
    });
}

static SVG_CLASS: collections::SvgAttr = collections::SvgAttr { attrs: &["class"], part: collections::SvgPart::Whole, initial: 0.0, slot: 420 };
static LINK_TARGET: collections::SvgAttr = collections::SvgAttr { attrs: &["target"], part: collections::SvgPart::Whole, initial: 0.0, slot: 431 };
static PATH_LENGTH: collections::SvgAttr = collections::SvgAttr { attrs: &["pathLength"], part: collections::SvgPart::Whole, initial: 0.0, slot: 421 };
static STOP_OFFSET: collections::SvgAttr = collections::SvgAttr { attrs: &["offset"], part: collections::SvgPart::NumberOrPercentage, initial: 0.0, slot: 422 };

simple_class!(SvgGeometryElement, "SVGGeometryElement", SvgGraphicsElement, |def| {
    def.method("getTotalLength", 0, |s, n, _| {
        Ok(s.number(pyro_dom::svg_geometry::total_length(&n.page().dom.borrow(), &n.page().layout(), n.id) as f32 as f64))
    });
    def.method("getPointAtLength", 1, |s, n, args| {
        args.require(1, "SVGGeometryElement.getPointAtLength")?;
        let distance = args.get(0).to_number()? as f32;
        if !distance.is_finite() {
            return Err(JsError::Type("The distance must be a finite float.".into()));
        }
        let point = pyro_dom::svg_geometry::point_at_length(&n.page().dom.borrow(), &n.page().layout(), n.id, f64::from(distance))
            .ok_or_else(|| JsError::dom("InvalidStateError", "The element has no path to measure."))?;
        collections::detached_point(s, point.x, point.y)
    });
    def.getter("pathLength", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &PATH_LENGTH)?.value()));
});
simple_class!(SvgPathElement, "SVGPathElement", SvgGeometryElement);
fn svg_animated_points<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("points", |s, n| collections::point_list(s, n, false));
    def.getter("animatedPoints", |s, n| collections::point_list(s, n, true));
}
simple_class!(SvgPolylineElement, "SVGPolylineElement", SvgGeometryElement, |def| { svg_animated_points(def); });
simple_class!(SvgPolygonElement, "SVGPolygonElement", SvgGeometryElement, |def| { svg_animated_points(def); });
simple_class!(SvgStopElement, "SVGStopElement", SvgElement, |def| {
    def.getter("offset", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &STOP_OFFSET)?.value()));
});
// The graphics elements that are boxes (SVG 2 `image`, `foreignObject`, `use`): their geometry as
// SVGAnimatedLengths. (The elements were plain SVGGraphicsElements.)
static LEN_411: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "textLength", initial: "0", dir: collections::SvgLengthDir::Other, slot: 411 };
static LEN_414: collections::SvgLengthAttr = collections::SvgLengthAttr { attr: "startOffset", initial: "0", dir: collections::SvgLengthDir::Other, slot: 414 };
simple_class!(SvgImageElement, "SVGImageElement", SvgGraphicsElement, |def| {
    svg_aspect_ratio(def);
    svg_href(def);
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_330)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_333)?.value()));
});
simple_class!(SvgForeignObjectElement, "SVGForeignObjectElement", SvgGraphicsElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_330)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_333)?.value()));
});
simple_class!(SvgUseElement, "SVGUseElement", SvgGraphicsElement, |def| {
    svg_href(def);
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_330)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_333)?.value()));
});
// SVG 2 text content elements: `textLength` (and `startOffset` of a `textPath`) as
// SVGAnimatedLengths. Not implemented yet: the text measuring methods and the
// `x`, `y`, `dx`, `dy` and `rotate` lists of the positioning elements.
static LENGTH_ADJUST: collections::SvgEnum = collections::SvgEnum { attr: "lengthAdjust", keywords: &["spacing", "spacingAndGlyphs"], initial: 1, slot: 423 };
static TEXT_METHOD: collections::SvgEnum = collections::SvgEnum { attr: "method", keywords: &["align", "stretch"], initial: 1, slot: 424 };
static TEXT_SPACING: collections::SvgEnum = collections::SvgEnum { attr: "spacing", keywords: &["auto", "exact"], initial: 2, slot: 425 };
static TEXT_SIDE: collections::SvgEnum = collections::SvgEnum { attr: "side", keywords: &["left", "right"], initial: 1, slot: 426 };
static MARKER_ORIENT: collections::SvgEnum = collections::SvgEnum { attr: "orient", keywords: &["auto", "0", "auto-start-reverse"], initial: 2, slot: 427 };

macro_rules! svg_enum {
    ($def:expr, $prop:literal, $e:expr) => {
        $def.getter($prop, |s, n| Ok(collections::animated_enumeration(s, n.page(), n.id, &$e)?.value()));
    };
}

simple_class!(SvgTextContentElement, "SVGTextContentElement", SvgGraphicsElement, |def| {
    def.getter("textLength", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_411)?.value()));
    svg_enum!(def, "lengthAdjust", LENGTH_ADJUST);
    def.constant("LENGTHADJUST_UNKNOWN", 0.0);
    def.constant("LENGTHADJUST_SPACING", 1.0);
    def.constant("LENGTHADJUST_SPACINGANDGLYPHS", 2.0);
});
simple_class!(SvgTextPositioningElement, "SVGTextPositioningElement", SvgTextContentElement, |def| {
    def.getter("rotate", |s, n| collections::animated_number_list(s, n, "rotate"));
});
simple_class!(SvgTextElement, "SVGTextElement", SvgTextPositioningElement);
simple_class!(SvgTSpanElement, "SVGTSpanElement", SvgTextPositioningElement);
simple_class!(SvgTextPathElement, "SVGTextPathElement", SvgTextContentElement, |def| {
    svg_href(def);
    def.getter("startOffset", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_414)?.value()));
    svg_enum!(def, "method", TEXT_METHOD);
    svg_enum!(def, "spacing", TEXT_SPACING);
    svg_enum!(def, "side", TEXT_SIDE);
    def.constant("TEXTPATH_SIDETYPE_UNKNOWN", 0.0);
    def.constant("TEXTPATH_SIDETYPE_LEFT", 1.0);
    def.constant("TEXTPATH_SIDETYPE_RIGHT", 2.0);
    def.constant("TEXTPATH_METHODTYPE_UNKNOWN", 0.0);
    def.constant("TEXTPATH_METHODTYPE_ALIGN", 1.0);
    def.constant("TEXTPATH_METHODTYPE_STRETCH", 2.0);
    def.constant("TEXTPATH_SPACINGTYPE_UNKNOWN", 0.0);
    def.constant("TEXTPATH_SPACINGTYPE_AUTO", 1.0);
    def.constant("TEXTPATH_SPACINGTYPE_EXACT", 2.0);
});
simple_class!(SvgRectElement, "SVGRectElement", SvgGeometryElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_378)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_381)?.value()));
    def.getter("rx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_384)?.value()));
    def.getter("ry", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_387)?.value()));
});
simple_class!(SvgCircleElement, "SVGCircleElement", SvgGeometryElement, |def| {
    def.getter("cx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_390)?.value()));
    def.getter("cy", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_393)?.value()));
    def.getter("r", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_396)?.value()));
});
simple_class!(SvgEllipseElement, "SVGEllipseElement", SvgGeometryElement, |def| {
    def.getter("cx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_390)?.value()));
    def.getter("cy", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_393)?.value()));
    def.getter("rx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_384)?.value()));
    def.getter("ry", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_387)?.value()));
});
simple_class!(SvgLineElement, "SVGLineElement", SvgGeometryElement, |def| {
    def.getter("x1", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_399)?.value()));
    def.getter("y1", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_402)?.value()));
    def.getter("x2", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_405)?.value()));
    def.getter("y2", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_408)?.value()));
});

// SVG 2 elements with enumerated attributes, as `SVGAnimatedEnumeration`s
// (`collections::SvgEnum`: the keywords, in the order of their values from 1).
const UNITS: &[&str] = &["userSpaceOnUse", "objectBoundingBox"];
static CLIP_PATH_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "clipPathUnits", keywords: UNITS, initial: 1, slot: 22 };
static MASK_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "maskUnits", keywords: UNITS, initial: 2, slot: 23 };
static MASK_CONTENT_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "maskContentUnits", keywords: UNITS, initial: 1, slot: 24 };
static PATTERN_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "patternUnits", keywords: UNITS, initial: 2, slot: 25 };
static PATTERN_CONTENT_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "patternContentUnits", keywords: UNITS, initial: 1, slot: 26 };
static GRADIENT_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "gradientUnits", keywords: UNITS, initial: 2, slot: 27 };
static SPREAD_METHOD: collections::SvgEnum =
    collections::SvgEnum { attr: "spreadMethod", keywords: &["pad", "reflect", "repeat"], initial: 1, slot: 28 };
static FILTER_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "filterUnits", keywords: UNITS, initial: 2, slot: 29 };
static PRIMITIVE_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "primitiveUnits", keywords: UNITS, initial: 1, slot: 30 };
static MARKER_UNITS: collections::SvgEnum =
    collections::SvgEnum { attr: "markerUnits", keywords: &["userSpaceOnUse", "strokeWidth"], initial: 2, slot: 31 };

/// The `SVGUnitTypes` constants, which the elements with units attributes include.
fn unit_type_constants<C: JsClass>(def: &mut ClassDef<C>) {
    def.constant("SVG_UNIT_TYPE_UNKNOWN", 0.0);
    def.constant("SVG_UNIT_TYPE_USERSPACEONUSE", 1.0);
    def.constant("SVG_UNIT_TYPE_OBJECTBOUNDINGBOX", 2.0);
}

pub struct SvgUnitTypes;
impl JsClass for SvgUnitTypes {
    const NAME: &'static str = "SVGUnitTypes";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        unit_type_constants(def);
    }
}

simple_class!(SvgClipPathElement, "SVGClipPathElement", SvgElement, |def| {
    unit_type_constants(def);
    svg_enum!(def, "clipPathUnits", CLIP_PATH_UNITS);
});
simple_class!(SvgMaskElement, "SVGMaskElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_312)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_315)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_318)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_321)?.value()));
    unit_type_constants(def);
    svg_enum!(def, "maskUnits", MASK_UNITS);
    svg_enum!(def, "maskContentUnits", MASK_CONTENT_UNITS);
});
simple_class!(SvgPatternElement, "SVGPatternElement", SvgElement, |def| {
    svg_aspect_ratio(def);
    svg_href(def);
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_324)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_327)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_330)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_333)?.value()));
    unit_type_constants(def);
    svg_enum!(def, "patternUnits", PATTERN_UNITS);
    svg_enum!(def, "patternContentUnits", PATTERN_CONTENT_UNITS);
});
simple_class!(SvgGradientElement, "SVGGradientElement", SvgElement, |def| {
    svg_href(def);
    unit_type_constants(def);
    def.constant("SVG_SPREADMETHOD_UNKNOWN", 0.0);
    def.constant("SVG_SPREADMETHOD_PAD", 1.0);
    def.constant("SVG_SPREADMETHOD_REFLECT", 2.0);
    def.constant("SVG_SPREADMETHOD_REPEAT", 3.0);
    svg_enum!(def, "gradientUnits", GRADIENT_UNITS);
    svg_enum!(def, "spreadMethod", SPREAD_METHOD);
});
simple_class!(SvgLinearGradientElement, "SVGLinearGradientElement", SvgGradientElement, |def| {
    def.getter("x1", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_336)?.value()));
    def.getter("y1", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_339)?.value()));
    def.getter("x2", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_342)?.value()));
    def.getter("y2", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_345)?.value()));
});
simple_class!(SvgRadialGradientElement, "SVGRadialGradientElement", SvgGradientElement, |def| {
    def.getter("cx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_348)?.value()));
    def.getter("cy", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_351)?.value()));
    def.getter("r", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_354)?.value()));
    def.getter("fx", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_357)?.value()));
    def.getter("fy", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_360)?.value()));
    def.getter("fr", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_363)?.value()));
});
simple_class!(SvgFilterElement, "SVGFilterElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_312)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_315)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_318)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_321)?.value()));
    unit_type_constants(def);
    svg_enum!(def, "filterUnits", FILTER_UNITS);
    svg_enum!(def, "primitiveUnits", PRIMITIVE_UNITS);
});
simple_class!(SvgMarkerElement, "SVGMarkerElement", SvgElement, |def| {
    svg_aspect_ratio(def);
    def.getter("refX", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_366)?.value()));
    def.getter("refY", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_369)?.value()));
    def.getter("markerWidth", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_372)?.value()));
    def.getter("markerHeight", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_375)?.value()));
    def.constant("SVG_MARKERUNITS_UNKNOWN", 0.0);
    def.constant("SVG_MARKERUNITS_USERSPACEONUSE", 1.0);
    def.constant("SVG_MARKERUNITS_STROKEWIDTH", 2.0);
    def.constant("SVG_MARKER_ORIENT_UNKNOWN", 0.0);
    def.constant("SVG_MARKER_ORIENT_AUTO", 1.0);
    def.constant("SVG_MARKER_ORIENT_ANGLE", 2.0);
    def.constant("SVG_MARKER_ORIENT_AUTO_START_REVERSE", 3.0);
    svg_enum!(def, "markerUnits", MARKER_UNITS);
    svg_enum!(def, "orientType", MARKER_ORIENT);
    def.getter("orientAngle", |s, n| Ok(collections::animated_angle(s, n.page(), n.id)?.value()));
    def.method("setOrientToAuto", 0, |s, n, _a| {
        n.page().dom.borrow_mut().set_attribute_ns(n.id, None, "orient", "auto").js()?;
        Ok(s.undefined())
    });
    def.method("setOrientToAngle", 1, |s, n, a| {
        a.require(1, "SVGMarkerElement.setOrientToAngle")?;
        let angle = collections::angle_string(s, a.get(0))?;
        n.page().dom.borrow_mut().set_attribute_ns(n.id, None, "orient", &angle).js()?;
        Ok(s.undefined())
    });
    def.accessor(
        "orient",
        |s, n| Ok(s.string(n.page().dom.borrow().get_attribute_ns(n.id, None, "orient").js()?.unwrap_or(""))),
        |_s, n, v| {
            let value = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute_ns(n.id, None, "orient", &value).js()
        },
    );
});

// SVG filter primitives (Filter Effects 1): their attributes as SVGAnimatedEnumeration, -String,
// -Number, -Integer and -Boolean values (`collections`), following the specification's IDL.
// The number lists (`values`, `tableValues`, `kernelMatrix`) are not yet implemented.
static FE_BLEND_MODE: collections::SvgEnum =
    collections::SvgEnum { attr: "mode", keywords: &["normal", "multiply", "screen", "darken", "lighten", "overlay", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color", "luminosity"], initial: 1, slot: 40 };
static FE_COLOR_MATRIX_TYPE: collections::SvgEnum =
    collections::SvgEnum { attr: "type", keywords: &["matrix", "saturate", "hueRotate", "luminanceToAlpha"], initial: 1, slot: 41 };
static FE_TRANSFER_TYPE: collections::SvgEnum =
    collections::SvgEnum { attr: "type", keywords: &["identity", "table", "discrete", "linear", "gamma"], initial: 1, slot: 42 };
static FE_COMPOSITE_OPERATOR: collections::SvgEnum =
    collections::SvgEnum { attr: "operator", keywords: &["over", "in", "out", "atop", "xor", "arithmetic"], initial: 1, slot: 43 };
static FE_CONVOLVE_EDGE_MODE: collections::SvgEnum =
    collections::SvgEnum { attr: "edgeMode", keywords: &["duplicate", "wrap", "none"], initial: 1, slot: 44 };
static FE_BLUR_EDGE_MODE: collections::SvgEnum =
    collections::SvgEnum { attr: "edgeMode", keywords: &["duplicate", "wrap", "none"], initial: 3, slot: 45 };
static FE_X_CHANNEL: collections::SvgEnum =
    collections::SvgEnum { attr: "xChannelSelector", keywords: &["R", "G", "B", "A"], initial: 4, slot: 46 };
static FE_Y_CHANNEL: collections::SvgEnum =
    collections::SvgEnum { attr: "yChannelSelector", keywords: &["R", "G", "B", "A"], initial: 4, slot: 47 };
static FE_MORPHOLOGY_OPERATOR: collections::SvgEnum =
    collections::SvgEnum { attr: "operator", keywords: &["erode", "dilate"], initial: 1, slot: 48 };
static FE_TURBULENCE_TYPE: collections::SvgEnum =
    collections::SvgEnum { attr: "type", keywords: &["fractalNoise", "turbulence"], initial: 2, slot: 49 };
static FE_STITCH_TILES: collections::SvgEnum =
    collections::SvgEnum { attr: "stitchTiles", keywords: &["stitch", "noStitch"], initial: 2, slot: 50 };
static FE_ATTR_100: collections::SvgAttr = collections::SvgAttr { attrs: &["in"], part: collections::SvgPart::Whole, initial: 0.0, slot: 100 };
static FE_ATTR_101: collections::SvgAttr = collections::SvgAttr { attrs: &["in2"], part: collections::SvgPart::Whole, initial: 0.0, slot: 101 };
static FE_ATTR_102: collections::SvgAttr = collections::SvgAttr { attrs: &["result"], part: collections::SvgPart::Whole, initial: 0.0, slot: 102 };
static FE_ATTR_103: collections::SvgAttr = collections::SvgAttr { attrs: &["slope"], part: collections::SvgPart::Whole, initial: 1.0, slot: 103 };
static FE_ATTR_104: collections::SvgAttr = collections::SvgAttr { attrs: &["intercept"], part: collections::SvgPart::Whole, initial: 0.0, slot: 104 };
static FE_ATTR_105: collections::SvgAttr = collections::SvgAttr { attrs: &["amplitude"], part: collections::SvgPart::Whole, initial: 1.0, slot: 105 };
static FE_ATTR_106: collections::SvgAttr = collections::SvgAttr { attrs: &["exponent"], part: collections::SvgPart::Whole, initial: 1.0, slot: 106 };
static FE_ATTR_107: collections::SvgAttr = collections::SvgAttr { attrs: &["offset"], part: collections::SvgPart::Whole, initial: 0.0, slot: 107 };
static FE_ATTR_108: collections::SvgAttr = collections::SvgAttr { attrs: &["k1"], part: collections::SvgPart::Whole, initial: 0.0, slot: 108 };
static FE_ATTR_109: collections::SvgAttr = collections::SvgAttr { attrs: &["k2"], part: collections::SvgPart::Whole, initial: 0.0, slot: 109 };
static FE_ATTR_110: collections::SvgAttr = collections::SvgAttr { attrs: &["k3"], part: collections::SvgPart::Whole, initial: 0.0, slot: 110 };
static FE_ATTR_111: collections::SvgAttr = collections::SvgAttr { attrs: &["k4"], part: collections::SvgPart::Whole, initial: 0.0, slot: 111 };
static FE_ATTR_112: collections::SvgAttr = collections::SvgAttr { attrs: &["order"], part: collections::SvgPart::First, initial: 3.0, slot: 112 };
static FE_ATTR_113: collections::SvgAttr = collections::SvgAttr { attrs: &["order"], part: collections::SvgPart::Second, initial: 3.0, slot: 113 };
static FE_ATTR_114: collections::SvgAttr = collections::SvgAttr { attrs: &["divisor"], part: collections::SvgPart::Whole, initial: 1.0, slot: 114 };
static FE_ATTR_115: collections::SvgAttr = collections::SvgAttr { attrs: &["bias"], part: collections::SvgPart::Whole, initial: 0.0, slot: 115 };
static FE_ATTR_116: collections::SvgAttr = collections::SvgAttr { attrs: &["targetX"], part: collections::SvgPart::Whole, initial: 0.0, slot: 116 };
static FE_ATTR_117: collections::SvgAttr = collections::SvgAttr { attrs: &["targetY"], part: collections::SvgPart::Whole, initial: 0.0, slot: 117 };
static FE_ATTR_118: collections::SvgAttr = collections::SvgAttr { attrs: &["kernelUnitLength"], part: collections::SvgPart::First, initial: 0.0, slot: 118 };
static FE_ATTR_119: collections::SvgAttr = collections::SvgAttr { attrs: &["kernelUnitLength"], part: collections::SvgPart::Second, initial: 0.0, slot: 119 };
static FE_ATTR_120: collections::SvgAttr = collections::SvgAttr { attrs: &["preserveAlpha"], part: collections::SvgPart::Whole, initial: 0.0, slot: 120 };
static FE_ATTR_121: collections::SvgAttr = collections::SvgAttr { attrs: &["surfaceScale"], part: collections::SvgPart::Whole, initial: 1.0, slot: 121 };
static FE_ATTR_122: collections::SvgAttr = collections::SvgAttr { attrs: &["diffuseConstant"], part: collections::SvgPart::Whole, initial: 1.0, slot: 122 };
static FE_ATTR_123: collections::SvgAttr = collections::SvgAttr { attrs: &["azimuth"], part: collections::SvgPart::Whole, initial: 0.0, slot: 123 };
static FE_ATTR_124: collections::SvgAttr = collections::SvgAttr { attrs: &["elevation"], part: collections::SvgPart::Whole, initial: 0.0, slot: 124 };
static FE_ATTR_125: collections::SvgAttr = collections::SvgAttr { attrs: &["x"], part: collections::SvgPart::Whole, initial: 0.0, slot: 125 };
static FE_ATTR_126: collections::SvgAttr = collections::SvgAttr { attrs: &["y"], part: collections::SvgPart::Whole, initial: 0.0, slot: 126 };
static FE_ATTR_127: collections::SvgAttr = collections::SvgAttr { attrs: &["z"], part: collections::SvgPart::Whole, initial: 0.0, slot: 127 };
static FE_ATTR_128: collections::SvgAttr = collections::SvgAttr { attrs: &["pointsAtX"], part: collections::SvgPart::Whole, initial: 0.0, slot: 128 };
static FE_ATTR_129: collections::SvgAttr = collections::SvgAttr { attrs: &["pointsAtY"], part: collections::SvgPart::Whole, initial: 0.0, slot: 129 };
static FE_ATTR_130: collections::SvgAttr = collections::SvgAttr { attrs: &["pointsAtZ"], part: collections::SvgPart::Whole, initial: 0.0, slot: 130 };
static FE_ATTR_131: collections::SvgAttr = collections::SvgAttr { attrs: &["specularExponent"], part: collections::SvgPart::Whole, initial: 1.0, slot: 131 };
static FE_ATTR_132: collections::SvgAttr = collections::SvgAttr { attrs: &["limitingConeAngle"], part: collections::SvgPart::Whole, initial: 0.0, slot: 132 };
static FE_ATTR_133: collections::SvgAttr = collections::SvgAttr { attrs: &["scale"], part: collections::SvgPart::Whole, initial: 0.0, slot: 133 };
static FE_ATTR_134: collections::SvgAttr = collections::SvgAttr { attrs: &["dx"], part: collections::SvgPart::Whole, initial: 2.0, slot: 134 };
static FE_ATTR_135: collections::SvgAttr = collections::SvgAttr { attrs: &["dy"], part: collections::SvgPart::Whole, initial: 2.0, slot: 135 };
static FE_ATTR_136: collections::SvgAttr = collections::SvgAttr { attrs: &["stdDeviation"], part: collections::SvgPart::First, initial: 2.0, slot: 136 };
static FE_ATTR_137: collections::SvgAttr = collections::SvgAttr { attrs: &["stdDeviation"], part: collections::SvgPart::Second, initial: 2.0, slot: 137 };
static FE_ATTR_138: collections::SvgAttr = collections::SvgAttr { attrs: &["stdDeviation"], part: collections::SvgPart::First, initial: 0.0, slot: 138 };
static FE_ATTR_139: collections::SvgAttr = collections::SvgAttr { attrs: &["stdDeviation"], part: collections::SvgPart::Second, initial: 0.0, slot: 139 };
static FE_ATTR_140: collections::SvgAttr = collections::SvgAttr { attrs: &["crossorigin"], part: collections::SvgPart::Whole, initial: 0.0, slot: 140 };
static SVG_HREF: collections::SvgAttr = collections::SvgAttr { attrs: &["href", "xlink:href"], part: collections::SvgPart::Whole, initial: 0.0, slot: 141 };
static FE_ATTR_142: collections::SvgAttr = collections::SvgAttr { attrs: &["radius"], part: collections::SvgPart::First, initial: 0.0, slot: 142 };
static FE_ATTR_143: collections::SvgAttr = collections::SvgAttr { attrs: &["radius"], part: collections::SvgPart::Second, initial: 0.0, slot: 143 };
static FE_ATTR_144: collections::SvgAttr = collections::SvgAttr { attrs: &["dx"], part: collections::SvgPart::Whole, initial: 0.0, slot: 144 };
static FE_ATTR_145: collections::SvgAttr = collections::SvgAttr { attrs: &["dy"], part: collections::SvgPart::Whole, initial: 0.0, slot: 145 };
static FE_ATTR_146: collections::SvgAttr = collections::SvgAttr { attrs: &["specularConstant"], part: collections::SvgPart::Whole, initial: 1.0, slot: 146 };
static FE_ATTR_147: collections::SvgAttr = collections::SvgAttr { attrs: &["baseFrequency"], part: collections::SvgPart::First, initial: 0.0, slot: 147 };
static FE_ATTR_148: collections::SvgAttr = collections::SvgAttr { attrs: &["baseFrequency"], part: collections::SvgPart::Second, initial: 0.0, slot: 148 };
static FE_ATTR_149: collections::SvgAttr = collections::SvgAttr { attrs: &["numOctaves"], part: collections::SvgPart::Whole, initial: 1.0, slot: 149 };
static FE_ATTR_150: collections::SvgAttr = collections::SvgAttr { attrs: &["seed"], part: collections::SvgPart::Whole, initial: 0.0, slot: 150 };
simple_class!(SvgFeBlendElement, "SVGFEBlendElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_FEBLEND_MODE_UNKNOWN", 0.0);
    def.constant("SVG_FEBLEND_MODE_NORMAL", 1.0);
    def.constant("SVG_FEBLEND_MODE_MULTIPLY", 2.0);
    def.constant("SVG_FEBLEND_MODE_SCREEN", 3.0);
    def.constant("SVG_FEBLEND_MODE_DARKEN", 4.0);
    def.constant("SVG_FEBLEND_MODE_LIGHTEN", 5.0);
    def.constant("SVG_FEBLEND_MODE_OVERLAY", 6.0);
    def.constant("SVG_FEBLEND_MODE_COLOR_DODGE", 7.0);
    def.constant("SVG_FEBLEND_MODE_COLOR_BURN", 8.0);
    def.constant("SVG_FEBLEND_MODE_HARD_LIGHT", 9.0);
    def.constant("SVG_FEBLEND_MODE_SOFT_LIGHT", 10.0);
    def.constant("SVG_FEBLEND_MODE_DIFFERENCE", 11.0);
    def.constant("SVG_FEBLEND_MODE_EXCLUSION", 12.0);
    def.constant("SVG_FEBLEND_MODE_HUE", 13.0);
    def.constant("SVG_FEBLEND_MODE_SATURATION", 14.0);
    def.constant("SVG_FEBLEND_MODE_COLOR", 15.0);
    def.constant("SVG_FEBLEND_MODE_LUMINOSITY", 16.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("in2", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_101)?.value()));
    svg_enum!(def, "mode", FE_BLEND_MODE);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeColorMatrixElement, "SVGFEColorMatrixElement", SvgElement, |def| {
    def.getter("values", |s, n| collections::animated_number_list(s, n, "values"));
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_FECOLORMATRIX_TYPE_UNKNOWN", 0.0);
    def.constant("SVG_FECOLORMATRIX_TYPE_MATRIX", 1.0);
    def.constant("SVG_FECOLORMATRIX_TYPE_SATURATE", 2.0);
    def.constant("SVG_FECOLORMATRIX_TYPE_HUEROTATE", 3.0);
    def.constant("SVG_FECOLORMATRIX_TYPE_LUMINANCETOALPHA", 4.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    svg_enum!(def, "type", FE_COLOR_MATRIX_TYPE);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeComponentTransferElement, "SVGFEComponentTransferElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgComponentTransferFunctionElement, "SVGComponentTransferFunctionElement", SvgElement, |def| {
    def.getter("tableValues", |s, n| collections::animated_number_list(s, n, "tableValues"));
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_UNKNOWN", 0.0);
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_IDENTITY", 1.0);
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_TABLE", 2.0);
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_DISCRETE", 3.0);
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_LINEAR", 4.0);
    def.constant("SVG_FECOMPONENTTRANSFER_TYPE_GAMMA", 5.0);
    svg_enum!(def, "type", FE_TRANSFER_TYPE);
    def.getter("slope", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_103)?.value()));
    def.getter("intercept", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_104)?.value()));
    def.getter("amplitude", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_105)?.value()));
    def.getter("exponent", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_106)?.value()));
    def.getter("offset", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_107)?.value()));
});
simple_class!(SvgFeFuncRElement, "SVGFEFuncRElement", SvgComponentTransferFunctionElement);
simple_class!(SvgFeFuncGElement, "SVGFEFuncGElement", SvgComponentTransferFunctionElement);
simple_class!(SvgFeFuncBElement, "SVGFEFuncBElement", SvgComponentTransferFunctionElement);
simple_class!(SvgFeFuncAElement, "SVGFEFuncAElement", SvgComponentTransferFunctionElement);
simple_class!(SvgFeCompositeElement, "SVGFECompositeElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_FECOMPOSITE_OPERATOR_UNKNOWN", 0.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_OVER", 1.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_IN", 2.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_OUT", 3.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_ATOP", 4.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_XOR", 5.0);
    def.constant("SVG_FECOMPOSITE_OPERATOR_ARITHMETIC", 6.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("in2", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_101)?.value()));
    svg_enum!(def, "operator", FE_COMPOSITE_OPERATOR);
    def.getter("k1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_108)?.value()));
    def.getter("k2", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_109)?.value()));
    def.getter("k3", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_110)?.value()));
    def.getter("k4", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_111)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeConvolveMatrixElement, "SVGFEConvolveMatrixElement", SvgElement, |def| {
    def.getter("kernelMatrix", |s, n| collections::animated_number_list(s, n, "kernelMatrix"));
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_EDGEMODE_UNKNOWN", 0.0);
    def.constant("SVG_EDGEMODE_DUPLICATE", 1.0);
    def.constant("SVG_EDGEMODE_WRAP", 2.0);
    def.constant("SVG_EDGEMODE_NONE", 3.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("orderX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedInteger>(s, n.page(), n.id, &FE_ATTR_112)?.value()));
    def.getter("orderY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedInteger>(s, n.page(), n.id, &FE_ATTR_113)?.value()));
    def.getter("divisor", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_114)?.value()));
    def.getter("bias", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_115)?.value()));
    def.getter("targetX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedInteger>(s, n.page(), n.id, &FE_ATTR_116)?.value()));
    def.getter("targetY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedInteger>(s, n.page(), n.id, &FE_ATTR_117)?.value()));
    svg_enum!(def, "edgeMode", FE_CONVOLVE_EDGE_MODE);
    def.getter("kernelUnitLengthX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_118)?.value()));
    def.getter("kernelUnitLengthY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_119)?.value()));
    def.getter("preserveAlpha", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedBoolean>(s, n.page(), n.id, &FE_ATTR_120)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeDiffuseLightingElement, "SVGFEDiffuseLightingElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("surfaceScale", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_121)?.value()));
    def.getter("diffuseConstant", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_122)?.value()));
    def.getter("kernelUnitLengthX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_118)?.value()));
    def.getter("kernelUnitLengthY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_119)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeDistantLightElement, "SVGFEDistantLightElement", SvgElement, |def| {
    def.getter("azimuth", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_123)?.value()));
    def.getter("elevation", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_124)?.value()));
});
simple_class!(SvgFePointLightElement, "SVGFEPointLightElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_125)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_126)?.value()));
    def.getter("z", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_127)?.value()));
});
simple_class!(SvgFeSpotLightElement, "SVGFESpotLightElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_125)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_126)?.value()));
    def.getter("z", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_127)?.value()));
    def.getter("pointsAtX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_128)?.value()));
    def.getter("pointsAtY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_129)?.value()));
    def.getter("pointsAtZ", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_130)?.value()));
    def.getter("specularExponent", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_131)?.value()));
    def.getter("limitingConeAngle", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_132)?.value()));
});
simple_class!(SvgFeDisplacementMapElement, "SVGFEDisplacementMapElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_CHANNEL_UNKNOWN", 0.0);
    def.constant("SVG_CHANNEL_R", 1.0);
    def.constant("SVG_CHANNEL_G", 2.0);
    def.constant("SVG_CHANNEL_B", 3.0);
    def.constant("SVG_CHANNEL_A", 4.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("in2", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_101)?.value()));
    def.getter("scale", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_133)?.value()));
    svg_enum!(def, "xChannelSelector", FE_X_CHANNEL);
    svg_enum!(def, "yChannelSelector", FE_Y_CHANNEL);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeDropShadowElement, "SVGFEDropShadowElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("dx", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_134)?.value()));
    def.getter("dy", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_135)?.value()));
    def.getter("stdDeviationX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_136)?.value()));
    def.getter("stdDeviationY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_137)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
    def.method("setStdDeviation", 2, |s, n, a| {
        a.require(2, "setStdDeviation")?;
        let (x, y) = (a.get(0).to_number()? as f32, a.get(1).to_number()? as f32);
        if !x.is_finite() || !y.is_finite() {
            return Err(JsError::Type("Failed to execute 'setStdDeviation': The provided float value is non-finite.".into()));
        }
        n.page().dom.borrow_mut().set_attribute(n.id, "stdDeviation", &format!("{x} {y}")).js()?;
        Ok(s.undefined())
    });
});
simple_class!(SvgFeFloodElement, "SVGFEFloodElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeGaussianBlurElement, "SVGFEGaussianBlurElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_EDGEMODE_UNKNOWN", 0.0);
    def.constant("SVG_EDGEMODE_DUPLICATE", 1.0);
    def.constant("SVG_EDGEMODE_WRAP", 2.0);
    def.constant("SVG_EDGEMODE_NONE", 3.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("stdDeviationX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_138)?.value()));
    def.getter("stdDeviationY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_139)?.value()));
    svg_enum!(def, "edgeMode", FE_BLUR_EDGE_MODE);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
    def.method("setStdDeviation", 2, |s, n, a| {
        a.require(2, "setStdDeviation")?;
        let (x, y) = (a.get(0).to_number()? as f32, a.get(1).to_number()? as f32);
        if !x.is_finite() || !y.is_finite() {
            return Err(JsError::Type("Failed to execute 'setStdDeviation': The provided float value is non-finite.".into()));
        }
        n.page().dom.borrow_mut().set_attribute(n.id, "stdDeviation", &format!("{x} {y}")).js()?;
        Ok(s.undefined())
    });
});
simple_class!(SvgFeImageElement, "SVGFEImageElement", SvgElement, |def| {
    svg_aspect_ratio(def);
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("crossOrigin", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_140)?.value()));
    svg_href(def);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeMergeElement, "SVGFEMergeElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeMergeNodeElement, "SVGFEMergeNodeElement", SvgElement, |def| {
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
});
simple_class!(SvgFeMorphologyElement, "SVGFEMorphologyElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_MORPHOLOGY_OPERATOR_UNKNOWN", 0.0);
    def.constant("SVG_MORPHOLOGY_OPERATOR_ERODE", 1.0);
    def.constant("SVG_MORPHOLOGY_OPERATOR_DILATE", 2.0);
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    svg_enum!(def, "operator", FE_MORPHOLOGY_OPERATOR);
    def.getter("radiusX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_142)?.value()));
    def.getter("radiusY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_143)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeOffsetElement, "SVGFEOffsetElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("dx", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_144)?.value()));
    def.getter("dy", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_145)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeSpecularLightingElement, "SVGFESpecularLightingElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("surfaceScale", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_121)?.value()));
    def.getter("specularConstant", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_146)?.value()));
    def.getter("specularExponent", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_131)?.value()));
    def.getter("kernelUnitLengthX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_118)?.value()));
    def.getter("kernelUnitLengthY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_119)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeTileElement, "SVGFETileElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.getter("in1", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_100)?.value()));
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});
simple_class!(SvgFeTurbulenceElement, "SVGFETurbulenceElement", SvgElement, |def| {
    def.getter("x", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_300)?.value()));
    def.getter("y", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_303)?.value()));
    def.getter("width", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_306)?.value()));
    def.getter("height", |s, n| Ok(collections::animated_length(s, n.page(), n.id, &LEN_309)?.value()));
    def.constant("SVG_TURBULENCE_TYPE_UNKNOWN", 0.0);
    def.constant("SVG_TURBULENCE_TYPE_FRACTALNOISE", 1.0);
    def.constant("SVG_TURBULENCE_TYPE_TURBULENCE", 2.0);
    def.constant("SVG_STITCHTYPE_UNKNOWN", 0.0);
    def.constant("SVG_STITCHTYPE_STITCH", 1.0);
    def.constant("SVG_STITCHTYPE_NOSTITCH", 2.0);
    def.getter("baseFrequencyX", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_147)?.value()));
    def.getter("baseFrequencyY", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_148)?.value()));
    def.getter("numOctaves", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedInteger>(s, n.page(), n.id, &FE_ATTR_149)?.value()));
    def.getter("seed", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedNumber>(s, n.page(), n.id, &FE_ATTR_150)?.value()));
    svg_enum!(def, "stitchTiles", FE_STITCH_TILES);
    svg_enum!(def, "type", FE_TURBULENCE_TYPE);
    def.getter("result", |s, n| Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &FE_ATTR_102)?.value()));
});

// SVG 2 `a`: the link attributes it shares with HTML's.
simple_class!(SvgAElement, "SVGAElement", SvgGraphicsElement, |def| {
    svg_href(def);
    def.getter("target", |s, n| {
        Ok(collections::animated_attr::<collections::SvgAnimatedString>(s, n.page(), n.id, &LINK_TARGET)?.value())
    });
    reflect_str!(def, "rel", "rel");
    reflect_tokens!(def, "relList", "rel");
    reflect_str!(def, "download", "download");
    reflect_str!(def, "ping", "ping");
    reflect_str!(def, "hreflang", "hreflang");
    reflect_str!(def, "type", "type");
});

// ----- document helpers ---------------------------------------------------------------

pub(crate) fn shadow_mode(dom: &Dom, root: NodeId) -> &'static str {
    match dom.shadow_root_mode(root) {
        Ok(Some(pyro_dom::ShadowRootMode::Closed)) => "closed",
        _ => "open",
    }
}

pub(crate) fn ready_state(page: &PageDom, doc: NodeId) -> &'static str {
    page.ready_state(doc)
}

pub(crate) fn document_cookie(page: &PageDom, doc: NodeId) -> String {
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.to_string()).unwrap_or_default();
    let (Some(net), Ok(url)) = (page.net.borrow().clone(), pyro_net::Url::parse(&url)) else { return String::new() };
    let jar = net.cookies().clone();
    let mut jar = jar.lock().unwrap();
    jar.document_cookie(&url, pyro_net::cookies::now_secs())
}

pub(crate) fn set_document_cookie(page: &PageDom, doc: NodeId, value: &str) {
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.to_string()).unwrap_or_default();
    let (Some(net), Ok(url)) = (page.net.borrow().clone(), pyro_net::Url::parse(&url)) else { return };
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    let jar = net.cookies().clone();
    let mut jar = jar.lock().unwrap();
    jar.set_from_header(&url, value, pyro_net::cookies::Access::Script, pyro_net::cookies::now_secs());
}

pub(crate) fn create_event<'s>(s: &Scope<'s>, page: &Rc<PageDom>, iface: &str) -> JsResult<Value<'s>> {
    let i = match iface.to_ascii_lowercase().as_str() {
        "event" | "events" | "htmlevents" | "svgevents" => EventInterface::Event,
        "customevent" => EventInterface::CustomEvent,
        "uievent" | "uievents" => EventInterface::UiEvent,
        "focusevent" => EventInterface::FocusEvent,
        "mouseevent" | "mouseevents" => EventInterface::MouseEvent,
        "dragevent" => EventInterface::DragEvent,
        "keyboardevent" => EventInterface::KeyboardEvent,
        "messageevent" => EventInterface::MessageEvent,
        "hashchangeevent" => EventInterface::HashChangeEvent,
        "beforeunloadevent" => EventInterface::BeforeUnloadEvent,
        "compositionevent" => EventInterface::CompositionEvent,
        "devicemotionevent" => EventInterface::DeviceMotionEvent,
        "deviceorientationevent" => EventInterface::DeviceOrientationEvent,
        "storageevent" => EventInterface::StorageEvent,
        "textevent" => EventInterface::TextEvent,
        "errorevent" => EventInterface::ErrorEvent,
        "popstateevent" => EventInterface::PopStateEvent,
        _ => return Err(JsError::dom("NotSupportedError", format!("The provided event type ('{iface}') is invalid."))),
    };
    let core = EventCore::new(page, i, "", false, false, false);
    core.state.borrow_mut().initialized = false;
    Ok(crate::event::wrap_event(s, &core)?.value())
}

pub(crate) fn create_range<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    crate::range::new_range(s, n.page(), n.id)
}

pub(crate) fn element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    crate::layout_api::element_from_point(s, n, a)
}

pub(crate) fn elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    crate::layout_api::elements_from_point(s, n, a)
}

pub(crate) fn upgrade_if_custom<'s>(s: &Scope<'s>, page: &Rc<PageDom>, doc: NodeId, el: NodeId) -> JsResult<Value<'s>> {
    crate::custom::create_element_value(s, page, doc, el)
}

// ----- document.open / write / close ----------------------------------------
//
// Dynamic markup insertion on a loaded document (what Playwright's
// page.setContent does): open() empties the document and starts a
// script-created parser, write() parses into it synchronously, running inline
// classic scripts in the main world as they are reached (their own writes are
// parsed at the insertion point), and close() ends it;
// the embedder then reports DOMContentLoaded/load.
//
// While the network parser loads the document, a script it reached has that
// parser on loan (`PageDom::lend_network_parser`), so its writes parse at the
// insertion point too. External and module scripts in written markup go to the
// embedder's loader; a parser-blocking one stops tokenizing (later writes only
// queue input) until it was fetched and ran. External and module scripts have
// no insertion point otherwise, so their writes are ignored. A write or open
// outside any script while the network parser runs, which aborts the load in a
// browser, is reported explicitly as unsupported.

fn html_document(page: &PageDom, doc: NodeId) -> JsResult<()> {
    let html = page.dom.borrow().document_data(doc).map(|d| d.kind == pyro_dom::DocumentKind::Html).js()?;
    if !html {
        return Err(JsError::dom("InvalidStateError", "Only HTML documents support dynamic markup insertion."));
    }
    Ok(())
}

fn network_parser_active(page: &PageDom, doc: NodeId) -> bool {
    // An about:blank document (a new iframe's initial one, still "loading"
    // until its load task) has no network parser: it can be opened at once,
    // as in Chrome.
    !page.script_parser_active.get()
        && page.ready_state(doc) != "complete"
        && !page.dom.borrow().document_data(doc).is_ok_and(|d| &*d.url == "about:blank")
}

/// A script the network parser reached is running and `doc` is that parser's.
fn writes_to_network_parser(page: &PageDom, doc: NodeId) -> bool {
    page.network_parser_lent.get() && doc == page.document()
}

/// A written parser-blocking script of `doc` is pending: input only queues.
fn writes_blocked(page: &PageDom, doc: NodeId) -> bool {
    doc == page.document() && page.written_pending.get().is_some()
}

/// `document.open()` (the two-argument form; three arguments open a window).
pub(crate) fn document_open<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = n.page();
    if a.len() >= 3 {
        let open = s.global().get_atom("open")?;
        let open = open.as_object().ok_or_else(|| JsError::Type("window.open is not a function".into()))?;
        return open.call(None, &[a.get(0), a.get(1), a.get(2)]);
    }
    html_document(page, n.id)?;
    crate::custom::check_dynamic_markup(page, n.id)?;
    let document = node_value(s, page, Some(n.id))?;
    // From a script the active parser is running: no-op.
    if (page.script_parser_active.get() || writes_to_network_parser(page, n.id)) && page.script_nesting.get() > 0 {
        return Ok(document);
    }
    open_document(page, n.id)?;
    Ok(document)
}

fn open_document(page: &Rc<PageDom>, doc: NodeId) -> JsResult<()> {
    if network_parser_active(page, doc) {
        return Err(JsError::dom(
            "NotSupportedError",
            "document.open/write outside a parser-run script while the document is still loading is not supported by pyro",
        ));
    }
    if let Some(mut old) = page.script_parser.borrow_mut().take() {
        old.end();
    }
    page.written_scripts.borrow_mut().clear();
    page.written_pending.set(None);
    // Erase all event listeners and handlers of the document's nodes and window.
    let mut keys: Vec<TargetKey> = {
        let dom = page.dom.borrow();
        dom.descendants(doc).js()?.filter_map(|id| dom.node_key(id)).collect()
    };
    // An inactive document has no associated Window. Its creator's
    // listeners belong to the displayed document.
    if doc == page.document() {
        keys.extend(page.window_target());
    }
    {
        let mut dom = page.dom.borrow_mut();
        for key in keys {
            dom.erase_listeners(key);
        }
        dom.replace_all(doc, None).js()?;
    }
    page.set_ready_state(doc, "loading");
    let parser = pyro_dom::parse::DocumentParser::new(page.dom.clone(), doc, !page.scripting_disabled()).js()?;
    *page.script_parser.borrow_mut() = Some(parser);
    page.script_parser_active.set(true);
    page.script_parser_close.set(false);
    // The frame's document only: one made with createHTMLDocument (or
    // DOMParser) is no frame's, which neither loads again nor stops being
    // the initial about:blank document.
    if let Some(hooks) = page.hooks().filter(|_| doc == page.document()) {
        hooks.document_reopened();
    }
    Ok(())
}

/// `document.write`/`writeln`.
pub(crate) fn document_write<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, ln: bool) -> JsResult<Value<'s>> {
    let page = n.page();
    html_document(page, n.id)?;
    crate::custom::check_dynamic_markup(page, n.id)?;
    let mut text = String::new();
    for i in 0..a.len() {
        text.push_str(&dom_string(a.get(i))?);
    }
    if ln {
        text.push('\n');
    }
    if !page.script_parser_active.get() && !writes_to_network_parser(page, n.id) {
        // No insertion point: ignored from an external or module script, else
        // an implicit open on a loaded document.
        if page.ignore_destructive_writes.get() > 0 {
            return Ok(s.undefined());
        }
        open_document(page, n.id)?;
    }
    let blocked = writes_blocked(page, n.id);
    let Some(step) = with_parser(page, n.id, |p| {
        if blocked {
            p.push_input(&text);
            pyro_dom::parse::ParseStep::NeedInput
        } else {
            p.feed(&text)
        }
    }) else {
        return Ok(s.undefined());
    };
    let result = drive_parser(page, n.id, step);
    // From a script the parser is running, its deferred close waits until the
    // outer write returns.
    if page.script_nesting.get() == 0 && !writes_blocked(page, n.id) && page.script_parser_close.take() {
        close_document(page, n.id);
    }
    result.map(|_| s.undefined())
}

/// `document.close()`.
pub(crate) fn document_close<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    html_document(page, n.id)?;
    crate::custom::check_dynamic_markup(page, n.id)?;
    if !page.script_parser_active.get() {
        return Ok(s.undefined());
    }
    if page.script_nesting.get() > 0 || writes_blocked(page, n.id) {
        // From a script the parser is running, or with a written script still
        // to come: close once the outer write returns or the script ran.
        page.script_parser_close.set(true);
        return Ok(s.undefined());
    }
    close_document(page, n.id);
    Ok(s.undefined())
}

fn close_document(page: &Rc<PageDom>, doc: NodeId) {
    if let Some(step) = with_parser(page, doc, |p| p.resume()) {
        let _ = drive_parser(page, doc, step);
    }
    // A blocking script at the very end of the input: close once it ran.
    if writes_blocked(page, doc) {
        page.script_parser_close.set(true);
        return;
    }
    let Some(mut parser) = page.script_parser.borrow_mut().take() else { return };
    parser.end();
    page.script_parser_active.set(false);
    crate::lifecycle::set_ready_state(page, doc, "interactive");
    // The frame's document only, as for `document_reopened`.
    if !page.hooks().filter(|_| doc == page.document()).is_some_and(|h| h.document_closed()) {
        let p = page.clone();
        page.post_task(Box::new(move || {
            if p.is_disposed() || p.document() != doc {
                return;
            }
            crate::lifecycle::dom_content_loaded(&p, doc);
            crate::lifecycle::load(&p, doc);
        }));
    }
}

/// Runs `f` on the parser writes to `doc` go to. The parser is borrowed only
/// for the tokenizer call, never while a script runs, so scripts can write to it.
fn with_parser<R>(page: &PageDom, doc: NodeId, f: impl FnOnce(&mut pyro_dom::parse::DocumentParser) -> R) -> Option<R> {
    let slot = if writes_to_network_parser(page, doc) { &page.network_parser } else { &page.script_parser };
    let mut parser = slot.borrow_mut().take()?;
    let result = f(&mut parser);
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(parser);
    }
    Some(result)
}

/// Runs the parser until it needs input or a written script blocks it,
/// executing the inline scripts it yields. The first script error is returned
/// after parsing continued past it.
fn drive_parser(page: &Rc<PageDom>, doc: NodeId, mut step: pyro_dom::parse::ParseStep) -> JsResult<()> {
    let mut first_error = None;
    while let pyro_dom::parse::ParseStep::Script(node) = step {
        if let Err(err) = run_parser_script(page, doc, node) {
            first_error.get_or_insert(err);
        }
        if writes_blocked(page, doc) {
            break;
        }
        match with_parser(page, doc, |p| p.resume()) {
            Some(next) => step = next,
            None => break,
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn run_parser_script(page: &Rc<PageDom>, doc: NodeId, node: NodeId) -> JsResult<()> {
    use pyro_dom::script::ScriptKind;
    if page.scripting_disabled() {
        return Ok(());
    }
    let info = {
        let dom = page.dom.borrow();
        let Ok(info) = dom.script_info(node) else { return Ok(()) };
        if info.already_started {
            return Ok(());
        }
        info
    };
    match info.kind {
        ScriptKind::Classic if !info.nomodule && info.src.is_none() => {}
        ScriptKind::Classic if !info.nomodule => {
            return queue_written_script(page, doc, node, !info.async_ && !info.defer, "external")
        }
        ScriptKind::Module => return queue_written_script(page, doc, node, false, "module"),
        // nomodule classic scripts, import maps and data blocks do not run.
        _ => {
            let _ = page.dom.borrow_mut().mark_script_started(node);
            return Ok(());
        }
    }
    let _ = page.dom.borrow_mut().mark_script_started(node);
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.clone()).unwrap_or_default();
    eval_parser_script(page, doc, node, &info.inline_text, &url)
}

/// Hands a script in written markup to the loader, which marks it started. A
/// parser-blocking one stops tokenizing until it ran.
fn queue_written_script(page: &PageDom, doc: NodeId, node: NodeId, blocking: bool, what: &str) -> JsResult<()> {
    let accepted = doc == page.document()
        && (page.network_parser_lent.get() || page.hooks().is_some_and(|h| h.written_script_queued()));
    if !accepted {
        let _ = page.dom.borrow_mut().mark_script_started(node);
        return Err(JsError::dom(
            "NotSupportedError",
            format!("{what} scripts in document.write markup are not supported by pyro"),
        ));
    }
    page.written_scripts.borrow_mut().push(node);
    if blocking {
        page.written_pending.set(Some(node));
    }
    Ok(())
}

/// Evaluates a script of the parser at its insertion point.
fn eval_parser_script(page: &Rc<PageDom>, doc: NodeId, node: NodeId, text: &str, url: &str) -> JsResult<()> {
    let Some(realm) = page.main_world_realm() else { return Ok(()) };
    let scope = realm.scope()?;
    let outer_script = page.current_script();
    page.set_executing_classic_script(node);
    page.script_nesting.set(page.script_nesting.get() + 1);
    with_parser(page, doc, |p| p.open_insertion_point());
    crate::trusted_types::sync_compile_strings(&scope, page);
    if let Err(e) = scope.eval(text, Some(url), 1) {
        scope.report_exception(&e);
    }
    with_parser(page, doc, |p| p.close_insertion_point());
    page.script_nesting.set(page.script_nesting.get() - 1);
    page.set_current_script(outer_script);
    Ok(())
}

/// The loader fetched the parser-blocking script `node` written to the
/// script-created parser: runs it, then parses on.
pub(crate) fn written_script_fetched(page: &Rc<PageDom>, node: NodeId, url: &str, source: Result<String, String>) {
    // A later document.open() dropped the parser this script belonged to.
    if page.is_disposed() || page.written_pending.get() != Some(node) {
        return;
    }
    page.written_pending.set(None);
    let doc = page.document();
    match source {
        Ok(text) => {
            let _ = eval_parser_script(page, doc, node, &text, url);
        }
        Err(error) => {
            if let Some(realm) = page.main_world_realm() {
                if let Ok(scope) = realm.scope() {
                    crate::lifecycle::report_diagnostic(
                        &scope,
                        &JsError::Type(format!("Failed to load script: {error}").into()),
                    );
                }
            }
        }
    }
    if !writes_blocked(page, doc) {
        if let Some(step) = with_parser(page, doc, |p| p.resume()) {
            let _ = drive_parser(page, doc, step);
        }
    }
    if !writes_blocked(page, doc) && page.script_parser_close.take() {
        close_document(page, doc);
    }
}

pub(crate) fn dom_implementation<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Object<'s>> {
    let key = IdentityKey(n.page().id() | (1 << 60), n.id.index() as u64);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let hold = crate::collections::NodeHold::new(n.page(), n.id)?;
    s.wrap::<DomImplementation>(hold, Some(key))
}

pub struct DomImplementation;
impl JsClass for DomImplementation {
    const NAME: &'static str = "DOMImplementation";
    type Data = crate::collections::NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("createHTMLDocument", 0, |s, h, a| {
            let title = if a.get(0).is_undefined() { None } else { Some(dom_string(a.get(0))?) };
            let mut dom = h.page.dom.borrow_mut();
            let doc = dom.create_document(pyro_dom::DocumentKind::Html, "about:blank");
            let dt = dom.create_doctype(doc, "html", "", "").js()?;
            dom.append_child(doc, dt).js()?;
            let html = dom.create_element(doc, "html").js()?;
            dom.append_child(doc, html).js()?;
            let head = dom.create_element(doc, "head").js()?;
            dom.append_child(html, head).js()?;
            if let Some(title) = title {
                let t = dom.create_element(doc, "title").js()?;
                let text = dom.create_text(doc, &title).js()?;
                dom.append_child(t, text).js()?;
                dom.append_child(head, t).js()?;
            }
            let body = dom.create_element(doc, "body").js()?;
            dom.append_child(html, body).js()?;
            drop(dom);
            Ok(wrap_node(s, &h.page, doc)?.value())
        });
        def.method("createDocument", 2, |s, h, a| {
            a.require(2, "DOMImplementation.createDocument")?;
            let ns = opt_dom_string(a.get(0))?;
            let q = null_to_empty(a.get(1))?;
            let doctype =
                if a.get(2).is_nullish() { None } else { Some(crate::node::node_arg(s, a.get(2), "createDocument")?) };
            let mut dom = h.page.dom.borrow_mut();
            let doc = dom.create_document(pyro_dom::DocumentKind::Xml, "about:blank");
            let element = if q.is_empty() { None } else { Some(dom.create_element_ns(doc, ns.as_deref(), &q).js()?) };
            if let Some(dt) = doctype {
                dom.append_child(doc, dt).js()?;
            }
            if let Some(el) = element {
                dom.append_child(doc, el).js()?;
            }
            let content_type = match ns.as_deref() {
                Some("http://www.w3.org/1999/xhtml") => "application/xhtml+xml",
                Some("http://www.w3.org/2000/svg") => "image/svg+xml",
                _ => "application/xml",
            };
            let _ = dom.set_document_kind(doc, pyro_dom::DocumentKind::Xml, content_type, "UTF-8");
            drop(dom);
            Ok(wrap_node(s, &h.page, doc)?.value())
        });
        def.method("createDocumentType", 3, |s, h, a| {
            a.require(3, "DOMImplementation.createDocumentType")?;
            let (name, public, system) = (dom_string(a.get(0))?, dom_string(a.get(1))?, dom_string(a.get(2))?);
            pyro_dom::dom::validate_doctype_name(&name).js()?;
            let mut dom = h.page.dom.borrow_mut();
            let dt = dom.create_doctype(h.id, &name, &public, &system).js()?;
            drop(dom);
            Ok(wrap_node(s, &h.page, dt)?.value())
        });
        def.pure_method("hasFeature", 0, |s, _h, _a| Ok(s.boolean(true)));
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<HtmlElement>();
    engine.register::<HtmlUnknownElement>();
    engine.register::<HtmlHtmlElement>();
    engine.register::<HtmlHeadElement>();
    engine.register::<HtmlBodyElement>();
    engine.register::<HtmlTitleElement>();
    engine.register::<HtmlMetaElement>();
    engine.register::<HtmlLinkElement>();
    engine.register::<HtmlStyleElement>();
    engine.register::<HtmlScriptElement>();
    engine.register::<HtmlDivElement>();
    engine.register::<HtmlSpanElement>();
    engine.register::<HtmlParagraphElement>();
    engine.register::<HtmlHeadingElement>();
    engine.register::<HtmlAnchorElement>();
    engine.register::<HtmlAreaElement>();
    engine.register::<HtmlImageElement>();
    engine.register::<HtmlInputElement>();
    engine.register::<HtmlTextAreaElement>();
    engine.register::<HtmlSelectElement>();
    engine.register::<HtmlOptionElement>();
    engine.register::<HtmlOptGroupElement>();
    engine.register::<HtmlButtonElement>();
    engine.register::<HtmlFormElement>();
    engine.register::<HtmlLabelElement>();
    engine.register::<HtmlFieldSetElement>();
    engine.register::<HtmlLegendElement>();
    engine.register::<HtmlUListElement>();
    engine.register::<HtmlOListElement>();
    engine.register::<HtmlLiElement>();
    engine.register::<HtmlTableElement>();
    engine.register::<HtmlTableSectionElement>();
    engine.register::<HtmlTableRowElement>();
    engine.register::<HtmlTableCellElement>();
    engine.register::<HtmlTableCaptionElement>();
    engine.register::<HtmlTableColElement>();
    engine.register::<HtmlIFrameElement>();
    engine.register::<HtmlTemplateElement>();
    engine.register::<HtmlSlotElement>();
    engine.register::<HtmlCanvasElement>();
    engine.register::<HtmlMediaElement>();
    engine.register::<HtmlVideoElement>();
    engine.register::<HtmlAudioElement>();
    engine.register::<HtmlBrElement>();
    engine.register::<HtmlHrElement>();
    engine.register::<HtmlPreElement>();
    engine.register::<HtmlQuoteElement>();
    engine.register::<HtmlDListElement>();
    engine.register::<HtmlDialogElement>();
    engine.register::<HtmlDetailsElement>();
    engine.register::<HtmlProgressElement>();
    engine.register::<HtmlMeterElement>();
    engine.register::<HtmlOutputElement>();
    engine.register::<HtmlDataListElement>();
    engine.register::<HtmlPictureElement>();
    engine.register::<HtmlSourceElement>();
    engine.register::<HtmlTimeElement>();
    engine.register::<HtmlDataElement>();
    engine.register::<HtmlBaseElement>();
    engine.register::<HtmlObjectElement>();
    engine.register::<HtmlEmbedElement>();
    engine.register::<HtmlMapElement>();
    engine.register::<HtmlModElement>();
    engine.register::<HtmlMenuElement>();
    engine.register::<HtmlMarqueeElement>();
    engine.register::<HtmlFrameSetElement>();
    engine.register::<HtmlFrameElement>();
    engine.register::<HtmlFontElement>();
    engine.register::<HtmlDirectoryElement>();
    engine.register::<HtmlParamElement>();
    engine.register::<HtmlTrackElement>();
    engine.register::<SvgElement>();
    engine.register::<MathMlElement>();
    engine.register::<SvgGraphicsElement>();
    engine.register::<SvgSvgElement>();
    engine.register::<SvgSymbolElement>();
    engine.register::<SvgViewElement>();
    engine.register::<SvgAElement>();
    engine.register::<SvgClipPathElement>();
    engine.register::<SvgMaskElement>();
    engine.register::<SvgPatternElement>();
    engine.register::<SvgGradientElement>();
    engine.register::<SvgLinearGradientElement>();
    engine.register::<SvgRadialGradientElement>();
    engine.register::<SvgFilterElement>();
    engine.register::<SvgUnitTypes>();
    engine.register::<SvgMarkerElement>();
    engine.register::<SvgGeometryElement>();
    engine.register::<SvgImageElement>();
    engine.register::<SvgForeignObjectElement>();
    engine.register::<SvgUseElement>();
    engine.register::<SvgTextContentElement>();
    engine.register::<SvgTextPositioningElement>();
    engine.register::<SvgTextElement>();
    engine.register::<SvgTSpanElement>();
    engine.register::<SvgTextPathElement>();
    engine.register::<SvgStopElement>();
    engine.register::<SvgPathElement>();
    engine.register::<SvgPolylineElement>();
    engine.register::<SvgPolygonElement>();
    engine.register::<SvgRectElement>();
    engine.register::<SvgCircleElement>();
    engine.register::<SvgEllipseElement>();
    engine.register::<SvgLineElement>();
    engine.register::<SvgComponentTransferFunctionElement>();
    engine.register::<SvgFeBlendElement>();
    engine.register::<SvgFeColorMatrixElement>();
    engine.register::<SvgFeComponentTransferElement>();
    engine.register::<SvgFeFuncRElement>();
    engine.register::<SvgFeFuncGElement>();
    engine.register::<SvgFeFuncBElement>();
    engine.register::<SvgFeFuncAElement>();
    engine.register::<SvgFeCompositeElement>();
    engine.register::<SvgFeConvolveMatrixElement>();
    engine.register::<SvgFeDiffuseLightingElement>();
    engine.register::<SvgFeDistantLightElement>();
    engine.register::<SvgFePointLightElement>();
    engine.register::<SvgFeSpotLightElement>();
    engine.register::<SvgFeDisplacementMapElement>();
    engine.register::<SvgFeDropShadowElement>();
    engine.register::<SvgFeFloodElement>();
    engine.register::<SvgFeGaussianBlurElement>();
    engine.register::<SvgFeImageElement>();
    engine.register::<SvgFeMergeElement>();
    engine.register::<SvgFeMergeNodeElement>();
    engine.register::<SvgFeMorphologyElement>();
    engine.register::<SvgFeOffsetElement>();
    engine.register::<SvgFeSpecularLightingElement>();
    engine.register::<SvgFeTileElement>();
    engine.register::<SvgFeTurbulenceElement>();
    engine.register::<DomImplementation>();
}

// Option getters can call back into JS; convert before focus/DOM borrows.
fn focus_element<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let value = a.get(0);
    let prevent_scroll = if value.is_null() || value.is_undefined() {
        false
    } else {
        value
            .as_object()
            .ok_or_else(|| JsError::Type("Focus options must be an object".into()))?
            .get_atom("preventScroll")?
            .to_bool()
    };
    crate::input::focus_element_with_options(n.page(), n.id, prevent_scroll);
    Ok(s.undefined())
}

#[cfg(test)]
mod tests {
    use super::{resolve_against, resolve_simple};

    /// `resolve_simple` answers only where the URL parser returns the same
    /// string, over bases and values around each of its conditions.
    #[test]
    fn simple_paths_resolve_as_the_parser_does() {
        let bases = [
            "http://127.0.0.1:5173/",
            "http://127.0.0.1:5173/app/page?q=1#f",
            "https://example.com/a/b/c",
            "https://user:pw@example.com:8443/x/",
            "http://EXAMPLE.com/",
            "http://example.com:80/",
            "file:///tmp/x.html",
            "data:text/html,x",
            "about:blank",
            "blob:http://example.com/uuid",
        ];
        let values = [
            "/assets/index-B8Zr9I8L.js",
            "/assets/__root-BOYMu7E6.js",
            "/",
            "/a//b",
            "//cdn.example.com/x.js",
            "/./x",
            "/a/../x",
            "/a/..",
            "/a/.",
            "/.hidden/..x/x..",
            "/a/b%20c",
            "/a b",
            "/a?b",
            "/a#b",
            "/a\\b",
            "/a\tb",
            "/é",
            "/a:b@c;d=e,f+g*h(i)j'k&l$m!n~o_p.q-r",
            "/a^b",
            "/a|b",
            "/a{b}",
            "/a`b",
            "/a\"b",
            "/a<b>",
            " /a",
            "/a ",
            "x.js",
            "./x.js",
            "../x.js",
            "",
            "http://127.0.0.1:5173/assets/x.js",
            "http://127.0.0.1:5173//x.js",
            "http://127.0.0.1:5173/./x.js",
            "http://127.0.0.1:51730/x.js",
            "http://127.0.0.1:5173",
            "http://127.0.0.1:5173?x",
            "https://example.com/a/b",
            "https://example.com:443/a",
            "https://user:pw@example.com:8443/y",
            "http://example.com/",
            "http://example.com:80/x",
            "HTTP://example.com/x",
        ];
        let mut answered = 0;
        for b in bases {
            let base = pyro_net::Url::parse(b).unwrap();
            for v in values {
                let parser = resolve_against(Some(&base), v, encoding_rs::UTF_8);
                if let Some(simple) = resolve_simple(&base, v) {
                    assert_eq!(simple, parser, "base {b:?}, value {v:?}");
                    answered += 1;
                }
            }
        }
        assert!(answered >= 20, "{answered} values answered without the parser");
        let base = pyro_net::Url::parse("http://127.0.0.1:5173/").unwrap();
        assert_eq!(resolve_simple(&base, "/assets/x.js").as_deref(), Some("http://127.0.0.1:5173/assets/x.js"));
        assert_eq!(
            resolve_simple(&base, "http://127.0.0.1:5173/assets/x.js").as_deref(),
            Some("http://127.0.0.1:5173/assets/x.js")
        );
    }
}

#[cfg(test)]
mod canvas_dimension_tests {
    use super::canvas_dimension;

    #[test]
    fn canvas_dimensions_follow_the_rules_for_non_negative_integers_and_the_reflected_range() {
        assert_eq!(canvas_dimension(Some("100px"), 300), 100);
        assert_eq!(canvas_dimension(Some(" 40"), 150), 40);
        assert_eq!(canvas_dimension(Some("-0"), 300), 0);
        assert_eq!(canvas_dimension(Some("-5"), 300), 300);
        assert_eq!(canvas_dimension(Some("px"), 300), 300);
        assert_eq!(canvas_dimension(Some("2147483647"), 300), 2147483647);
        assert_eq!(canvas_dimension(Some("4294967296"), 300), 300);
        assert_eq!(canvas_dimension(None, 150), 150);
    }
}
