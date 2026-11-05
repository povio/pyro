//! HTML's render-blocking mechanism: the document's render-blocking element
//! set. While it is not empty the document has no rendering opportunity, so
//! animation frame callbacks wait.
//!
//! The loader blocks rendering on the scripts and stylesheets it fetches
//! (`PageDom::block_rendering`) and unblocks it when they are done.
//! `<link rel=expect blocking=render href="#id">` elements are found here:
//! they block until the element their `href` indicates is parsed. As in the
//! standard, an element can only be added while the document is loading and
//! has no body yet.
//!
//! The set is re-evaluated at safe points instead of from every DOM hook
//! (`PageDom::update_render_blocking`): after each DOM call from script,
//! whenever the parser reaches a script, after each task, and before an
//! animation frame runs. An entry is dropped once it no longer blocks: its
//! element left the document, lost `blocking=render` (unless it blocks
//! implicitly), or, for an expect link, its target was parsed.
//!
//! Deviations: there is no time limit on render-blocking; a document without
//! a body is not render-blocked by that alone (only by the elements in the
//! set); `<style>` elements with pending `@import`s do not block.

use std::cell::{Cell, RefCell};

use lp_dom::{Dom, NodeId};

use crate::page::PageDom;

/// Why an element is in the render-blocking element set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBlocker {
    /// Implicitly render-blocking (a parser-inserted stylesheet, a
    /// parser-blocking script): blocks until the loader unblocks it,
    /// whatever its `blocking` attribute says.
    Implicit,
    /// Blocks while its `blocking` attribute has the `render` token.
    Explicit,
    /// `<link rel=expect>`: blocks until its target element is parsed.
    Expect,
}

#[derive(Default)]
pub(crate) struct RenderBlocking {
    /// The render-blocking element set; every node holds a DOM reference.
    set: RefCell<Vec<(NodeId, RenderBlocker)>>,
    /// The document no longer allows adding render-blocking elements (it
    /// has a body, or stopped loading). Never reopens.
    closed: Cell<bool>,
    /// The script the document's parser is paused at (it holds a DOM
    /// reference): its ancestors are the parser's open elements.
    parser_position: Cell<Option<NodeId>>,
    /// DOM version of the last evaluation, and whether something else the
    /// evaluation depends on changed since.
    version: Cell<Option<u64>>,
    dirty: Cell<bool>,
    evaluating: Cell<bool>,
    /// Run once rendering is no longer blocked.
    waiters: RefCell<Vec<Box<dyn FnOnce()>>>,
}

fn has_token(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|v| v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case(token)))
}

fn parser_inserted(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| e.flags & lp_dom::node::element_flags::PARSER_INSERTED != 0)
}

/// `url` without its fragment.
fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

impl PageDom {
    /// HTML "block rendering" on `el`: adds it to the render-blocking
    /// element set if the document still allows that (it is loading and has
    /// no body; an element the parser inserted before the body counts even
    /// when the loader learns of it after the body was parsed). Returns
    /// whether `el` now blocks rendering.
    pub fn block_rendering(&self, el: NodeId, kind: RenderBlocker) -> bool {
        if self.is_disposed() {
            return false;
        }
        let Ok(mut dom) = self.dom.try_borrow_mut() else { return false };
        let doc = self.document();
        if self.ready_state(doc) != "loading" || dom.node_document(el).ok() != Some(doc) {
            return false;
        }
        if let Some(body) = dom.body(doc).ok().flatten() {
            if !parser_inserted(&dom, el) || dom.contains(body, el).unwrap_or(true) {
                return false;
            }
        }
        let mut set = self.render_blocking.set.borrow_mut();
        if let Some(entry) = set.iter_mut().find(|(n, _)| *n == el) {
            entry.1 = kind;
            return true;
        }
        if dom.add_ref(el).is_err() {
            return false;
        }
        set.push((el, kind));
        true
    }

