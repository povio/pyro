//! Native node identity and automation geometry. Backend IDs do not retain DOM
//! nodes or old documents. Frontend mirror/mutation tracking remains unsupported.
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use lp_dom::{NodeId, NodeType};
use lp_webapi::{node::Node, PageDom};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{remote, runtime::context_id, EngineHost};
use crate::msg::{CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

const MAX_BACKEND_NODES: usize = 4096;
const MAX_DESCRIPTION_NODES: usize = 1024;

struct BackendNode {
    document: Weak<PageDom>,
    node: NodeId,
}

#[derive(Default)]
pub(super) struct BackendNodes {
    ids: HashMap<u64, BackendNode>,
    reverse: HashMap<(usize, NodeId), u64>,
}

impl BackendNodes {
    fn identify(&mut self, page: &Rc<PageDom>, node: NodeId, next: &mut u64) -> CdpResult<u64> {
        let key = (Rc::as_ptr(page) as usize, node);
        if let Some(id) = self.reverse.get(&key) {
            if self.ids.get(id).is_some_and(|entry| entry.document.upgrade().is_some_and(|old| Rc::ptr_eq(&old, page)))
            {
                return Ok(*id);
            }
        }
        if self.ids.len() >= MAX_BACKEND_NODES {
            self.ids.retain(|_, entry| {
                entry
                    .document
                    .upgrade()
                    .is_some_and(|document| !document.is_disposed() && document.dom.borrow().node(entry.node).is_ok())
            });
            self.reverse.retain(|_, id| self.ids.contains_key(id));
            if self.ids.len() >= MAX_BACKEND_NODES {
                return Err(CdpError::server("Too many backend node IDs"));
            }
        }
        if *next > i32::MAX as u64 {
            return Err(CdpError::server("Backend node ID space exhausted"));
        }
        let id = *next;
        *next += 1;
        self.ids.insert(id, BackendNode { document: Rc::downgrade(page), node });
        self.reverse.insert(key, id);
        Ok(id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeParams {
    node_id: Option<u64>,
    backend_node_id: Option<u64>,
    object_id: Option<String>,
}

fn stale() -> CdpError {
    CdpError::server("Could not find node with given id")
}
fn dom_error(error: lp_dom::DomError) -> CdpError {
    CdpError::server(format!("DOM error: {error:?}"))
}

impl EngineHost {
    fn resolve_dom_node(&mut self, cmd: &PageCommand<'_, '_>, p: &NodeParams) -> CdpResult<(Rc<PageDom>, NodeId)> {
        if usize::from(p.node_id.is_some())
            + usize::from(p.backend_node_id.is_some())
            + usize::from(p.object_id.is_some())
            != 1
        {
            return Err(CdpError::invalid_params("Specify exactly one nodeId, backendNodeId or objectId"));
        }
        if p.node_id.is_some() {
            return Err(CdpError::unsupported("DOM frontend node IDs"));
        }
        let (page, node) = if let Some(id) = p.backend_node_id {
            let state = self.pages.get(&cmd.page).ok_or_else(stale)?;
            let entry = state.dom_nodes.ids.get(&id).ok_or_else(stale)?;
            (entry.document.upgrade().ok_or_else(stale)?, entry.node)
        } else {
            let handles = self.sessions.get(cmd.session_id).ok_or_else(stale)?.handles.clone();
            let (context, object) = {
                let table = handles.borrow();
                let (context, object) = table.get(p.object_id.as_deref().expect("one selector"))?;
                (context, object.clone())
            };
            let realm = self.target_realm(cmd.page, Some(context))?;
            let scope = realm.scope().map_err(remote::js_to_cdp)?;
            let data =
                scope.unwrap::<Node>(object.get(&scope)).ok_or_else(|| CdpError::server("Object is not a DOM node"))?;
            (data.page().clone(), data.id)
        };
        // A surviving remote wrapper does not make a replaced document live.
        self.frame_for_document(cmd.page, &page).ok_or_else(stale)?;
        page.dom.borrow().node(node).map_err(dom_error)?;
        Ok((page, node))
    }

    fn describe_dom_node(
        &mut self,
        key: PageKey,
        page: &Rc<PageDom>,
        node: NodeId,
        depth: u32,
        remaining: &mut usize,
    ) -> CdpResult<Value> {
        if *remaining == 0 {
            return Err(CdpError::server("DOM description exceeds node budget"));
        }
        *remaining -= 1;
        let backend =
            self.pages.get_mut(&key).ok_or_else(stale)?.dom_nodes.identify(page, node, &mut self.next_backend_node)?;
        let (mut value, children, owns_frame) = {
            let dom = page.dom.borrow();
            let native = dom.node(node).map_err(dom_error)?;
            let ty = native.kind.node_type();
            let child_count = dom.children(node).map_err(dom_error)?.count();
            if depth > 0 && child_count > *remaining {
                return Err(CdpError::server("DOM description exceeds node budget"));
            }
            let children: Vec<_> =
                if depth > 0 { dom.children(node).map_err(dom_error)?.collect() } else { Vec::new() };
            let local = match &native.kind {
                lp_dom::NodeKind::Element(e) => e.name.local.to_string(),
                _ => String::new(),
            };
            let mut value = json!({"nodeId":0,"backendNodeId":backend,"nodeType":ty as u16,
                "nodeName":dom.node_name(node).map_err(dom_error)?,"localName":local,
                "nodeValue":native.kind.char_data().unwrap_or(""),"childNodeCount":child_count});
            if ty == NodeType::Element {
                let attrs: Vec<_> = dom
                    .attributes(node)
                    .map_err(dom_error)?
                    .iter()
                    .flat_map(|a| [a.qualified_name(), a.value.to_string()])
                    .collect();
                value["attributes"] = json!(attrs);
            }
            let owns_frame =
                node == page.document() || dom.document_element(page.document()).map_err(dom_error)? == Some(node);
            (value, children, owns_frame)
        };
        let state = self.pages.get(&key).ok_or_else(stale)?;
        let page_id = state.id;
        if owns_frame {
            value["frameId"] = json!(self.frame_for_document(key, page).ok_or_else(stale)?.id.0);
        }
        if let Some(child_id) = self.thread.browser.frame_of_owner(page_id, node) {
            value["frameId"] = json!(child_id.0);
            if depth > 0 {
                if let Some(child) = self.thread.browser.frame(page_id, &child_id).and_then(|f| f.page_dom) {
                    value["contentDocument"] =
                        self.describe_dom_node(key, &child, child.document(), depth - 1, remaining)?;
                }
            }
        }
        if depth > 0 {
            if children.len() > *remaining {
                return Err(CdpError::server("DOM description exceeds node budget"));
            }
            let mut described = Vec::with_capacity(children.len());
            for child in children {
                described.push(self.describe_dom_node(key, page, child, depth - 1, remaining)?);
            }
            value["children"] = json!(described);
        }
        Ok(value)
    }

    pub(super) fn dom_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" | "disable" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    include_whitespace: Option<String>,
                }
                let p: P = cmd.command.params()?;
                if p.include_whitespace.as_deref().is_some_and(|v| v != "none") {
                    return Err(CdpError::unsupported("DOM whitespace mirror tracking"));
                }
                self.session_mut(cmd.session_id)?.dom_enabled = action == "enable";
                return Ok(Some(json!({})));
            }
            "getFrameOwner" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    frame_id: String,
                }
                let p: P = cmd.command.params()?;
                let state = self.pages.get(&cmd.page).ok_or_else(stale)?;
                let frame = self.thread.browser.frame(state.id, &lp_browser::FrameId(p.frame_id)).ok_or_else(stale)?;
                let owner = frame.owner.ok_or_else(stale)?;
                let parent = self
                    .thread
                    .browser
                    .frame(state.id, frame.parent.as_ref().ok_or_else(stale)?)
                    .and_then(|f| f.page_dom)
                    .ok_or_else(stale)?;
                let backend = self.pages.get_mut(&cmd.page).ok_or_else(stale)?.dom_nodes.identify(
                    &parent,
                    owner,
                    &mut self.next_backend_node,
                )?;
                return Ok(Some(json!({"backendNodeId": backend})));
            }
            "describeNode" | "resolveNode" | "getBoxModel" | "getContentQuads" | "scrollIntoViewIfNeeded" | "focus" => {
            }
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        }
        let p: NodeParams = cmd.command.params()?;
        let (page, node) = self.resolve_dom_node(cmd, &p)?;
        let result = match action {
            "describeNode" => {
                #[derive(Deserialize)]
                struct P {
                    depth: Option<i32>,
                    #[serde(default)]
                    pierce: bool,
                }
                let p: P = cmd.command.params()?;
                if p.pierce {
                    return Err(CdpError::unsupported("DOM pierced subtree descriptions"));
                }
                let depth = p.depth.unwrap_or(1);
                if !(0..=32).contains(&depth) {
                    return Err(CdpError::invalid_params("DOM depth must be between 0 and 32"));
                }
                let mut remaining = MAX_DESCRIPTION_NODES;
                json!({"node":self.describe_dom_node(cmd.page,&page,node,depth as u32,&mut remaining)?})
            }
            "resolveNode" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    execution_context_id: Option<i64>,
                    object_group: Option<String>,
                }
                let p: P = cmd.command.params()?;
                let frame = self.frame_for_document(cmd.page, &page).ok_or_else(stale)?;
                let realm = if let Some(id) = p.execution_context_id {
                    let realm = self.target_realm(cmd.page, Some(id))?;
                    if !frame.realms.iter().any(|r| r.realm.id() == realm.id()) {
                        return Err(CdpError::server("Execution context belongs to a different frame"));
                    }
                    realm
                } else {
                    frame.main_realm().cloned().ok_or_else(stale)?
                };
                let scope = realm.scope().map_err(remote::js_to_cdp)?;
                let object = lp_webapi::wrap_node(&scope, &page, node).map_err(remote::js_to_cdp)?;
                let described = remote::describe(&scope, object.value(), false)?;
                let handles = self.sessions.get(cmd.session_id).ok_or_else(stale)?.handles.clone();
                json!({"object":remote::register(described,&handles,context_id(realm.id()),p.object_group.as_deref())?})
            }
            _ => {
                let frame = self.frame_for_document(cmd.page, &page).ok_or_else(stale)?;
                // Child frames report main-frame viewport coordinates, as Chrome
                // does for same-process iframes.
                let (ox, oy) = if action == "focus" { (0.0, 0.0) } else { self.frame_offset(cmd.page, &frame)? };
                let rect = {
                    let dom = page.dom.borrow();
                    if dom.root_node(node, true).map_err(dom_error)? != page.document() {
                        return Err(CdpError::server("Node is detached from document"));
                    }
                    if dom.node_type(node).map_err(dom_error)? != NodeType::Element {
                        return Err(CdpError::server("Node does not have a layout object"));
                    }
                    page.layout
                        .borrow()
                        .client_rect(&dom, node)
                        .ok_or_else(|| CdpError::server("Node does not have a layout object"))?
                };
                // In layout units, as Chromium reports untransformed geometry:
                // the frame's offset and the element's rect each quantized,
                // so the quad's edges give back the width exactly
                // (Playwright's boundingBox).
                let rect = rect.layout_units();
                let (ox, oy) = (lp_dom::layout::layout_unit(ox), lp_dom::layout::layout_unit(oy));
                let (rx, ry) = (rect.x + ox, rect.y + oy);
                let quad = [rx, ry, rx + rect.width, ry, rx + rect.width, ry + rect.height, rx, ry + rect.height];
                match action {
                    "getBoxModel" => {
                        json!({"model":{"content":quad,"padding":quad,"border":quad,"margin":quad,"width":rect.width.round() as i64,"height":rect.height.round() as i64}})
                    }
                    "getContentQuads" => json!({"quads":[quad]}),
                    "focus" => {
                        if !page.dom.borrow().is_focusable(node).map_err(dom_error)? {
                            return Err(CdpError::server("Node is not focusable"));
                        }
                        lp_webapi::input::focus_element(&page, node);
                        json!({})
                    }
                    "scrollIntoViewIfNeeded" => {
                        #[derive(Deserialize)]
                        struct Area {
                            x: f64,
                            y: f64,
                            width: f64,
                            height: f64,
                        }
                        #[derive(Deserialize)]
                        struct P {
                            rect: Option<Area>,
                        }
                        let rect = cmd.command.params::<P>()?.rect;
                        reveal(&page, node);
                        // `rect` (relative to the node's border box; Playwright sends it
                        // for positional clicks): after revealing the node, scroll the
                        // window by the least amount that brings the area into view.
                        if let Some(area) = rect {
                            reveal_area(&page, node, area.x, area.y, area.width, area.height);
                        }
                        // Then each owning iframe inside its parent document.
                        let page_id = self.pages.get(&cmd.page).ok_or_else(stale)?.id;
                        let mut cur = frame;
                        while let (Some(parent_id), Some(owner)) = (cur.parent.clone(), cur.owner) {
                            let Some(parent) = self.thread.browser.frame(page_id, &parent_id) else { break };
                            if let Some(parent_dom) = parent.page_dom.as_ref() {
                                reveal(parent_dom, owner);
                            }
                            cur = parent;
                        }
                        json!({})
                    }
                    _ => unreachable!(),
                }
            }
        };
        Ok(Some(result))
    }
}

