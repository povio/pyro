//! HTML element interfaces, attribute reflection and document helpers.

use std::rc::Rc;

use lp_dom::events::TargetKey;
use lp_dom::forms::InputType;
use lp_dom::{Dom, NodeId};
use lp_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

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
                crate::input::blur_if_focus_disabled(n.page(), $attr);
                Ok(())
            },
        );
    };
}

macro_rules! reflect_url {
    ($def:expr, $prop:literal, $attr:literal) => {
        $def.accessor(
            $prop,
            |s, n| {
                let dom = n.page().dom.borrow();
                let Some(v) = dom.get_attribute(n.id, $attr).js()? else { return Ok(s.string("")) };
                Ok(s.string(&resolve_reflected_url(n.page(), &dom, n.id, v)))
            },
            |_s, n, v| {
                let arg0 = dom_string(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, $attr, &arg0).js()
            },
        );
    };
}

macro_rules! reflect_long {
    ($def:expr, $prop:literal, $attr:literal, $default:expr) => {
        $def.accessor(
            $prop,
            |s, n| {
                let dom = n.page().dom.borrow();
                let v = dom.get_attribute(n.id, $attr).js()?.and_then(|v| v.trim().parse::<i32>().ok());
                Ok(s.number(v.unwrap_or($default) as f64))
            },
            |_s, n, v| {
                let arg0 = to_i32(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, $attr, &arg0.to_string()).js()
            },
        );
    };
}

macro_rules! simple_class {
    ($name:ident, $js:literal, $parent:ty) => {
        pub struct $name;
        impl JsClass for $name {
            const NAME: &'static str = $js;
            type Data = NodeData;
            type Parent = $parent;
            fn define(_def: &mut ClassDef<Self>) {}
        }
    };
    ($name:ident, $js:literal, $parent:ty, $body:expr) => {
        pub struct $name;
        impl JsClass for $name {
            const NAME: &'static str = $js;
            type Data = NodeData;
            type Parent = $parent;
            fn define(def: &mut ClassDef<Self>) {
                let f: fn(&mut ClassDef<Self>) = $body;
                f(def);
            }
        }
    };
}

/// Resolves a URL attribute against the node document's inherited base or URL.
pub fn resolve_url(dom: &Dom, node: NodeId, value: &str) -> String {
    let base = dom.node_document(node).ok().and_then(|d| dom.document_base_url(d).ok());
    let parsed = base.and_then(|b| lp_net::Url::parse(b).ok());
    resolve_against(parsed.as_ref(), value)
}

fn resolve_against(base: Option<&lp_net::Url>, value: &str) -> String {
    match base {
        Some(b) => b.join(value.trim()).map(|u| u.to_string()).unwrap_or_else(|_| value.to_owned()),
        None => value.to_owned(),
    }
}

/// One bounded entry, with no node or JavaScript references. The actual base
/// string is the key: document URL/base setters do not increment DOM version,
/// and a retained wrapper can be adopted into a different owner document.
pub(crate) struct ReflectedUrlBase {
    source: String,
    parsed: Option<lp_net::Url>,
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
        let parsed = lp_net::Url::parse(base).ok();
        // Canonicalization can expand non-ASCII input. Oversized bases remain
        // fully supported, but neither source nor parsed strings are retained.
        if parsed.as_ref().is_some_and(|url| url.as_str().len() > MAX_BASE_BYTES) {
            return resolve_against(parsed.as_ref(), value);
        }
        *cache = Some(ReflectedUrlBase { source: base.to_owned(), parsed });
    }
    resolve_against(cache.as_ref().and_then(|entry| entry.parsed.as_ref()), value)
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
    Unknown,
    Generic,
    Svg,
    SvgGraphics,
    SvgRoot,
    Other,
}

pub fn class_for(dom: &Dom, id: NodeId) -> ElClass {
    let Ok(e) = dom.element(id) else { return ElClass::Other };
    if e.namespace() == &markup5ever::ns!(svg) {
        return match &*e.name.local {
            "svg" => ElClass::SvgRoot,
            // SVGGraphicsElement interfaces (SVG 2).
            "g" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" | "text" | "tspan"
            | "textPath" | "use" | "image" | "foreignObject" | "a" | "switch" | "defs" => ElClass::SvgGraphics,
            _ => ElClass::Svg,
        };
    }
    if !e.is_html() {
        return ElClass::Other;
    }
    use ElClass::*;
    match &*e.name.local {
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
        "applet" | "bgsound" | "blink" | "isindex" | "keygen" | "multicol" | "nextid" | "spacer" => Unknown,
        "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code" | "dd" | "dfn" | "dt"
        | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i" | "kbd" | "main" | "mark" | "nav"
        | "noscript" | "rp" | "rt" | "ruby" | "s" | "samp" | "search" | "section" | "small" | "strong" | "sub"
        | "summary" | "sup" | "u" | "var" | "wbr" | "center" | "nobr" | "noembed" | "noframes" | "plaintext"
        | "big" | "strike" | "tt" | "acronym" | "basefont" | "marquee" | "frameset" | "frame" | "font" | "dir"
        | "param" | "track" => Generic,
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
        Unknown => s.wrap::<HtmlUnknownElement>(d, k),
        Generic => s.wrap::<HtmlElement>(d, k),
        Svg => s.wrap::<SvgElement>(d, k),
        SvgGraphics => s.wrap::<SvgGraphicsElement>(d, k),
        SvgRoot => s.wrap::<SvgSvgElement>(d, k),
        Other => s.wrap::<Element>(d, k),
    }
}

// ----- Element extras (layout-dependent members) --------------------------------

