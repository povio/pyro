//! Form data set construction and form submission encoding (HTML §4.10.21).

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use encoding_rs::Encoding;
use pyro_dom::forms::InputType;
use pyro_dom::{Dom, NodeId};
use pyro_js::{Object, Persistent, Scope};

use crate::event::{EventCore, EventInterface};
use crate::file::FileEntry;
use crate::page::{NavigationRequest, PageDom};

#[derive(Clone, Debug)]
pub enum EntryValue {
    Text(String),
    File(FileEntry),
}

/// A submitted form's entry list, as its navigation carries it. Lists
/// compare by identity (a navigation request is equal to itself).
#[derive(Clone, Debug)]
pub struct FormEntries(pub Rc<Vec<(String, EntryValue)>>);

impl PartialEq for FormEntries {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FormEntries {}

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

/// The form controls' entries for `form` (submitter included if given),
/// without the `formdata` event: see [`construct_entry_list`].
pub fn form_entries(page: &PageDom, form: NodeId, submitter: Option<NodeId>) -> Vec<(String, EntryValue)> {
    form_entries_with_charset(page, form, submitter, "UTF-8", None)
}

thread_local! {
    /// Forms whose `formdata` event is being dispatched (page id, form):
    /// their "constructing entry list" flag.
    static CONSTRUCTING: RefCell<Vec<(u64, NodeId)>> = const { RefCell::new(Vec::new()) };
}

/// An entry list: the controls' entries, or the FormData a `formdata`
/// event carried (listeners may have changed it).
pub(crate) enum EntryList<'s> {
    Entries(Vec<(String, EntryValue)>),
    FormData(Object<'s>),
}

/// HTML "constructing the entry list" for `form`: None when the form is
/// already constructing one (a `formdata` listener asked again). When a
/// `formdata` listener was ever added in the tree and a realm is given,
/// the entries are put in a new FormData of that realm and a `formdata`
/// event carrying it is fired at the form; otherwise no event can be
/// observed and the entries are returned as they are.
pub(crate) fn construct_entry_list<'s>(
    s: Option<&Scope<'s>>,
    page: &Rc<PageDom>,
    form: NodeId,
    submitter: Option<NodeId>,
    charset: &str,
) -> Option<EntryList<'s>> {
    let key = (page.id(), form);
    if CONSTRUCTING.with(|c| c.borrow().contains(&key)) {
        return None;
    }
    let mut sources = Vec::new();
    let entries = form_entries_with_charset(page, form, submitter, charset, Some(&mut sources));
    let Some(s) = s.filter(|_| page.dom.borrow().has_formdata_listeners()) else {
        return Some(EntryList::Entries(entries));
    };
    let Some(target) = page.dom.borrow().node_key(form) else { return Some(EntryList::Entries(entries)) };
    let fd = match crate::form_data::with_entries(s, entries)
        .and_then(|fd| crate::form_data::adopt_input_files(s, page, fd, &sources).map(|()| fd))
    {
        Ok(fd) => fd,
        Err(e) => {
            s.report_exception(&e);
            return Some(EntryList::Entries(Vec::new()));
        }
    };
    let ev = EventCore::trusted(page, EventInterface::FormDataEvent, "formdata", true, false);
    ev.fields.borrow_mut().form_data = Some(Persistent::new(s, fd.value()));
    CONSTRUCTING.with(|c| c.borrow_mut().push(key));
    crate::target::fire(page, target, &ev);
    CONSTRUCTING.with(|c| c.borrow_mut().retain(|k| *k != key));
    Some(EntryList::FormData(fd))
}

/// `form_entries`, reporting `charset` in a hidden `_charset_` control.
/// `sources` gets (entry index, file input, file index) of each selected
/// file's entry.
fn form_entries_with_charset(
    page: &PageDom,
    form: NodeId,
    submitter: Option<NodeId>,
    charset: &str,
    mut sources: Option<&mut Vec<(usize, NodeId, usize)>>,
) -> Vec<(String, EntryValue)> {
    let dom = page.dom.borrow();
    let mut out = Vec::new();
    let controls = dom.owned_form_elements(form).unwrap_or_default();
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
                            push_dirname(&dom, c, &mut out);
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
                            for (k, f) in files.into_iter().enumerate() {
                                if let Some(sources) = sources.as_deref_mut() {
                                    sources.push((out.len(), c, k));
                                }
                                out.push((name.clone(), EntryValue::File(f)));
                            }
                        }
                    }
                    InputType::Hidden if name.eq_ignore_ascii_case("_charset_") => {
                        out.push((name, EntryValue::Text(charset.to_owned())));
                        push_dirname(&dom, c, &mut out);
                    }
                    _ => {
                        if !name.is_empty() {
                            out.push((name, EntryValue::Text(dom.value(c).unwrap_or_default())));
                            if matches!(
                                ty,
                                InputType::Hidden
                                    | InputType::Text
                                    | InputType::Search
                                    | InputType::Tel
                                    | InputType::Url
                                    | InputType::Email
                                    | InputType::Password
                            ) {
                                push_dirname(&dom, c, &mut out);
                            }
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
                let mut v = dom.value(c).unwrap_or_default();
                // HTML "textarea wrapping transformation": wrap="hard" (any case) breaks the
                // submitted value's lines at the element's cols.
                if e.attr("wrap").is_some_and(|w| w.eq_ignore_ascii_case("hard")) {
                    let cols = e
                        .attr("cols")
                        .and_then(|c| pyro_dom::forms::parse_html_int(c))
                        .filter(|&c| c > 0)
                        .map_or(20, |c| c.min(u32::MAX as i64) as usize);
                    v = hard_wrap(&v, cols);
                }
                out.push((name, EntryValue::Text(v)));
                push_dirname(&dom, c, &mut out);
            }
            _ if dom.is_form_associated(c) => crate::internals::face_entries(&dom, c, &mut out),
            _ => {}
        }
    }
    out
}