/// `DOM.scrollIntoViewIfNeeded` for `node` in its document, queueing the scroll events.
pub(super) fn reveal(page: &Rc<PageDom>, node: NodeId) {
    let moved = page.layout.borrow_mut().scroll_into_view_if_needed_targets(&page.dom.borrow(), node);
    for target in moved {
        let node = match target {
            lp_dom::layout::ScrollTarget::Container(node) => Some(node),
            lp_dom::layout::ScrollTarget::Viewport => None,
        };
        lp_webapi::layout_api::fire_scroll(page, node);
    }
}

/// Tries center alignment without bypassing the driver's subsequent hit test.
pub(super) fn reveal_centered(page: &Rc<PageDom>, node: NodeId) -> bool {
    let moved = page.layout.borrow_mut().scroll_into_view_centered_targets(&page.dom.borrow(), node);
    let changed = !moved.is_empty();
    for target in moved {
        let node = match target {
            lp_dom::layout::ScrollTarget::Container(node) => Some(node),
            lp_dom::layout::ScrollTarget::Viewport => None,
        };
        lp_webapi::layout_api::fire_scroll(page, node);
    }
    changed
}

/// Scrolls the window so the node-relative area is visible, as little as
/// possible (nearest edge), queueing a scroll event when it moved.
fn reveal_area(page: &Rc<PageDom>, node: NodeId, x: f64, y: f64, width: f64, height: f64) {
    let dom = page.dom.borrow();
    let mut layout = page.layout.borrow_mut();
    let origin = layout.bounding_client_rect(&dom, node);
    let (left, top) = (origin.left() + x, origin.top() + y);
    let nearest = |start: f64, size: f64, viewport: f64, scroll: u32| -> Option<i32> {
        let end = start + size.max(0.0);
        let delta = if start < 0.0 {
            start
        } else if end > viewport {
            (end - viewport).min(start)
        } else {
            return None;
        };
        Some((scroll as f64 + delta).max(0.0).trunc().min(i32::MAX as f64) as i32)
    };
    let dx = nearest(left, width, layout.inner_width() as f64, layout.scroll_x());
    let dy = nearest(top, height, layout.inner_height() as f64, layout.scroll_y());
    if (dx.is_some() || dy.is_some()) && layout.window_scroll_to(&dom, dx, dy) {
        drop(layout);
        drop(dom);
        lp_webapi::layout_api::fire_scroll(page, None);
    }
}