pub(crate) fn define_element_extras(def: &mut ClassDef<Element>) {
    def.method("getBoundingClientRect", 0, |s, n, _a| crate::layout_api::bounding_rect(s, n));
    def.method("getClientRects", 0, |s, n, _a| crate::layout_api::client_rects(s, n));
    def.method("scrollIntoView", 0, |s, n, _a| crate::layout_api::scroll_into_view(s, n));
    def.method("scrollIntoViewIfNeeded", 0, |s, n, _a| crate::layout_api::scroll_into_view(s, n));
    def.method("checkVisibility", 0, |s, n, _a| crate::layout_api::check_visibility(s, n));
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
    def.getter("clientTop", |s, _n| Ok(s.number(0.0)));
    def.getter("clientLeft", |s, _n| Ok(s.number(0.0)));
    def.method("setPointerCapture", 1, |s, _n, _a| Ok(s.undefined()));
    def.method("releasePointerCapture", 1, |s, _n, _a| Ok(s.undefined()));
    def.method("hasPointerCapture", 1, |s, _n, _a| Ok(s.boolean(false)));
    def.method("animate", 0, |s, _n, _a| {
        s.eval("({finished: Promise.resolve(), cancel(){}, finish(){}, play(){}, pause(){}, addEventListener(){}, removeEventListener(){}, onfinish: null, playState: 'finished'})", None, 1)
    });
    def.method("getAnimations", 0, |s, _n, _a| Ok(s.array(&[])?.value()));
    // ARIA reflection (ARIAMixin) for the attributes Playwright/Testing
    // Library read through properties.
    reflect_aria(def);
}

