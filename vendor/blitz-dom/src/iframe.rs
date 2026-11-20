//! Loading of `<iframe>` elements into sub-documents.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::{AbortController, AbortSignal, Url};
use blitz_traits::node_id::NodeId;
use blitz_traits::shell::ShellProvider;

use crate::document::DocumentEvent;
use crate::net::{DocumentSrcHandler, ResourceHandler, stamped_request};
use crate::{BaseDocument, DocumentConfig, UserAgentStylesheet, local_name};

/// Maximum nesting depth of documents-within-documents. Iframes nested deeper
/// than this are not loaded (this guards against infinitely recursive
/// self-embedding pages).
pub(crate) const MAX_SUBDOCUMENT_DEPTH: usize = 10;

/// Per-iframe load state: the abort controller covering the current
/// "generation" of the iframe's content (the HTML fetch and, once loaded, the
/// sub-document's own sub-resource fetches), plus the id of the in-flight HTML
/// request (if any) so stale responses can be discarded.
pub(crate) struct IframeLoad {
    pub(crate) request_id: Option<usize>,
    pub(crate) abort_controller: AbortController,
}

/// A [`NavigationProvider`] given to iframe sub-documents. It routes
/// navigations (e.g. link clicks within the iframe) back to the parent
/// document, which reloads the iframe's sub-document with the new URL.
struct IframeNavigationProvider {
    node_id: NodeId,
    tx: Sender<DocumentEvent>,
    shell_provider: Arc<dyn ShellProvider>,
}

impl NavigationProvider for IframeNavigationProvider {
    fn navigate_to(&self, options: NavigationOptions) {
        let _ = self.tx.send(DocumentEvent::NavigateIframe {
            node_id: self.node_id,
            url: options.url,
        });
        self.shell_provider.request_redraw();
    }
}

impl BaseDocument {
    /// Build the [`DocumentConfig`] for an iframe sub-document, inheriting
    /// providers and settings from this (parent) document.
    fn iframe_document_config(
        &self,
        node_id: NodeId,
        base_url: Option<String>,
        abort_signal: AbortSignal,
        frame: Option<&crate::FrameContent>,
    ) -> DocumentConfig {
        // pyro: a render snapshot's frame document is its frame table entry's: the frame it is of
        // (its client) and its own frames come from there, never from an attribute.
        DocumentConfig {
            viewport: None,
            base_url,
            frame_client: frame.map(|frame| frame.client.clone()),
            frames: frame.map(|frame| frame.frames.clone()).unwrap_or_default(),
            ua_stylesheets: self.ua_stylesheets.clone(),
            net_provider: Some(self.net_provider.clone()),
            navigation_provider: Some(Arc::new(IframeNavigationProvider {
                node_id,
                tx: self.tx.clone(),
                shell_provider: self.shell_provider.clone(),
            })),
            shell_provider: Some(self.shell_provider.clone()),
            html_parser_provider: Some(self.html_parser_provider.clone()),
            font_ctx: Some(self.font_ctx.lock().unwrap().clone()),
            media_type: Some(self.media_type.clone()),
            style_threading: self.style_threading,
            incremental: Some(self.incremental_layout),
            abort_signal: Some(abort_signal),
            subdocument_depth: self.subdocument_depth + 1,
            line_break_override: self.layout_ctx.line_break_override(),
            // pyro: a frame's document is parsed as HTML.
            xml_document: false,
            quirks_mode: false,
            line_height_quirk: false,
            content_language: None,
            // A sub-document is a separate devtools target with its own provider
            #[cfg(feature = "devtools")]
            devtools_provider: None,
        }
    }

    /// Abort the current generation of the iframe's content (in-flight HTML
    /// fetch and/or the loaded sub-document's sub-resource fetches) and start
    /// a new generation.
    fn new_iframe_generation(&mut self, node_id: NodeId, request_id: Option<usize>) -> AbortSignal {
        if let Some(load) = self.iframe_loads.remove(&node_id) {
            load.abort_controller.abort();
        }
        let abort_controller = AbortController::default();
        let signal = abort_controller.signal.clone();
        self.iframe_loads.insert(
            node_id,
            IframeLoad {
                request_id,
                abort_controller,
            },
        );
        signal
    }

    /// Parse `html` and attach the resulting document as `node_id`'s
    /// sub-document.
    pub(crate) fn attach_iframe_document(
        &mut self,
        node_id: NodeId,
        html: &str,
        base_url: Option<String>,
        abort_signal: AbortSignal,
    ) {
        let config = self.iframe_document_config(node_id, base_url, abort_signal, None);
        let sub_doc = self
            .html_parser_provider
            .clone()
            .parse_document(html, config);
        self.set_sub_document(node_id, sub_doc);
        self.shell_provider.request_redraw();
    }

    /// pyro: the render snapshot's frame table now (`DocumentConfig::frames`): a frame element
    /// whose `data-pyro-frame` changes loads its new entry from it.
    pub fn set_frame_table(&mut self, frames: crate::FrameTable) {
        self.frames = frames;
    }