    /// HTML "unblock rendering" on `el` (it loaded, failed or ran).
    pub fn unblock_rendering(&self, el: NodeId) {
        let found = {
            let mut set = self.render_blocking.set.borrow_mut();
            let at = set.iter().position(|(n, _)| *n == el);
            at.map(|i| set.remove(i)).is_some()
        };
        if found {
            self.release_render_blocking_ref(el);
            self.render_blocking.dirty.set(true);
            self.update_render_blocking();
        }
    }

    fn release_render_blocking_ref(&self, node: NodeId) {
        self.release_node(node);
    }

    /// The script element the document's parser is paused at (`None` once
    /// it runs on or is done). An expect link's target that is an ancestor
    /// of it is still open in the parser, so not parsed yet.
    pub fn set_parser_position(&self, script: Option<NodeId>) {
        let pinned = script.filter(|&s| self.dom.try_borrow_mut().is_ok_and(|mut dom| dom.add_ref(s).is_ok()));
        if let Some(old) = self.render_blocking.parser_position.replace(pinned) {
            self.release_render_blocking_ref(old);
        }
        self.render_blocking.dirty.set(true);
    }

    pub(crate) fn render_blocking_changed(&self) {
        self.render_blocking.dirty.set(true);
    }

    /// Whether an evaluation could change anything (for the native callback
    /// hook, which runs after every DOM call from script).
    pub(crate) fn render_blocking_pending(&self, dom: &Dom) -> bool {
        let rb = &self.render_blocking;
        if rb.closed.get() && rb.set.borrow().is_empty() {
            return !rb.waiters.borrow().is_empty();
        }
        rb.dirty.get() || rb.version.get() != Some(dom.version())
    }

    /// Whether the document is render-blocked now (after re-evaluating the
    /// render-blocking element set).
    pub fn render_blocked(&self) -> bool {
        self.render_blocking.dirty.set(true);
        self.update_render_blocking();
        !self.render_blocking.set.borrow().is_empty()
    }

    /// Runs `f` once rendering is no longer blocked (at the evaluation that
    /// finds the set empty).
    pub fn when_render_unblocked(&self, f: Box<dyn FnOnce()>) {
        self.render_blocking.waiters.borrow_mut().push(f);
    }

    /// Re-evaluates the render-blocking element set at a safe point (no
    /// script runs; a DOM borrow held by the caller defers it).
    pub fn update_render_blocking(&self) {
        let rb = &self.render_blocking;
        if self.is_disposed() || rb.evaluating.replace(true) {
            return;
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&rb.evaluating);
        if !(rb.closed.get() && rb.set.borrow().is_empty()) {
            let Ok(mut dom) = self.dom.try_borrow_mut() else { return };
            let version = dom.version();
            if rb.dirty.replace(false) || rb.version.get() != Some(version) {
                rb.version.set(Some(version));
                self.evaluate_render_blocking(&mut dom);
            }
        }
        if rb.set.borrow().is_empty() {
            let waiters = std::mem::take(&mut *rb.waiters.borrow_mut());
            for waiter in waiters {
                waiter();
            }
        }
    }