fn reflect_aria(def: &mut ClassDef<Element>) {
    macro_rules! aria {
        ($prop:literal, $attr:literal) => {
            def.accessor(
                $prop,
                |s, n| Ok(n.page().dom.borrow().get_attribute(n.id, $attr).js()?.map_or(s.null(), |v| s.string(v))),
                |_s, n, v| {
                    if v.is_null() {
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
    aria!("ariaLabel", "aria-label");
    aria!("ariaHidden", "aria-hidden");
    aria!("ariaExpanded", "aria-expanded");
    aria!("ariaChecked", "aria-checked");
    aria!("ariaSelected", "aria-selected");
    aria!("ariaDisabled", "aria-disabled");
    aria!("ariaPressed", "aria-pressed");
    aria!("ariaLevel", "aria-level");
    aria!("ariaValueNow", "aria-valuenow");
    aria!("ariaDescription", "aria-description");
}

// ----- HTMLElement ------------------------------------------------------------

pub struct HtmlElement;
impl JsClass for HtmlElement {
    const NAME: &'static str = "HTMLElement";
    type Data = NodeData;
    type Parent = Element;
    fn define(def: &mut ClassDef<Self>) {
        crate::handlers::define_global(def);
        def.constructor(0, |s, nt, _a| crate::custom::construct_html_element(s, nt));
        reflect_str!(def, "title", "title");
        reflect_str!(def, "lang", "lang");
        reflect_str!(def, "dir", "dir");
        reflect_str!(def, "accessKey", "accesskey");
        reflect_str!(def, "nonce", "nonce");
        reflect_bool!(def, "hidden", "hidden");
        reflect_bool!(def, "inert", "inert");
        reflect_bool!(def, "autofocus", "autofocus");
        reflect_str!(def, "popover", "popover");
        def.accessor(
            "tabIndex",
            |s, n| {
                let dom = n.page().dom.borrow();
                let explicit = dom.get_attribute(n.id, "tabindex").js()?.and_then(|v| v.trim().parse::<i32>().ok());
                let default = if dom.is_focusable(n.id).unwrap_or(false) { 0 } else { -1 };
                Ok(s.number(explicit.unwrap_or(default) as f64))
            },
            |_s, n, v| {
                let arg0 = to_i32(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, "tabindex", &arg0.to_string()).js()
            },
        );
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
            Ok(s.boolean(false))
        });
        def.getter("dataset", |s, n| Ok(collections::dataset(s, n.page(), n.id)?.value()));
        def.accessor(
            "style",
            |s, n| Ok(crate::css::inline_style(s, n.page(), n.id)?.value()),
            |_s, n, v| crate::css::set_inline_style(n.page(), n.id, v),
        );
        def.accessor("innerText", crate::layout_api::inner_text, crate::layout_api::set_inner_text);
        def.accessor("outerText", crate::layout_api::inner_text, crate::layout_api::set_inner_text);
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
        def.method("attachInternals", 0, |s, _n, _a| s.eval("({setFormValue(){}, setValidity(){}, states: new Set(), form: null, labels: [], willValidate: false, checkValidity(){return true}, reportValidity(){return true}})", None, 1));
        def.method("showPopover", 0, |s, _n, _a| Ok(s.undefined()));
        def.method("hidePopover", 0, |s, _n, _a| Ok(s.undefined()));
        def.method("togglePopover", 0, |s, _n, _a| Ok(s.boolean(false)));
    }
}

simple_class!(HtmlUnknownElement, "HTMLUnknownElement", HtmlElement);
simple_class!(HtmlHtmlElement, "HTMLHtmlElement", HtmlElement);
simple_class!(HtmlHeadElement, "HTMLHeadElement", HtmlElement);
simple_class!(HtmlBodyElement, "HTMLBodyElement", HtmlElement);
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
});
simple_class!(HtmlLinkElement, "HTMLLinkElement", HtmlElement, |def| {
    reflect_url!(def, "href", "href");
    reflect_str!(def, "rel", "rel");
    reflect_str!(def, "media", "media");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "as", "as");
    reflect_str!(def, "crossOrigin", "crossorigin");
    reflect_str!(def, "integrity", "integrity");
    reflect_str!(def, "hreflang", "hreflang");
    reflect_bool!(def, "disabled", "disabled");
    def.getter("relList", |s, n| Ok(collections::token_list(s, n.page(), n.id, "rel")?.value()));
    def.getter("sheet", crate::css::element_sheet);
});
simple_class!(HtmlStyleElement, "HTMLStyleElement", HtmlElement, |def| {
    reflect_str!(def, "media", "media");
    reflect_str!(def, "type", "type");
    reflect_bool!(def, "disabled", "disabled");
    def.getter("sheet", crate::css::element_sheet);
});
simple_class!(HtmlScriptElement, "HTMLScriptElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "charset", "charset");
    reflect_str!(def, "crossOrigin", "crossorigin");
    reflect_str!(def, "integrity", "integrity");
    reflect_str!(def, "referrerPolicy", "referrerpolicy");
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
        |_s, n, v| {
            let arg0 = dom_string(v)?;
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
    reflect_str!(def, "referrerPolicy", "referrerpolicy");
    reflect_str!(def, "ping", "ping");
    def.getter("relList", |s, n| Ok(collections::token_list(s, n.page(), n.id, "rel")?.value()));
    def.method("toString", 0, |s, n, _a| {
        let dom = n.page().dom.borrow();
        Ok(s.string(&dom.get_attribute(n.id, "href").js()?.map(|h| resolve_url(&dom, n.id, h)).unwrap_or_default()))
    });
    macro_rules! part {
        ($prop:literal, $f:expr) => {
            def.getter($prop, |s, n| {
                let dom = n.page().dom.borrow();
                let href = dom.get_attribute(n.id, "href").js()?.map(|h| resolve_url(&dom, n.id, h));
                let url = href.and_then(|h| lp_net::Url::parse(&h).ok());
                let f: fn(&lp_net::Url) -> String = $f;
                Ok(s.string(&url.as_ref().map(f).unwrap_or_default()))
            });
        };
    }
    part!("origin", |u| u.origin().ascii_serialization());
    part!("protocol", |u| format!("{}:", u.scheme()));
    part!("host", |u| match u.port() {
        Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
        None => u.host_str().unwrap_or("").to_owned(),
    });
    part!("hostname", |u| u.host_str().unwrap_or("").to_owned());
    part!("port", |u| u.port().map(|p| p.to_string()).unwrap_or_default());
    part!("pathname", |u| u.path().to_owned());
    part!("search", |u| u.query().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default());
    part!("hash", |u| u.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default());
    part!("username", |u| u.username().to_owned());
    part!("password", |u| u.password().unwrap_or("").to_owned());
}

simple_class!(HtmlAnchorElement, "HTMLAnchorElement", HtmlElement, |def| {
    define_hyperlink(def);
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
});
simple_class!(HtmlImageElement, "HTMLImageElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "alt", "alt");
    reflect_str!(def, "srcset", "srcset");
    reflect_str!(def, "sizes", "sizes");
    reflect_str!(def, "crossOrigin", "crossorigin");
    reflect_str!(def, "useMap", "usemap");
    reflect_str!(def, "loading", "loading");
    reflect_str!(def, "decoding", "decoding");
    reflect_bool!(def, "isMap", "ismap");
    reflect_long!(def, "width", "width", 0);
    reflect_long!(def, "height", "height", 0);
    // Natural size from the loaded file's header (no pixel decoding); 0
    // while loading, for broken images and for unparsed formats.
    def.getter("naturalWidth", |s, n| Ok(s.number(n.page().dom.borrow().natural_size(n.id).map_or(0.0, |(w, _)| w))));
    def.getter("naturalHeight", |s, n| Ok(s.number(n.page().dom.borrow().natural_size(n.id).map_or(0.0, |(_, h)| h))));
    def.getter("complete", |s, n| Ok(s.boolean(n.page().image_complete(n.id))));
    def.getter("currentSrc", |s, n| {
        let dom = n.page().dom.borrow();
        Ok(s.string(&dom.get_attribute(n.id, "src").js()?.map(|v| resolve_url(&dom, n.id, v)).unwrap_or_default()))
    });
    def.method("decode", 0, |s, _n, _a| s.eval("Promise.resolve()", None, 1));
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
            let mut dom = n.page().dom.borrow_mut();
            let (_, e) = dom.selection(n.id).js()?;
            dom.set_selection_range(n.id, st, e.max(st)).js()
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
            let mut dom = n.page().dom.borrow_mut();
            let (st, _) = dom.selection(n.id).js()?;
            dom.set_selection_range(n.id, st, end).js()
        },
    );
    def.accessor("selectionDirection", |s, _n| Ok(s.string("none")), |_s, _n, _v| Ok(()));
    def.method("setSelectionRange", 2, |s, n, a| {
        if !has_selection(n) {
            return Err(JsError::dom("InvalidStateError", "The input element's type does not support selection."));
        }
        let arg0 = to_u32(a.get(0))?;
        let arg1 = to_u32(a.get(1))?;
        n.page().dom.borrow_mut().set_selection_range(n.id, arg0, arg1).js()?;
        Ok(s.undefined())
    });
    def.method("select", 0, |s, n, _a| {
        // select() also applies to number inputs, though the selection API
        // does not: Chromium selects their whole value, so the next edit
        // (Playwright fill: select(), then Input.insertText) replaces it.
        let number = n.page().dom.borrow().input_type(n.id).is_ok_and(|t| t == InputType::Number);
        if has_selection(n) || number {
            let mut dom = n.page().dom.borrow_mut();
            let len = lp_dom::dom::utf16_len(&dom.value(n.id).js()?);
            dom.set_selection_range(n.id, 0, len).js()?;
            drop(dom);
            crate::input::fire_simple(n.page(), n.id, "select", true, false);
        }
        Ok(s.undefined())
    });
    def.method("setRangeText", 1, |s, n, a| {
        let text = arg_string(&a, 0, "setRangeText")?;
        let range = if a.len() >= 3 { Some((to_u32(a.get(1))?, to_u32(a.get(2))?)) } else { None };
        let mut dom = n.page().dom.borrow_mut();
        if let Some((st, en)) = range {
            dom.set_selection_range(n.id, st, en).js()?;
        }
        dom.insert_text(n.id, &text).js()?;
        Ok(s.undefined())
    });
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
    reflect_str!(def, "autocomplete", "autocomplete");
    reflect_str!(def, "inputMode", "inputmode");
    reflect_str!(def, "enterKeyHint", "enterkeyhint");
    reflect_str!(def, "dirName", "dirname");
    reflect_url!(def, "src", "src");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "readOnly", "readonly");
    reflect_bool!(def, "required", "required");
    reflect_bool!(def, "multiple", "multiple");
    reflect_bool!(def, "formNoValidate", "formnovalidate");
    reflect_long!(def, "maxLength", "maxlength", -1);
    reflect_long!(def, "minLength", "minlength", -1);
    reflect_long!(def, "size", "size", 20);
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
        |s, n| {
            let dom = n.page().dom.borrow();
            let ty = dom.input_type(n.id).js()?;
            let v = dom.value(n.id).js()?;
            Ok(s.number(match ty {
                InputType::Number | InputType::Range => v.parse::<f64>().unwrap_or(f64::NAN),
                _ => f64::NAN,
            }))
        },
        |_s, n, v| {
            let x = v.to_number()?;
            let text = if x.is_finite() { format_number(x) } else { String::new() };
            n.page().dom.borrow_mut().set_value(n.id, &text).js()
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
    def.method("stepUp", 0, |s, n, a| step(s, n, a, 1.0));
    def.method("stepDown", 0, |s, n, a| step(s, n, a, -1.0));
    def.method("showPicker", 0, |s, _n, _a| Ok(s.undefined()));
});

