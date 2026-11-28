//! Copy, cut and paste: the trusted `copy`/`cut`/`paste` ClipboardEvent
//! (bubbles, cancelable, composed) with its `clipboardData` DataTransfer,
//! and the default actions on the browser context's clipboard (the store
//! `navigator.clipboard` reads and writes, `crate::navigator_extras`).
//!
//! Keyboard shortcuts and Playwright's `copy`/`cut`/`paste` commands are the
//! user's: they need no clipboard permission, as in Chromium. Script reaches
//! copy and cut through `execCommand`; reading the clipboard from script
//! goes through `navigator.clipboard` and its permission only.

use std::rc::Rc;

use crate::edit_commands::EditSource;

use pyro_dom::NodeId;

use crate::data_transfer::{Mode, NativeTransfer};
use crate::editing;
use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Copy,
    Cut,
    Paste,
    /// Paste the clipboard's text only (Ctrl+Shift+V, pasteAndMatchStyle).
    PastePlain,
}

/// Where the clipboard operation applies.
enum Source {
    Control(NodeId),
    Host(NodeId),
    Document,
}

fn source(page: &PageDom) -> Source {
    let dom = page.dom.borrow();
    let focused = dom.focused(page.document()).ok().flatten();
    if let Some(f) = focused {
        if editing::is_text_entry_control(&dom, f) {
            return Source::Control(f);
        }
        if let Some(h) = editing::editing_host(&dom, f) {
            return Source::Host(h);
        }
    }
    drop(dom);
    match editing::selection_range(page) {
        Some((s, _)) => match editing::editing_host(&page.dom.borrow(), s.0) {
            Some(h) => Source::Host(h),
            None => Source::Document,
        },
        None => Source::Document,
    }
}

/// The event's target: the focused text control, else the element where
/// the selection starts, else the focused element or the body.
fn event_target(page: &PageDom, source: &Source) -> Option<NodeId> {
    if let Source::Control(c) = source {
        return Some(*c);
    }
    let dom = page.dom.borrow();
    if let Some((s, _)) = editing::selection_range(page) {
        let el = if editing::is_text(&dom, s.0) { dom.parent_element(s.0).ok().flatten() } else { Some(s.0) };
        if let Some(el) = el.filter(|&e| dom.node_type(e).ok() == Some(pyro_dom::NodeType::Element)) {
            return Some(el);
        }
    }
    let doc = page.document();
    dom.focused(doc)
        .ok()
        .flatten()
        .or_else(|| dom.body(doc).ok().flatten())
        .or_else(|| dom.document_element(doc).ok().flatten())
}

/// The selection as copied: (text/plain, text/html).
fn selected(page: &Rc<PageDom>, source: &Source) -> Option<(String, Option<String>)> {
    if let Source::Control(c) = source {
        let dom = page.dom.borrow();
        let (s, e) = dom.selection(*c).ok()?;
        if s == e {
            return None;
        }
        // A password's value is not copied.
        if dom.input_type(*c).ok() == Some(pyro_dom::forms::InputType::Password) {
            return None;
        }
        let units: Vec<u16> = dom.value(*c).ok()?.encode_utf16().collect();
        let slice = units.get(s as usize..(e as usize).min(units.len()))?;
        return Some((String::from_utf16_lossy(slice), None));
    }
    let (start, end) = editing::selection_range(page)?;
    if start == end {
        return None;
    }
    let text = crate::edit_commands::selection_text(page, start, end);
    let html = crate::range::html_between(page, start, end);
    Some((text, html))
}

/// The selected text of the focused text control (`Selection.toString()` reports it, as Chrome does: the user selected
/// text in a control, whose selection the document's own selection does not hold); `None` when no control is focused or
/// its selection is empty.
pub(crate) fn focused_control_selection(page: &Rc<PageDom>) -> Option<String> {
    let src = source(page);
    match src {
        Source::Control(_) => selected(page, &src).map(|(text, _)| text),
        _ => None,
    }
}

fn write(page: &PageDom, items: Vec<(String, String)>) {
    page.context_services().write_clipboard_strings(items);
}