impl EngineHost {
    /// The viewport origin of `frame`'s document in main-frame viewport
    /// coordinates: the sum of its owning iframes' client rects.
    pub(super) fn frame_offset(&self, page: PageKey, frame: &lp_browser::Frame) -> CdpResult<(f64, f64)> {
        let page_id = self.pages.get(&page).ok_or_else(stale)?.id;
        let (mut x, mut y) = (0.0, 0.0);
        let mut cur = frame.clone();
        while let (Some(parent_id), Some(owner)) = (cur.parent.clone(), cur.owner) {
            let parent = self.thread.browser.frame(page_id, &parent_id).ok_or_else(stale)?;
            let dom = parent.page_dom.clone().ok_or_else(stale)?;
            let r = dom
                .layout
                .borrow()
                .client_rect(&dom.dom.borrow(), owner)
                .ok_or_else(|| CdpError::server("Frame owner does not have a layout object"))?;
            // Each frame's offset in layout units (Chromium sums them).
            x += lp_dom::layout::layout_unit(r.x);
            y += lp_dom::layout::layout_unit(r.y);
            cur = parent;
        }
        Ok((x, y))
    }

    /// Page.fileChooserOpened to the sessions intercepting file choosers, with
    /// the input's backend node id (resolved by Playwright via DOM.resolveNode).
    pub(super) fn file_chooser_opened(
        &mut self,
        page: PageKey,
        frame_id: &lp_browser::FrameId,
        input: NodeId,
        multiple: bool,
    ) {
        let sessions = self.sessions_of(page, |s| s.intercept_file_chooser);
        if sessions.is_empty() {
            return;
        }
        let Some(state) = self.pages.get(&page) else { return };
        let Some(document) = self.thread.browser.frame(state.id, frame_id).and_then(|f| f.page_dom) else { return };
        let Some(state) = self.pages.get_mut(&page) else { return };
        let Ok(backend) = state.dom_nodes.identify(&document, input, &mut self.next_backend_node) else { return };
        let mode = if multiple { "selectMultiple" } else { "selectSingle" };
        for s in &sessions {
            self.event(
                "Page.fileChooserOpened",
                &serde_json::json!({ "frameId": frame_id.0, "mode": mode, "backendNodeId": backend }),
                s,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn backend_table_is_bounded_nonretaining_and_generation_checked() {
        let dom = Rc::new(RefCell::new(lp_dom::Dom::new()));
        let doc = dom.borrow_mut().create_document(lp_dom::DocumentKind::Html, "about:blank");
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let page = PageDom::new(dom.clone(), doc);
        let mut table = BackendNodes::default();
        let mut next = 1;
        let first = dom.borrow_mut().create_element(doc, "div").unwrap();
        let first_id = table.identify(&page, first, &mut next).unwrap();
        for _ in 1..MAX_BACKEND_NODES {
            let node = dom.borrow_mut().create_element(doc, "div").unwrap();
            table.identify(&page, node, &mut next).unwrap();
        }
        let extra = dom.borrow_mut().create_element(doc, "div").unwrap();
        assert!(table.identify(&page, extra, &mut next).is_err());
        assert_eq!(Rc::strong_count(&page), 1, "backend IDs do not retain binding documents");
        dom.borrow_mut().collect_garbage();
        assert!(dom.borrow().node(first).is_err());
        let replacement = dom.borrow_mut().create_element(doc, "span").unwrap();
        let replacement_id = table.identify(&page, replacement, &mut next).unwrap();
        assert_ne!(replacement_id, first_id);
        assert!(!table.ids.contains_key(&first_id), "full-table sweep discards dead generations");
        drop(page);
        assert!(table.ids.values().all(|entry| entry.document.upgrade().is_none()));
    }
}