fn format_number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

fn step<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, dir: f64) -> JsResult<Value<'s>> {
    let k = if a.get(0).is_undefined() { 1.0 } else { a.get(0).to_number()? };
    let mut dom = n.page().dom.borrow_mut();
    let step = dom.get_attribute(n.id, "step").js()?.and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
    let cur = dom.value(n.id).js()?.parse::<f64>().unwrap_or(0.0);
    let mut v = cur + dir * k * step;
    if let Some(max) = dom.get_attribute(n.id, "max").js()?.and_then(|x| x.parse::<f64>().ok()) {
        v = v.min(max);
    }
    if let Some(min) = dom.get_attribute(n.id, "min").js()?.and_then(|x| x.parse::<f64>().ok()) {
        v = v.max(min);
    }
    dom.set_value(n.id, &format_number(v)).js()?;
    Ok(s.undefined())
}

simple_class!(HtmlTextAreaElement, "HTMLTextAreaElement", HtmlElement, |def| {
    define_labels(def);
    define_validation(def);
    define_text_selection(def);
    reflect_str!(def, "name", "name");
    reflect_str!(def, "placeholder", "placeholder");
    reflect_str!(def, "autocomplete", "autocomplete");
    reflect_str!(def, "wrap", "wrap");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "readOnly", "readonly");
    reflect_bool!(def, "required", "required");
    reflect_long!(def, "maxLength", "maxlength", -1);
    reflect_long!(def, "minLength", "minlength", -1);
    reflect_long!(def, "rows", "rows", 2);
    reflect_long!(def, "cols", "cols", 20);
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
        Ok(s.number(lp_dom::dom::utf16_len(&n.page().dom.borrow().value(n.id).js()?) as f64))
    });
});