/// Copy, cut or paste as the user does it. Returns false when there was
/// nothing it could act on (execCommand's result).
pub(crate) fn run(page: &Rc<PageDom>, action: Action, origin: EditSource) -> bool {
    let doc = page.document();
    let src = source(page);
    let Some(target) = event_target(page, &src) else { return false };
    let paste = matches!(action, Action::Paste | Action::PastePlain);
    let clipboard = if paste { page.context_services().clipboard_strings() } else { Vec::new() };
    let transfer =
        NativeTransfer::new(page, &clipboard, "uninitialized", if paste { Mode::ReadOnly } else { Mode::ReadWrite });
    let ty = match action {
        Action::Copy => "copy",
        Action::Cut => "cut",
        Action::Paste | Action::PastePlain => "paste",
    };
    let key = page.dom.borrow().node_key(target);
    let not_canceled = match (&transfer, key) {
        (Some(t), Some(key)) => {
            let ev = EventCore::trusted(page, EventInterface::ClipboardEvent, ty, true, true);
            ev.state.borrow_mut().composed = true;
            ev.fields.borrow_mut().data_transfer = Some(t.reference.clone());
            crate::target::fire(page, key, &ev)
        }
        _ => true,
    };
    if page.document() != doc || page.is_disposed() {
        return true;
    }
    // The event's handlers may have moved focus or the selection.
    let src = source(page);
    match action {
        Action::Copy | Action::Cut => {
            if !not_canceled {
                if let Some(t) = &transfer {
                    let items = t.strings();
                    if !items.is_empty() {
                        write(page, items);
                    }
                }
            } else if action == Action::Cut && matches!(src, Source::Document) {
                // Chromium cuts only editable content.
            } else if let Some((text, html)) = selected(page, &src) {
                let mut items = vec![("text/plain".to_owned(), text)];
                if let Some(html) = html.filter(|h| !h.is_empty()) {
                    items.push(("text/html".to_owned(), html));
                }
                write(page, items);
                if action == Action::Cut {
                    cut_selection(page, &src, origin);
                }
            }
        }
        Action::Paste | Action::PastePlain => {
            if not_canceled {
                let text = clipboard.iter().find(|(t, _)| t == "text/plain").map(|(_, d)| d.as_str()).unwrap_or("");
                let html = clipboard
                    .iter()
                    .find(|(t, _)| t == "text/html")
                    .map(|(_, d)| d.as_str())
                    .filter(|_| action == Action::Paste);
                match src {
                    Source::Control(c) if !text.is_empty() => {
                        crate::edit_commands::control_insert(page, c, text, "insertFromPaste");
                    }
                    Source::Host(h) if !text.is_empty() || html.is_some() => {
                        let plaintext_only = page
                            .dom
                            .borrow()
                            .get_attribute(h, "contenteditable")
                            .ok()
                            .flatten()
                            .is_some_and(|v| v.eq_ignore_ascii_case("plaintext-only"));
                        let html = html.filter(|_| !plaintext_only);
                        editing::insert_pasted(page, h, text, html, transfer.as_ref());
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(t) = &transfer {
        t.set_mode(Mode::Disabled);
    }
    true
}

/// Cut's deletion: the selection of the control or editable content
/// (`deleteByCut`); nothing outside editable content.
fn cut_selection(page: &Rc<PageDom>, src: &Source, origin: EditSource) {
    match *src {
        Source::Control(c) => crate::edit_commands::control_delete(
            page,
            c,
            false,
            crate::edit_commands::Gran::Character,
            Some("deleteByCut"),
            origin,
        ),
        Source::Host(h) => {
            let (s, e) = editing::host_range(page, h);
            if s != e {
                // A script's cut fires no beforeinput (Input Events), as in a control.
                match origin {
                    EditSource::Script => {
                        editing::without_beforeinput(|| editing::delete_span(page, h, "deleteByCut", s, e))
                    }
                    EditSource::User => editing::delete_span(page, h, "deleteByCut", s, e),
                }
            }
        }
        Source::Document => {}
    }
}