    fn evaluate_render_blocking(&self, dom: &mut Dom) {
        let rb = &self.render_blocking;
        let doc = self.document();
        let loading = self.ready_state(doc) == "loading";
        let mut set = rb.set.borrow_mut();
        // "Process this type of link" for expect links, while the document
        // allows adding render-blocking elements. The last scan (the body
        // appeared) only takes links the parser inserted: it may have parsed
        // them and the body in one go.
        if !rb.closed.get() {
            let closing = !loading || dom.body(doc).ok().flatten().is_some();
            if loading {
                let links: Vec<NodeId> = match dom.head(doc).ok().flatten() {
                    Some(head) => dom
                        .descendants(head)
                        .map(|d| {
                            d.filter(|&n| {
                                dom.element(n).is_ok_and(|e| e.is_html() && &*e.local_name() == "link")
                                    && has_token(dom.get_attribute(n, "rel").ok().flatten(), "expect")
                            })
                            .collect()
                        })
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                for link in links {
                    if set.iter().any(|(n, _)| *n == link) || (closing && !parser_inserted(dom, link)) {
                        continue;
                    }
                    if self.expect_link_blocks(dom, link, true) && dom.add_ref(link).is_ok() {
                        set.push((link, RenderBlocker::Expect));
                    }
                }
            }
            if closing {
                rb.closed.set(true);
            }
        }
        let mut released = Vec::new();
        set.retain(|&(node, kind)| {
            let blocks = dom.is_connected(node).unwrap_or(false)
                && dom.node_document(node).ok() == Some(doc)
                && match kind {
                    RenderBlocker::Implicit => true,
                    RenderBlocker::Explicit => has_token(dom.get_attribute(node, "blocking").ok().flatten(), "render"),
                    RenderBlocker::Expect => self.expect_link_blocks(dom, node, loading),
                };
            if !blocks {
                released.push(node);
            }
            blocks
        });
        for node in released {
            dom.release(node);
        }
    }

    /// HTML "process internal resource link": whether the expect link
    /// `link` blocks rendering.
    fn expect_link_blocks(&self, dom: &Dom, link: NodeId, loading: bool) -> bool {
        let attr = |name| dom.get_attribute(link, name).ok().flatten();
        if !loading || !has_token(attr("rel"), "expect") || !has_token(attr("blocking"), "render") {
            return false;
        }
        if attr("media").is_some_and(|m| !m.trim().is_empty() && !lp_dom::media::matches(m, &self.viewport())) {
            return false;
        }
        let doc = self.document();
        let Some(href) = attr("href") else { return false };
        let Ok(document_url) = dom.document_data(doc).map(|d| d.url.to_string()) else { return false };
        let base = dom.document_base_url(doc).map(str::to_owned).unwrap_or_else(|_| document_url.clone());
        let Some(url) = url::Url::parse(&base).ok().and_then(|b| b.join(href).ok()) else { return false };
        if without_fragment(url.as_str()) != without_fragment(&document_url) {
            return false;
        }
        // No fragment (or an empty one) indicates the top of the document,
        // which is not an element to wait for.
        let Some(fragment) = url.fragment().filter(|f| !f.is_empty()) else { return false };
        let indicated = self.potential_indicated_element(dom, fragment).or_else(|| {
            let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8().ok()?;
            self.potential_indicated_element(dom, &decoded)
        });
        match indicated {
            None => true,
            // Found: it still blocks while the parser has it open.
            Some(element) => self
                .render_blocking
                .parser_position
                .get()
                .is_some_and(|script| script != element && dom.contains(element, script).unwrap_or(false)),
        }
    }

    /// HTML "find a potential indicated element": the element with that ID,
    /// else the first `a` element with that `name`.
    fn potential_indicated_element(&self, dom: &Dom, fragment: &str) -> Option<NodeId> {
        let doc = self.document();
        if let Some(element) = dom.get_element_by_id(doc, fragment).ok().flatten() {
            return Some(element);
        }
        dom.descendants(doc).ok()?.find(|&n| {
            dom.element(n).is_ok_and(|e| e.is_html() && &*e.local_name() == "a" && e.attr("name") == Some(fragment))
        })
    }

    /// Drops the set: the document is going away (`closed`), or the page
    /// shows another document, which starts with an empty, open set.
    pub(crate) fn clear_render_blocking(&self, closed: bool) {
        let rb = &self.render_blocking;
        let mut nodes: Vec<NodeId> = rb.set.borrow_mut().drain(..).map(|(n, _)| n).collect();
        nodes.extend(rb.parser_position.take());
        drop(std::mem::take(&mut *rb.waiters.borrow_mut()));
        rb.closed.set(closed);
        rb.version.set(None);
        rb.dirty.set(true);
        for node in nodes {
            self.release_render_blocking_ref(node);
        }
    }
}