simple_class!(HtmlSelectElement, "HTMLSelectElement", HtmlElement, |def| {
    define_labels(def);
    define_validation(def);
    reflect_str!(def, "name", "name");
    reflect_str!(def, "autocomplete", "autocomplete");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "required", "required");
    reflect_bool!(def, "multiple", "multiple");
    reflect_long!(def, "size", "size", 0);
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
    def.method("item", 1, |s, n, a| {
        let i = to_u32(a.get(0))? as usize;
        let o = n.page().dom.borrow().select_options(n.id).js()?.get(i).copied();
        node_value(s, n.page(), o)
    });
    def.indexed(|s, n, i| {
        let o = n.page().dom.borrow().select_options(n.id).js()?.get(i as usize).copied();
        match o {
            Some(o) => Ok(Some(wrap_node(s, n.page(), o)?.value())),
            None => Ok(None),
        }
    });
    def.method("namedItem", 1, |s, n, a| {
        let name = dom_string(a.get(0))?;
        let dom = n.page().dom.borrow();
        let found = dom.select_options(n.id).js()?.into_iter().find(|&o| {
            dom.element(o).is_ok_and(|e| e.id() == Some(name.as_str()) || e.attr("name") == Some(name.as_str()))
        });
        drop(dom);
        node_value(s, n.page(), found)
    });
    def.method("add", 1, |s, n, a| {
        let el = crate::node::node_arg(s, a.get(0), "HTMLSelectElement.add")?;
        let before = crate::node::opt_node_arg(s, a.get(1), "HTMLSelectElement.add").ok().flatten();
        let mut dom = n.page().dom.borrow_mut();
        let parent = match before {
            Some(b) => dom.parent(b).js()?.unwrap_or(n.id),
            None => n.id,
        };
        dom.insert_before(parent, el, before).js()?;
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
    reflect_str!(def, "formAction", "formaction");
    reflect_str!(def, "formMethod", "formmethod");
    reflect_str!(def, "formTarget", "formtarget");
    reflect_bool!(def, "disabled", "disabled");
    reflect_bool!(def, "formNoValidate", "formnovalidate");
    def.getter("form", form_value);
    def.accessor(
        "type",
        |s, n| {
            let dom = n.page().dom.borrow();
            let t = dom.get_attribute(n.id, "type").js()?.map(|t| t.to_ascii_lowercase());
            Ok(s.string(match t.as_deref() {
                Some("reset") => "reset",
                Some("button") => "button",
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
    reflect_str!(def, "enctype", "enctype");
    reflect_str!(def, "acceptCharset", "accept-charset");
    reflect_str!(def, "autocomplete", "autocomplete");
    reflect_bool!(def, "noValidate", "novalidate");
    def.accessor(
        "action",
        |s, n| {
            let dom = n.page().dom.borrow();
            let doc = dom.node_document(n.id).js()?;
            let base = dom.document_data(doc).js()?.url.to_string();
            Ok(s.string(&match dom.get_attribute(n.id, "action").js()? {
                Some(a) if !a.is_empty() => resolve_url(&dom, n.id, a),
                _ => base,
            }))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "action", &arg0).js()
        },
    );
    def.accessor(
        "method",
        |s, n| {
            let m = n.page().dom.borrow().get_attribute(n.id, "method").js()?.map(|m| m.to_ascii_lowercase());
            Ok(s.string(match m.as_deref() {
                Some("post") => "post",
                Some("dialog") => "dialog",
                _ => "get",
            }))
        },
        |_s, n, v| {
            let arg0 = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "method", &arg0).js()
        },
    );
    def.getter(
        "elements",
        |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?.value()),
    );
    def.getter("length", |s, n| {
        let c = collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?;
        c.get("length")
    });
    def.indexed(|s, n, i| {
        let c = collections::collection(s, n.page(), n.id, CollectionKind::FormElements)?;
        let v = c.get_index(i)?;
        Ok(if v.is_undefined() { None } else { Some(v) })
    });
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
    let labelable = |id: NodeId| {
        dom.element(id).is_ok_and(|e| {
            e.is_html()
                && (matches!(&*e.name.local, "button" | "meter" | "output" | "progress" | "select" | "textarea")
                    || (&*e.name.local == "input"
                        && dom.input_type(id).map(|t| t != InputType::Hidden).unwrap_or(false)))
        })
    };
    if let Ok(Some(f)) = dom.get_attribute(label, "for") {
        let root = dom.root_node(label, false).ok()?;
        let target = dom.get_element_by_id(root, f).ok()??;
        return labelable(target).then_some(target);
    }
    dom.descendants(label).ok()?.skip(1).find(|&d| labelable(d))
}

pub(crate) fn labels_for(dom: &Dom, control: NodeId) -> Vec<NodeId> {
    let Ok(root) = dom.root_node(control, false) else { return Vec::new() };
    let Ok(desc) = dom.descendants(root) else { return Vec::new() };
    desc.filter(|&n| dom.element(n).is_ok_and(|e| e.is_html() && &*e.name.local == "label"))
        .filter(|&l| labeled_control(dom, l) == Some(control))
        .collect()
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
simple_class!(HtmlLegendElement, "HTMLLegendElement", HtmlElement);
simple_class!(HtmlUListElement, "HTMLUListElement", HtmlElement);
simple_class!(HtmlOListElement, "HTMLOListElement", HtmlElement, |def| {
    reflect_bool!(def, "reversed", "reversed");
    reflect_long!(def, "start", "start", 1);
    reflect_str!(def, "type", "type");
});
simple_class!(HtmlLiElement, "HTMLLIElement", HtmlElement, |def| {
    reflect_long!(def, "value", "value", 0);
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

simple_class!(HtmlTableElement, "HTMLTableElement", HtmlElement, |def| {
    def.getter("rows", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::TableRows)?.value()));
    def.getter("tBodies", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::TableBodies)?.value()));
    def.getter("tHead", |s, n| table_section(s, n, "thead"));
    def.getter("tFoot", |s, n| table_section(s, n, "tfoot"));
    def.getter("caption", |s, n| table_section(s, n, "caption"));
    reflect_str!(def, "border", "border");
    reflect_str!(def, "width", "width");
});
simple_class!(HtmlTableSectionElement, "HTMLTableSectionElement", HtmlElement, |def| {
    def.getter("rows", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::SectionRows)?.value()));
});
simple_class!(HtmlTableRowElement, "HTMLTableRowElement", HtmlElement, |def| {
    def.getter("cells", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::RowCells)?.value()));
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
    reflect_long!(def, "colSpan", "colspan", 1);
    reflect_long!(def, "rowSpan", "rowspan", 1);
    reflect_str!(def, "headers", "headers");
    reflect_str!(def, "scope", "scope");
    reflect_str!(def, "abbr", "abbr");
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
simple_class!(HtmlTableCaptionElement, "HTMLTableCaptionElement", HtmlElement);
simple_class!(HtmlTableColElement, "HTMLTableColElement", HtmlElement, |def| {
    reflect_long!(def, "span", "span", 1);
});

// ----- embedded / misc --------------------------------------------------------------

simple_class!(HtmlIFrameElement, "HTMLIFrameElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "srcdoc", "srcdoc");
    reflect_str!(def, "name", "name");
    reflect_str!(def, "allow", "allow");
    reflect_str!(def, "width", "width");
    reflect_str!(def, "height", "height");
    reflect_str!(def, "loading", "loading");
    reflect_str!(def, "referrerPolicy", "referrerpolicy");
    reflect_bool!(def, "allowFullscreen", "allowfullscreen");
    def.getter("sandbox", |s, n| Ok(collections::token_list(s, n.page(), n.id, "sandbox")?.value()));
    def.getter("contentDocument", |s, n| {
        if let Some(document) = n.page().frame_document_value(s, n.id)? {
            return Ok(document);
        }
        let doc = n.page().frame_document(n.id);
        node_value(s, n.page(), doc)
    });
    def.getter("contentWindow", |s, n| {
        // Child browsing contexts are owned by lp-browser; the hook returns
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
});
simple_class!(HtmlSlotElement, "HTMLSlotElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
    def.method("assignedNodes", 0, |s, _n, _a| Ok(s.array(&[])?.value()));
    def.method("assignedElements", 0, |s, _n, _a| Ok(s.array(&[])?.value()));
});
simple_class!(HtmlCanvasElement, "HTMLCanvasElement", HtmlElement, |def| {
    reflect_long!(def, "width", "width", 300);
    reflect_long!(def, "height", "height", 150);
    // "2d": text measurement and state without a rasterizer (canvas.rs);
    // other context types are unsupported (null).
    def.method("getContext", 1, |s, n, a| {
        let kind = arg_string(&a, 0, "HTMLCanvasElement.getContext")?;
        crate::canvas::get_context(s, n.page(), n.id, &kind)
    });
    def.method("toDataURL", 0, |s, _n, _a| Ok(s.string("data:,")));
});
simple_class!(HtmlMediaElement, "HTMLMediaElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_bool!(def, "autoplay", "autoplay");
    reflect_bool!(def, "loop", "loop");
    reflect_bool!(def, "controls", "controls");
    reflect_bool!(def, "muted", "muted");
    reflect_str!(def, "preload", "preload");
    def.getter("paused", |s, _n| Ok(s.boolean(true)));
    def.getter("currentTime", |s, _n| Ok(s.number(0.0)));
    def.getter("duration", |s, _n| Ok(s.number(f64::NAN)));
    def.getter("readyState", |s, _n| Ok(s.number(0.0)));
    def.method("play", 0, |s, _n, _a| {
        s.eval("Promise.reject(new DOMException('Media playback is not supported', 'NotSupportedError'))", None, 1)
    });
    def.method("pause", 0, |s, _n, _a| Ok(s.undefined()));
    def.method("load", 0, |s, _n, _a| Ok(s.undefined()));
    def.method("canPlayType", 1, |s, _n, _a| Ok(s.string("")));
});
simple_class!(HtmlVideoElement, "HTMLVideoElement", HtmlMediaElement, |def| {
    reflect_long!(def, "width", "width", 0);
    reflect_long!(def, "height", "height", 0);
    def.getter("videoWidth", |s, _n| Ok(s.number(0.0)));
    def.getter("videoHeight", |s, _n| Ok(s.number(0.0)));
});
simple_class!(HtmlAudioElement, "HTMLAudioElement", HtmlMediaElement);
simple_class!(HtmlBrElement, "HTMLBRElement", HtmlElement);
simple_class!(HtmlHrElement, "HTMLHRElement", HtmlElement);
simple_class!(HtmlPreElement, "HTMLPreElement", HtmlElement);
simple_class!(HtmlQuoteElement, "HTMLQuoteElement", HtmlElement, |def| {
    reflect_url!(def, "cite", "cite");
});
simple_class!(HtmlDListElement, "HTMLDListElement", HtmlElement);
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
    def.method("show", 0, |s, n, _a| {
        n.page().dom.borrow_mut().set_attribute(n.id, "open", "").js()?;
        Ok(s.undefined())
    });
    def.method("showModal", 0, |s, n, _a| {
        n.page().dom.borrow_mut().set_attribute(n.id, "open", "").js()?;
        Ok(s.undefined())
    });
    def.method("close", 0, |s, n, a| {
        let was_open = n.page().dom.borrow().has_attribute(n.id, "open").js()?;
        if !was_open {
            return Ok(s.undefined());
        }
        if !a.get(0).is_undefined() {
            n.page().set_dialog_return(n.id, dom_string(a.get(0))?);
        }
        n.page().dom.borrow_mut().remove_attribute(n.id, "open").js()?;
        crate::input::queue_simple(n.page(), n.id, "close", false, false);
        Ok(s.undefined())
    });
});
simple_class!(HtmlDetailsElement, "HTMLDetailsElement", HtmlElement, |def| {
    reflect_bool!(def, "open", "open");
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlProgressElement, "HTMLProgressElement", HtmlElement, |def| {
    define_labels(def);
    def.getter("value", |s, n| {
        Ok(s.number(
            n.page().dom.borrow().get_attribute(n.id, "value").js()?.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0),
        ))
    });
    def.getter("max", |s, n| {
        Ok(s.number(
            n.page().dom.borrow().get_attribute(n.id, "max").js()?.and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0),
        ))
    });
});
simple_class!(HtmlMeterElement, "HTMLMeterElement", HtmlElement, |def| {
    define_labels(def);
});
simple_class!(HtmlOutputElement, "HTMLOutputElement", HtmlElement, |def| {
    define_labels(def);
    reflect_str!(def, "name", "name");
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
    reflect_url!(def, "href", "href");
    reflect_str!(def, "target", "target");
});
simple_class!(HtmlObjectElement, "HTMLObjectElement", HtmlElement, |def| {
    reflect_url!(def, "data", "data");
    reflect_str!(def, "type", "type");
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlEmbedElement, "HTMLEmbedElement", HtmlElement, |def| {
    reflect_url!(def, "src", "src");
    reflect_str!(def, "type", "type");
});
simple_class!(HtmlMapElement, "HTMLMapElement", HtmlElement, |def| {
    reflect_str!(def, "name", "name");
});
simple_class!(HtmlModElement, "HTMLModElement", HtmlElement, |def| {
    reflect_url!(def, "cite", "cite");
    reflect_str!(def, "dateTime", "datetime");
});
simple_class!(HtmlMenuElement, "HTMLMenuElement", HtmlElement);

pub struct SvgElement;
impl JsClass for SvgElement {
    const NAME: &'static str = "SVGElement";
    type Data = NodeData;
    type Parent = Element;
    fn define(def: &mut ClassDef<Self>) {
        crate::handlers::define_global(def);
        def.getter("dataset", |s, n| Ok(collections::dataset(s, n.page(), n.id)?.value()));
        def.accessor(
            "style",
            |s, n| Ok(crate::css::inline_style(s, n.page(), n.id)?.value()),
            |_s, n, v| crate::css::set_inline_style(n.page(), n.id, v),
        );
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
        def.getter("tabIndex", |s, n| {
            Ok(s.number(
                n.page()
                    .dom
                    .borrow()
                    .get_attribute(n.id, "tabindex")
                    .js()?
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(-1) as f64,
            ))
        });
    }
}
simple_class!(SvgGraphicsElement, "SVGGraphicsElement", SvgElement, |def| {
    // No rasterizer, but geometry is attribute-defined: the user-space box
    // of shapes, paths, text and containers (lp_dom::svg_geometry).
    def.method("getBBox", 0, |s, n, _a| {
        let page = n.page();
        let dom = page.dom.borrow();
        let layout = page.layout.borrow();
        crate::layout_api::rect_value(s, lp_dom::svg_geometry::bbox(&dom, layout.style(), n.id))
    });
});
simple_class!(SvgSvgElement, "SVGSVGElement", SvgGraphicsElement);

// ----- document helpers ---------------------------------------------------------------

pub(crate) fn shadow_mode(dom: &Dom, root: NodeId) -> &'static str {
    match dom.shadow_root_mode(root) {
        Ok(Some(lp_dom::ShadowRootMode::Closed)) => "closed",
        _ => "open",
    }
}

