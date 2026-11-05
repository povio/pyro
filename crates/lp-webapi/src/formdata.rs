//! Form data set construction and form submission encoding (HTML §4.10.21).

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use encoding_rs::Encoding;
use lp_dom::forms::InputType;
use lp_dom::{Dom, NodeId};

use crate::file::FileEntry;
use crate::page::{NavigationRequest, PageDom};

#[derive(Clone, Debug)]
pub enum EntryValue {
    Text(String),
    File(FileEntry),
}

thread_local! {
    /// The image button whose activation is submitting its form right now,
    /// with its "selected coordinate" (page id, element, x, y). Set around
    /// the submission so that `new FormData(form, submitter)` in a `submit`
    /// handler, and the submission itself, see the click's coordinates; an
    /// image button that is not being activated submits (0, 0).
    static SELECTED_COORDINATE: Cell<Option<(u64, NodeId, i64, i64)>> = const { Cell::new(None) };
}

/// Runs `f` (a form submission from the image button `el`) with `el`'s
/// selected coordinate set to (`x`, `y`); nested submissions restore the
/// outer coordinate afterwards.
pub(crate) fn with_selected_coordinate<R>(page: &PageDom, el: NodeId, x: i64, y: i64, f: impl FnOnce() -> R) -> R {
    let outer = SELECTED_COORDINATE.with(|c| c.replace(Some((page.id(), el, x, y))));
    let result = f();
    SELECTED_COORDINATE.with(|c| c.set(outer));
    result
}

/// The selected coordinate of the image button `el` of `page`: (0, 0)
/// unless it is the button being activated.
fn selected_coordinate(page: &PageDom, el: NodeId) -> (i64, i64) {
    SELECTED_COORDINATE.with(|c| match c.get() {
        Some((p, e, x, y)) if p == page.id() && e == el => (x, y),
        _ => (0, 0),
    })
}

/// "Constructing the entry list" for `form` (submitter included if given).
pub fn form_entries(page: &PageDom, form: NodeId, submitter: Option<NodeId>) -> Vec<(String, EntryValue)> {
    form_entries_with_charset(page, form, submitter, "UTF-8")
}

/// `form_entries`, reporting `charset` in a hidden `_charset_` control.
fn form_entries_with_charset(
    page: &PageDom,
    form: NodeId,
    submitter: Option<NodeId>,
    charset: &str,
) -> Vec<(String, EntryValue)> {
    let dom = page.dom.borrow();
    let mut out = Vec::new();
    let Ok(root) = dom.root_node(form, false) else { return out };
    let Ok(desc) = dom.descendants(root) else { return out };
    let controls: Vec<NodeId> = desc.filter(|&n| dom.form_owner(n).ok().flatten() == Some(form)).collect();
    for c in controls {
        let Ok(e) = dom.element(c) else { continue };
        if !e.is_html() || dom.is_disabled(c).unwrap_or(false) {
            continue;
        }
        let local = e.local_name().to_string();
        let name = e.attr("name").unwrap_or("").to_owned();
        match local.as_str() {
            "button" => {
                if Some(c) == submitter && !name.is_empty() {
                    out.push((name, EntryValue::Text(e.attr("value").unwrap_or("").to_owned())));
                }
            }
            "input" => {
                let ty = dom.input_type(c).unwrap_or(InputType::Text);
                match ty {
                    InputType::Submit | InputType::Reset | InputType::Button => {
                        if ty == InputType::Submit && Some(c) == submitter && !name.is_empty() {
                            out.push((name, EntryValue::Text(dom.value(c).unwrap_or_default())));
                        }
                    }
                    InputType::Image => {
                        if Some(c) == submitter {
                            let prefix = if name.is_empty() { String::new() } else { format!("{name}.") };
                            let (x, y) = selected_coordinate(page, c);
                            out.push((format!("{prefix}x"), EntryValue::Text(x.to_string())));
                            out.push((format!("{prefix}y"), EntryValue::Text(y.to_string())));
                        }
                    }
                    InputType::Checkbox | InputType::Radio => {
                        if !name.is_empty() && dom.checked(c).unwrap_or(false) {
                            out.push((name, EntryValue::Text(dom.value(c).unwrap_or_default())));
                        }
                    }
                    InputType::File => {
                        if name.is_empty() {
                            continue;
                        }
                        let files = page.files.borrow().get(&c).cloned().unwrap_or_default();
                        if files.is_empty() {
                            out.push((
                                name,
                                EntryValue::File(FileEntry {
                                    name: String::new(),
                                    mime: "application/octet-stream".into(),
                                    bytes: bytes::Bytes::new(),
                                    last_modified: 0.0,
                                }),
                            ));
                        } else {
                            for f in files {
                                out.push((name.clone(), EntryValue::File(f)));
                            }
                        }
                    }
                    InputType::Hidden if name.eq_ignore_ascii_case("_charset_") => {
                        out.push((name, EntryValue::Text(charset.to_owned())));
                    }
                    _ => {
                        if !name.is_empty() {
                            out.push((name, EntryValue::Text(dom.value(c).unwrap_or_default())));
                        }
                    }
                }
            }
            "select" => {
                if name.is_empty() {
                    continue;
                }
                for o in dom.selected_options(c).unwrap_or_default() {
                    if !dom.is_disabled(o).unwrap_or(false) {
                        out.push((name.clone(), EntryValue::Text(dom.option_value(o).unwrap_or_default())));
                    }
                }
            }
            "textarea" if !name.is_empty() => {
                let v = dom.value(c).unwrap_or_default().replace('\n', "\r\n");
                out.push((name, EntryValue::Text(v)));
            }
            _ => {}
        }
    }
    out
}