    /// pyro: the frame table entry the frame element `node_id` names (`data-pyro-frame`), if any.
    pub(crate) fn frame_entry(&self, node_id: NodeId) -> Option<crate::FrameContent> {
        let value = self.get_node(node_id)?.reserved_attr(crate::FRAME_ATTRIBUTE)?;
        self.frames.lookup(value).cloned()
    }

    /// pyro: whether this is a render snapshot's document (its frame table carries the nonce its
    /// markup was serialised with): its frame elements load only valid table entries.
    pub(crate) fn is_snapshot_document(&self) -> bool {
        !self.frames.nonce.is_empty()
    }

    /// pyro: the nonce this document's markup was serialised with (empty: no render snapshot's).
    pub fn snapshot_nonce(&self) -> &str {
        &self.frames.nonce
    }

    /// pyro: engine attribute `name` of element `node_id`, only when the element carries this
    /// document's nonce ([`crate::engine_marked`]).
    pub fn engine_attr(&self, node_id: NodeId, name: &str) -> Option<&str> {
        let element = self.get_node(node_id)?.element_data()?;
        crate::engine_marked(&self.frames.nonce, element).then(|| element.reserved_attr(name)).flatten()
    }

    /// pyro: the elements carrying engine attribute `name` and this document's nonce, in no
    /// particular order (an engine reader's walk: selectors never see the reserved family).
    pub fn engine_nodes(&self, name: &str) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, node)| {
                node.element_data().is_some_and(|e| e.reserved_attr(name).is_some() && crate::engine_marked(&self.frames.nonce, e))
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// pyro: whether the frame element `node_id` names a frame table entry (it then shows that
    /// entry's document or none, never its own attributes').
    pub(crate) fn names_frame(&self, node_id: NodeId) -> bool {
        self.get_node(node_id).is_some_and(|node| node.reserved_attr(crate::FRAME_ATTRIBUTE).is_some())
    }

    /// pyro: attach the frame table entry `frame` as `node_id`'s sub-document: its markup at its
    /// URL, made for its client, with its own frame table.
    pub(crate) fn load_frame_document(&mut self, node_id: NodeId, frame: &crate::FrameContent) {
        let signal = self.new_iframe_generation(node_id, None);
        let config = self.iframe_document_config(node_id, Some(frame.url.clone()), signal, Some(frame));
        let sub_doc = self.html_parser_provider.clone().parse_document(&frame.html, config);
        self.set_sub_document(node_id, sub_doc);
        self.shell_provider.request_redraw();
    }

    /// Attach a sub-document parsed from the iframe's `srcdoc` attribute.
    /// Relative URLs within a `srcdoc` document resolve against the parent
    /// document's base URL.
    pub(crate) fn load_iframe_srcdoc(&mut self, node_id: NodeId, srcdoc: &str) {
        let signal = self.new_iframe_generation(node_id, None);
        let base_url = Some(self.url.to_string());
        self.attach_iframe_document(node_id, srcdoc, base_url, signal);
    }

    /// Start fetching HTML for an iframe from `url`. The parsed document is
    /// attached when the response arrives (via [`DocumentEvent::ResourceLoad`]).
    pub(crate) fn start_iframe_load(&mut self, node_id: NodeId, url: Url) {
        let handler = ResourceHandler::new(
            self.tx.clone(),
            self.id(),
            Some(node_id),
            self.shell_provider.clone(),
            DocumentSrcHandler,
        );
        let signal = self.new_iframe_generation(node_id, Some(handler.request_id()));
        self.net_provider.fetch(
            self.id(),
            {
                let mut request = stamped_request(url, Some(&signal));
                request.options.destination = blitz_traits::net::Destination::Document;
                request
            },
            Box::new(handler),
        );
    }

    /// Handle a navigation originating from within an iframe's sub-document
    /// (e.g. a link click) by loading the new URL into the iframe.
    pub(crate) fn navigate_iframe(&mut self, node_id: NodeId, url: Url) {
        let Some(node) = self.get_node(node_id) else {
            return;
        };
        if !node.flags.is_in_document() {
            return;
        }
        self.start_iframe_load(node_id, url);
    }

    /// Apply fetched iframe HTML, discarding stale responses (the iframe may
    /// have been removed or re-navigated since the request was issued).
    pub(crate) fn apply_iframe_html(
        &mut self,
        node_id: NodeId,
        request_id: usize,
        resolved_url: Option<String>,
        html: &str,
    ) {
        let is_current = self
            .iframe_loads
            .get(&node_id)
            .is_some_and(|load| load.request_id == Some(request_id));
        if !is_current {
            return;
        }
        let load = self.iframe_loads.get_mut(&node_id).unwrap();
        load.request_id = None;
        let signal = load.abort_controller.signal.clone();

        let node_is_iframe = self
            .get_node(node_id)
            .and_then(|node| node.element_data())
            .is_some_and(|el| el.name.local == local_name!("iframe"));
        if !node_is_iframe {
            return;
        }

        self.attach_iframe_document(node_id, html, resolved_url, signal);
    }
}