pub(crate) fn ready_state(page: &PageDom, doc: NodeId) -> &'static str {
    page.ready_state(doc)
}

pub(crate) fn document_cookie(page: &PageDom, doc: NodeId) -> String {
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.to_string()).unwrap_or_default();
    let (Some(net), Ok(url)) = (page.net.borrow().clone(), lp_net::Url::parse(&url)) else { return String::new() };
    let jar = net.cookies().clone();
    let mut jar = jar.lock().unwrap();
    jar.document_cookie(&url, lp_net::cookies::now_secs())
}

pub(crate) fn set_document_cookie(page: &PageDom, doc: NodeId, value: &str) {
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.to_string()).unwrap_or_default();
    let (Some(net), Ok(url)) = (page.net.borrow().clone(), lp_net::Url::parse(&url)) else { return };
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    let jar = net.cookies().clone();
    let mut jar = jar.lock().unwrap();
    jar.set_from_header(&url, value, lp_net::cookies::Access::Script, lp_net::cookies::now_secs());
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

pub(crate) fn upgrade_if_custom<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    crate::custom::create_element_value(s, page, el)
}

// ----- document.open / write / close ----------------------------------------
//
// Dynamic markup insertion on a loaded document (what Playwright's
// page.setContent does): open() empties the document and starts a
// script-created parser, write() parses into it synchronously, running inline
// classic scripts in the main world as they are reached (their own writes are
// parsed at the insertion point), and close() ends it;
// the embedder then reports DOMContentLoaded/load. Writing while the network
// parser is still loading the document, and external or module scripts in
// written markup, are reported explicitly as unsupported.

