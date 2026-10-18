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
                if self.frame_for_document(cmd.page, &page).ok_or_else(stale)?.parent.is_some() {
                    return Err(CdpError::unsupported("DOM geometry/focus/scroll in a child frame"));
                }
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
                let quad = [
                    rect.x,
                    rect.y,
                    rect.x + rect.width,
                    rect.y,
                    rect.x + rect.width,
                    rect.y + rect.height,
                    rect.x,
                    rect.y + rect.height,
                ];
                match action {
                    "getBoxModel" => {
                        json!({"model":{"content":quad,"padding":quad,"border":quad,"margin":quad,"width":rect.width.round() as i64,"height":rect.height.round() as i64}})
                    }
                    "getContentQuads" => json!({"quads":[quad]}),
                    "focus" => {
                        let native = self.pages.get(&cmd.page).ok_or_else(stale)?.id;
                        if self.thread.browser.frame_of_owner(native, node).is_some() {
                            return Err(CdpError::unsupported("DOM.focus of a child frame owner"));
                        }
                        if !page.dom.borrow().is_focusable(node).map_err(dom_error)? {
                            return Err(CdpError::server("Node is not focusable"));
                        }
                        lp_webapi::input::focus_element(&page, node);
                        json!({})
                    }
                    "scrollIntoViewIfNeeded" => {
                        #[derive(Deserialize)]
                        struct P {
                            rect: Option<Value>,
                        }
                        if cmd.command.params::<P>()?.rect.is_some() {
                            return Err(CdpError::unsupported("DOM scroll rectangle override"));
                        }
                        let moved = page.layout.borrow_mut().scroll_into_view(&page.dom.borrow(), node);
                        if moved {
                            lp_webapi::input::queue_simple(&page, page.document(), "scroll", true, false);
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