/// Inserts line feeds so that no line of `value` is longer than `cols` characters: after
/// the last space that fits (as Chrome), else at `cols`. The entry keeps LF; the
/// serializers turn every newline into CRLF.
fn hard_wrap(value: &str, cols: usize) -> String {
    let mut out = String::with_capacity(value.len() + value.len() / cols.max(1) * 2);
    for (i, line) in value.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Walked by index: draining the front per wrapped line was quadratic on a long line.
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        while chars.len() - start > cols {
            let cut = chars[start..=start + cols].iter().rposition(|&c| c == ' ').map_or(cols, |p| p + 1);
            let cut = if cut == 0 { cols } else { cut };
            out.extend(&chars[start..start + cut]);
            out.push('\n');
            start += cut;
        }
        out.extend(&chars[start..]);
    }
    out
}

/// HTML "constructing the entry list": a control whose `dirname` applies and is not empty
/// adds an entry of that name with its directionality, "ltr" or "rtl", after its own.
fn push_dirname(dom: &Dom, c: NodeId, out: &mut Vec<(String, EntryValue)>) {
    let Some(dirname) = dom.element(c).ok().and_then(|e| e.attr("dirname")).filter(|d| !d.is_empty()) else {
        return;
    };
    let dir = match dom.directionality(c) {
        Ok(pyro_dom::direction::Direction::Rtl) => "rtl",
        _ => "ltr",
    };
    out.push((dirname.to_owned(), EntryValue::Text(dir.to_owned())));
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
        let value = match v {
            EntryValue::Text(t) => t,
            EntryValue::File(f) => &f.name,
        };
        ser.append_pair(&crlf(k), &crlf(value));
    }
    ser.finish()
}