fn html_document(page: &PageDom, doc: NodeId) -> JsResult<()> {
    let html = page.dom.borrow().document_data(doc).map(|d| d.kind == lp_dom::DocumentKind::Html).js()?;
    if !html {
        return Err(JsError::dom("InvalidStateError", "Only HTML documents support dynamic markup insertion."));
    }
    Ok(())
}

fn network_parser_active(page: &PageDom, doc: NodeId) -> bool {
    !page.script_parser_active.get() && page.ready_state(doc) != "complete"
}

/// `document.open()` (the two-argument form; three arguments open a window).
pub(crate) fn document_open<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = n.page();
    if a.len() >= 3 {
        let open = s.global().get("open")?;
        let open = open.as_object().ok_or_else(|| JsError::Type("window.open is not a function".into()))?;
        return open.call(None, &[a.get(0), a.get(1), a.get(2)]);
    }
    html_document(page, n.id)?;
    let document = node_value(s, page, Some(n.id))?;
    // From a script the script-created parser is running: no-op.
    if page.script_parser_active.get() && page.script_nesting.get() > 0 {
        return Ok(document);
    }
    open_document(page, n.id)?;
    Ok(document)
}

fn open_document(page: &Rc<PageDom>, doc: NodeId) -> JsResult<()> {
    if network_parser_active(page, doc) {
        return Err(JsError::dom(
            "NotSupportedError",
            "document.open/write while the document is still loading is not supported by lightningpanda",
        ));
    }
    if let Some(mut old) = page.script_parser.borrow_mut().take() {
        old.end();
    }
    // Erase all event listeners and handlers of the document's nodes and window.
    let mut keys: Vec<TargetKey> = {
        let dom = page.dom.borrow();
        dom.descendants(doc).js()?.filter_map(|id| dom.node_key(id)).collect()
    };
    keys.extend(page.window_target());
    {
        let mut dom = page.dom.borrow_mut();
        for key in keys {
            dom.erase_listeners(key);
        }
        dom.replace_all(doc, None).js()?;
    }
    page.set_ready_state(doc, "loading");
    let parser = lp_dom::parse::DocumentParser::new(page.dom.clone(), doc, !page.scripting_disabled()).js()?;
    *page.script_parser.borrow_mut() = Some(parser);
    page.script_parser_active.set(true);
    page.script_parser_close.set(false);
    if let Some(hooks) = page.hooks() {
        hooks.document_reopened();
    }
    Ok(())
}