/// `s` in the submission encoding, with `&#NNN;` for unmappable characters.
fn encode_entry<'a>(enc: &'static Encoding, s: &'a str) -> Cow<'a, [u8]> {
    if enc == encoding_rs::UTF_8 {
        Cow::Borrowed(s.as_bytes())
    } else {
        Cow::Owned(crate::encoding::encode_with_ncr(enc, s, "&#", ";"))
    }
}

/// application/x-www-form-urlencoded serialization in `enc`.
pub fn urlencode(entries: &[(String, EntryValue)], enc: &'static Encoding) -> String {
    let encode: &dyn Fn(&str) -> Cow<'_, [u8]> = &|s| encode_entry(enc, s);
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    if enc != encoding_rs::UTF_8 {
        ser.encoding_override(Some(encode));
    }
    for (k, v) in entries {
        match v {
            EntryValue::Text(t) => ser.append_pair(k, t),
            EntryValue::File(f) => ser.append_pair(k, &f.name),
        };
    }
    ser.finish()
}

/// multipart/form-data body with names and text values in `enc`; returns
/// (content type, body).
pub fn multipart(entries: &[(String, EntryValue)], enc: &'static Encoding) -> (String, Vec<u8>) {
    let boundary = format!("----lightningpandaFormBoundary{:016x}", rand_u64());
    let mut body = Vec::new();
    let esc = |s: &str| s.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A");
    for (k, v) in entries {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match v {
            EntryValue::Text(t) => {
                body.extend_from_slice(&encode_entry(
                    enc,
                    &format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", esc(k)),
                ));
                body.extend_from_slice(&encode_entry(enc, t));
            }
            EntryValue::File(f) => {
                body.extend_from_slice(&encode_entry(
                    enc,
                    &format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                        esc(k),
                        esc(&f.name),
                        if f.mime.is_empty() { "application/octet-stream" } else { &f.mime }
                    ),
                ));
                body.extend_from_slice(&f.bytes);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(crate::encoding::now_ms() as u64);
    h.finish()
}

fn attr_override(
    dom: &Dom,
    form: NodeId,
    submitter: Option<NodeId>,
    form_attr: &str,
    sub_attr: &str,
) -> Option<String> {
    submitter
        .and_then(|s| dom.get_attribute(s, sub_attr).ok().flatten().map(str::to_owned))
        .or_else(|| dom.get_attribute(form, form_attr).ok().flatten().map(str::to_owned))
}

/// "Pick an encoding for the form": the first usable `accept-charset` label,
/// else the document's encoding, as an output encoding.
fn form_encoding(dom: &Dom, form: NodeId) -> &'static Encoding {
    dom.get_attribute(form, "accept-charset")
        .ok()
        .flatten()
        .and_then(|v| v.split_ascii_whitespace().find_map(|l| Encoding::for_label(l.as_bytes())))
        .map_or_else(|| crate::html::document_output_encoding(dom, form), |e| e.output_encoding())
}

/// Builds the navigation for submitting `form` (None for `method=dialog`
/// or an unresolvable action).
pub(crate) fn form_submission(
    page: &Rc<PageDom>,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Option<NavigationRequest> {
    let enc = form_encoding(&page.dom.borrow(), form);
    let entries = form_entries_with_charset(page, form, submitter, enc.name());
    let dom = page.dom.borrow();
    let method = attr_override(&dom, form, submitter, "method", "formmethod").map(|m| m.to_ascii_lowercase());
    let method = match method.as_deref() {
        Some("post") => "POST",
        Some("dialog") => return None,
        _ => "GET",
    };
    let action = attr_override(&dom, form, submitter, "action", "formaction").filter(|a| !a.is_empty());
    let doc = dom.node_document(form).ok()?;
    let doc_url = dom.document_data(doc).ok()?.url.to_string();
    let action = match action {
        Some(a) => crate::html::resolve_url(&dom, form, &a),
        None => doc_url,
    };
    let mut url = url::Url::parse(&action).ok()?;
    let enctype = attr_override(&dom, form, submitter, "enctype", "formenctype").map(|e| e.to_ascii_lowercase());
    let target = attr_override(&dom, form, submitter, "target", "formtarget").filter(|t| !t.is_empty() && t != "_self");
    drop(dom);
    if method == "GET" {
        url.set_query(Some(&urlencode(&entries, enc)));
        return Some(NavigationRequest {
            url: url.to_string(),
            target,
            method: "GET".into(),
            body: None,
            content_type: None,
            opener: false,
            popup: None,
        });
    }
    let (ct, body) = match enctype.as_deref() {
        Some("multipart/form-data") => multipart(&entries, enc),
        Some("text/plain") => {
            let mut b = String::new();
            for (k, v) in &entries {
                let v = match v {
                    EntryValue::Text(t) => t.clone(),
                    EntryValue::File(f) => f.name.clone(),
                };
                b.push_str(&format!("{k}={v}\r\n"));
            }
            ("text/plain".to_owned(), encode_entry(enc, &b).into_owned())
        }
        _ => ("application/x-www-form-urlencoded".to_owned(), urlencode(&entries, enc).into_bytes()),
    };
    Some(NavigationRequest {
        url: url.to_string(),
        target,
        method: "POST".into(),
        body: Some(body),
        content_type: Some(ct),
        opener: false,
        popup: None,
    })
}