/// multipart/form-data body with names and text values in `enc`; returns
/// (content type, body).
pub fn multipart(entries: &[(String, EntryValue)], enc: &'static Encoding) -> (String, Vec<u8>) {
    let boundary = format!("----pyroFormBoundary{:016x}", rand_u64());
    let mut body = Vec::new();
    let esc = |s: &str| s.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A");
    for (k, v) in entries {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match v {
            EntryValue::Text(t) => {
                body.extend_from_slice(&encode_entry(
                    enc,
                    &format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", esc(&crlf(k))),
                ));
                body.extend_from_slice(&encode_entry(enc, &crlf(t)));
            }
            EntryValue::File(f) => {
                body.extend_from_slice(&encode_entry(
                    enc,
                    &format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                        esc(&crlf(k)),
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

/// Whether submitting `form` (from `submitter`) uses the dialog method.
pub(crate) fn is_dialog_method(dom: &Dom, form: NodeId, submitter: Option<NodeId>) -> bool {
    attr_override(dom, form, submitter, "method", "formmethod").is_some_and(|m| m.eq_ignore_ascii_case("dialog"))
}

/// The result a dialog submission closes its dialog with: an image
/// button's selected coordinate as "x,y", else the submitter's value
/// attribute (None: null, the return value is kept).
pub(crate) fn dialog_result(page: &PageDom, submitter: Option<NodeId>) -> Option<String> {
    let s = submitter?;
    let dom = page.dom.borrow();
    let image = dom.element(s).is_ok_and(|e| e.is_html() && *e.local_name() == *"input")
        && dom.input_type(s).ok() == Some(InputType::Image);
    if image {
        let (x, y) = selected_coordinate(page, s);
        return Some(format!("{x},{y}"));
    }
    dom.get_attribute(s, "value").ok().flatten().map(str::to_owned)
}

/// The entry list a submission sends; None (no submission) when the form
/// is already constructing one. The `formdata` event's FormData is made in
/// the main world.
fn submission_entries(
    page: &Rc<PageDom>,
    form: NodeId,
    submitter: Option<NodeId>,
    charset: &str,
) -> Option<Vec<(String, EntryValue)>> {
    let realm = page.dom.borrow().has_formdata_listeners().then(|| page.main_world_realm()).flatten();
    let scope = realm.as_ref().and_then(|r| r.scope().ok());
    match construct_entry_list(scope.as_ref(), page, form, submitter, charset)? {
        EntryList::Entries(entries) => Some(entries),
        EntryList::FormData(fd) => {
            let s = scope.as_ref()?;
            match crate::form_data::entries_of(s, fd) {
                Ok(entries) => Some(entries),
                Err(e) => {
                    s.report_exception(&e);
                    None
                }
            }
        }
    }
}

/// `s` with every CR, LF and CRLF as CRLF.
fn crlf(s: &str) -> Cow<'_, str> {
    if !s.contains(['\r', '\n']) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n"))
}

/// Builds the navigation for submitting `form` (None for `method=dialog`
/// or an unresolvable action).
pub(crate) fn form_submission(
    page: &Rc<PageDom>,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Option<NavigationRequest> {
    let (enc, method) = {
        let dom = page.dom.borrow();
        let method = attr_override(&dom, form, submitter, "method", "formmethod").map(|m| m.to_ascii_lowercase());
        let method = match method.as_deref() {
            Some("post") => "POST",
            Some("dialog") => return None,
            _ => "GET",
        };
        (form_encoding(&dom, form), method)
    };
    let navigations = page.navigation_requests();
    let entries = submission_entries(page, form, submitter, enc.name())?;
    // A `formdata` listener that navigated the document's frame itself
    // (`navigation.navigate`, `location`) supersedes the submission, as in
    // Chromium: the form does not navigate after it.
    if page.navigation_requests() != navigations {
        return None;
    }
    let dom = page.dom.borrow();
    // A `formdata` listener can disconnect the form: it cannot navigate.
    if !dom.is_connected(form).unwrap_or(false) {
        return None;
    }
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
    // HTML "get an element's noopener" with the form's rel: a named target keeps its opener.
    let rel = dom.get_attribute(form, "rel").ok().flatten().unwrap_or_default().to_owned();
    drop(dom);
    // CSP `form-action` (no fallback): a blocked submission does not navigate.
    if let Some(list) = crate::csp::policy_list(page) {
        let outcome = list.form_action_check(&url, false);
        crate::csp::report(page, outcome.violations, Some(form));
        if outcome.blocked {
            return None;
        }
    }
    // HTML "submit": a form that navigates its own document before it has
    // completely loaded replaces the document's entry.
    let replace = target.is_none() && page.navigates_by_replacing();
    let noopener = crate::window::element_noopener(&rel, target.as_deref());
    // HTML "submit" planned navigation: rel=noreferrer sends no referrer.
    let referrer_policy = rel
        .split_ascii_whitespace()
        .any(|t| t.eq_ignore_ascii_case("noreferrer"))
        .then_some(pyro_net::ReferrerPolicy::NoReferrer);
    if method == "GET" {
        url.set_query(Some(&urlencode(&entries, enc)));
        let popup = crate::window::element_popup_ticket(page, target.as_deref(), noopener);
        return Some(NavigationRequest {
            url: url.to_string(),
            target,
            method: "GET".into(),
            body: None,
            content_type: None,
            opener: popup.is_some(),
            popup,
            window_features: Default::default(),
            referrer_policy,
            source: Some(submitter.unwrap_or(form)),
            form_entries: None,
            replace,
            user_initiated: crate::input::user_initiated_navigation(),
        });
    }
    let (ct, body) = match enctype.as_deref() {
        Some("multipart/form-data") => multipart(&entries, enc),
        Some("text/plain") => {
            let mut b = String::new();
            for (k, v) in &entries {
                let v = match v {
                    EntryValue::Text(t) => t,
                    EntryValue::File(f) => &f.name,
                };
                b.push_str(&format!("{}={}\r\n", crlf(k), crlf(v)));
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
        window_features: Default::default(),
        referrer_policy,
        source: Some(submitter.unwrap_or(form)),
        form_entries: Some(FormEntries(Rc::new(entries))),
        replace,
        user_initiated: crate::input::user_initiated_navigation(),
    })
}

/// A new FormData of `s`'s realm holding `entries` (the Navigation API's
/// `NavigateEvent.formData` for a POST form submission).
pub fn form_data_object<'s>(s: &Scope<'s>, entries: &FormEntries) -> pyro_js::JsResult<Object<'s>> {
    crate::form_data::with_entries(s, entries.0.to_vec())
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if SELECTED_COORDINATE.get().is_some() {
        return Some("formdata::SELECTED_COORDINATE");
    }
    CONSTRUCTING.with(|c| !c.borrow().is_empty()).then_some("formdata::CONSTRUCTING")
}