/// `document.write`/`writeln`.
pub(crate) fn document_write<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, ln: bool) -> JsResult<Value<'s>> {
    let page = n.page();
    html_document(page, n.id)?;
    let mut text = String::new();
    for i in 0..a.len() {
        text.push_str(&dom_string(a.get(i))?);
    }
    if ln {
        text.push('\n');
    }
    if !page.script_parser_active.get() {
        // Implicit open on a loaded document.
        open_document(page, n.id)?;
    }
    if page.script_nesting.get() > 0 {
        // From a script the parser is running: parse at the insertion point
        // now, before the rest of the outer input.
        let Some(step) = with_parser(page, |p| p.begin_nested(&text)) else { return Ok(s.undefined()) };
        let result = drive_parser(page, n.id, step);
        with_parser(page, |p| p.end_nested());
        return result.map(|_| s.undefined());
    }
    let Some(step) = with_parser(page, |p| p.feed(&text)) else { return Ok(s.undefined()) };
    let result = drive_parser(page, n.id, step);
    if page.script_parser_close.take() {
        close_document(page, n.id);
    }
    result.map(|_| s.undefined())
}

/// `document.close()`.
pub(crate) fn document_close<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    html_document(page, n.id)?;
    if !page.script_parser_active.get() {
        return Ok(s.undefined());
    }
    if page.script_nesting.get() > 0 {
        // From a script the parser is running: close once the outer write
        // returns.
        page.script_parser_close.set(true);
        return Ok(s.undefined());
    }
    close_document(page, n.id);
    Ok(s.undefined())
}

fn close_document(page: &Rc<PageDom>, doc: NodeId) {
    if let Some(step) = with_parser(page, |p| p.resume()) {
        let _ = drive_parser(page, doc, step);
    }
    let Some(mut parser) = page.script_parser.borrow_mut().take() else { return };
    parser.end();
    page.script_parser_active.set(false);
    crate::lifecycle::set_ready_state(page, doc, "interactive");
    if !page.hooks().is_some_and(|h| h.document_closed()) {
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

/// Runs `f` on the script-created parser. The parser is borrowed only for the
/// tokenizer call, never while a script runs, so scripts can write to it.
fn with_parser<R>(page: &PageDom, f: impl FnOnce(&mut lp_dom::parse::DocumentParser) -> R) -> Option<R> {
    let mut parser = page.script_parser.borrow_mut().take()?;
    let result = f(&mut parser);
    let mut slot = page.script_parser.borrow_mut();
    if slot.is_none() {
        *slot = Some(parser);
    }
    Some(result)
}

/// Runs the parser until it needs input, executing the scripts it yields.
/// The first script error is returned after parsing continued past it.
fn drive_parser(page: &Rc<PageDom>, doc: NodeId, mut step: lp_dom::parse::ParseStep) -> JsResult<()> {
    let mut first_error = None;
    while let lp_dom::parse::ParseStep::Script(node) = step {
        if let Err(err) = run_parser_script(page, doc, node) {
            first_error.get_or_insert(err);
        }
        match with_parser(page, |p| p.resume()) {
            Some(next) => step = next,
            None => break,
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn run_parser_script(page: &Rc<PageDom>, doc: NodeId, node: NodeId) -> JsResult<()> {
    use lp_dom::script::ScriptKind;
    if page.scripting_disabled() {
        return Ok(());
    }
    let info = {
        let mut dom = page.dom.borrow_mut();
        let Ok(info) = dom.script_info(node) else { return Ok(()) };
        if info.already_started {
            return Ok(());
        }
        let _ = dom.mark_script_started(node);
        info
    };
    match info.kind {
        ScriptKind::Classic if !info.nomodule && info.src.is_none() => {}
        ScriptKind::Classic if !info.nomodule => {
            return Err(JsError::dom(
                "NotSupportedError",
                "external scripts in document.write markup are not supported by lightningpanda",
            ))
        }
        ScriptKind::Module => {
            return Err(JsError::dom(
                "NotSupportedError",
                "module scripts in document.write markup are not supported by lightningpanda",
            ))
        }
        // nomodule classic scripts, import maps and data blocks do not run.
        _ => return Ok(()),
    }
    let Some(realm) = page.main_world_realm() else { return Ok(()) };
    let url = page.dom.borrow().document_data(doc).map(|d| d.url.clone()).unwrap_or_default();
    let scope = realm.scope()?;
    page.set_current_script(Some(node));
    page.script_nesting.set(page.script_nesting.get() + 1);
    if let Err(e) = scope.eval(&info.inline_text, Some(&url), 1) {
        scope.report_exception(&e);
    }
    page.script_nesting.set(page.script_nesting.get() - 1);
    page.set_current_script(None);
    Ok(())
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
            let doc = dom.create_document(lp_dom::DocumentKind::Html, "about:blank");
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
            let ns = opt_dom_string(a.get(0))?;
            let q = null_to_empty(a.get(1))?;
            let mut dom = h.page.dom.borrow_mut();
            let doc = dom.create_document(lp_dom::DocumentKind::Xml, "about:blank");
            if !q.is_empty() {
                let el = dom.create_element_ns(doc, ns.as_deref(), &q).js()?;
                dom.append_child(doc, el).js()?;
            }
            drop(dom);
            Ok(wrap_node(s, &h.page, doc)?.value())
        });
        def.method("createDocumentType", 3, |s, h, a| {
            a.require(3, "DOMImplementation.createDocumentType")?;
            let (name, public, system) = (dom_string(a.get(0))?, dom_string(a.get(1))?, dom_string(a.get(2))?);
            let mut dom = h.page.dom.borrow_mut();
            let dt = dom.create_doctype(h.id, &name, &public, &system).js()?;
            drop(dom);
            Ok(wrap_node(s, &h.page, dt)?.value())
        });
        def.method("hasFeature", 0, |s, _h, _a| Ok(s.boolean(true)));
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
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
    engine.register::<SvgElement>();
    engine.register::<SvgGraphicsElement>();
    engine.register::<SvgSvgElement>();
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
            .get("preventScroll")?
            .to_bool()
    };
    crate::input::focus_element_with_options(n.page(), n.id, prevent_scroll);
    Ok(s.undefined())
}
