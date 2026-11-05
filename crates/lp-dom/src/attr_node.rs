//! `Attr` nodes (https://dom.spec.whatwg.org/#interface-attr): an attribute
//! seen as a node. An element's attribute list stays the plain `Vec<Attr>`;
//! a node is made for an attribute the first time script asks for one
//! (`getAttributeNode`, `attributes[i]`) and is found again through
//! `Dom::attr_nodes`. Such a node has no parent and no children: its root is
//! itself, its length 0.

use markup5ever::QualName;

use crate::dom::Dom;
use crate::error::{DomError, DomErrorKind, DomResult};
use crate::node::{AttrNodeData, NodeId, NodeKind, NIL};

fn not_an_attribute() -> DomError {
    DomError::Type("not an attribute")
}

fn same_name(a: &QualName, b: &QualName) -> bool {
    a.ns == b.ns && a.local == b.local
}

impl Dom {
    pub(crate) fn attr_data(&self, i: u32) -> Option<&AttrNodeData> {
        match &self.n(i).kind {
            NodeKind::Attr(a) => Some(a),
            _ => None,
        }
    }
    fn attr_data_mut(&mut self, i: u32) -> Option<&mut AttrNodeData> {
        match &mut self.n_mut(i).kind {
            NodeKind::Attr(a) => Some(a),
            _ => None,
        }
    }

    /// The Attr nodes made so far for attributes of element `el`.
    pub(crate) fn attr_nodes_of(&self, el: u32) -> &[u32] {
        if self.attr_nodes.is_empty() {
            return &[];
        }
        self.attr_nodes.get(&el).map_or(&[], |v| v.as_slice())
    }

    /// `createAttribute` / `createAttributeNS`: an Attr without an element.
    pub fn create_attribute(&mut self, doc: NodeId, name: QualName) -> DomResult<NodeId> {
        let d = self.check(doc)?;
        if !matches!(self.n(d).kind, NodeKind::Document(_)) {
            return Err(DomError::Type("not a document"));
        }
        let i = self.alloc_raw(NodeKind::Attr(Box::new(AttrNodeData { name, value: String::new(), owner: NIL })), d);
        self.release_candidate(i);
        Ok(self.id_of(i))
    }

    /// The Attr node of `el`'s attribute `(ns, local)`: the same node until the
    /// attribute leaves the element. `None` when there is no such attribute.
    pub fn attribute_node(&mut self, el: NodeId, ns: Option<&str>, local: &str) -> DomResult<Option<NodeId>> {
        let e = self.check(el)?;
        let ns = ns.unwrap_or("");
        let attrs = &self.n(e).element().ok_or(DomError::Type("not an element"))?.attrs;
        let Some(k) = attrs.iter().position(|a| &*a.name.ns == ns && &*a.name.local == local) else {
            return Ok(None);
        };
        let i = self.attribute_node_at(e, k);
        Ok(Some(self.id_of(i)))
    }

    /// Attr node of the `k`th attribute of element `e`.
    fn attribute_node_at(&mut self, e: u32, k: usize) -> u32 {
        let name = self.n(e).element().expect("element").attrs[k].name.clone();
        let known = self.attr_nodes_of(e).iter().copied().find(|&i| self.attr_data(i).is_some_and(|a| same_name(&a.name, &name)));
        if let Some(i) = known {
            return i;
        }
        let doc = self.n(e).owner_doc;
        let i = self.alloc_raw(NodeKind::Attr(Box::new(AttrNodeData { name, value: String::new(), owner: e })), doc);
        self.attr_nodes.entry(e).or_default().push(i);
        // Dropped again once no wrapper, range or record refers to it.
        self.release_candidate(i);
        i
    }

    /// Name and state of an Attr node.
    pub fn attr_node_data(&self, attr: NodeId) -> DomResult<&AttrNodeData> {
        let i = self.check(attr)?;
        self.attr_data(i).ok_or_else(not_an_attribute)
    }

    /// `Attr.ownerElement`.
    pub fn attr_owner_element(&self, attr: NodeId) -> DomResult<Option<NodeId>> {
        Ok(self.opt_id(self.attr_node_data(attr)?.owner))
    }

    /// `Attr.value`.
    pub fn attr_node_value(&self, attr: NodeId) -> DomResult<&str> {
        let i = self.check(attr)?;
        self.attr_value_idx(i).ok_or_else(not_an_attribute)
    }

    pub(crate) fn attr_value_idx(&self, i: u32) -> Option<&str> {
        let a = self.attr_data(i)?;
        if a.owner == NIL {
            return Some(&a.value);
        }
        let attrs = &self.n(a.owner).element()?.attrs;
        Some(attrs.iter().find(|x| same_name(&x.name, &a.name)).map_or("", |x| &*x.value))
    }

    /// "Set an existing attribute value": the `Attr.value`, `nodeValue` and
    /// `textContent` setters.
    pub fn set_attr_node_value(&mut self, attr: NodeId, value: &str) -> DomResult<()> {
        let i = self.check(attr)?;
        let a = self.attr_data(i).ok_or_else(not_an_attribute)?;
        let owner = a.owner;
        if owner != NIL {
            let name = a.name.clone();
            let k = self.n(owner).element().and_then(|e| e.attrs.iter().position(|x| same_name(&x.name, &name)));
            if let Some(k) = k {
                self.change_attr(owner, k, value);
                return Ok(());
            }
        }
        if let Some(a) = self.attr_data_mut(i) {
            a.value = value.to_owned();
        }
        Ok(())
    }

    /// "Set an attribute" (`setAttributeNode[NS]`, `setNamedItem[NS]`): returns
    /// the Attr this one replaced, itself when it already is `el`'s, else None.
    pub fn set_attribute_node(&mut self, el: NodeId, attr: NodeId) -> DomResult<Option<NodeId>> {
        let e = self.check(el)?;
        if !self.n(e).is_element() {
            return Err(DomError::Type("not an element"));
        }
        let a = self.check(attr)?;
        let data = self.attr_data(a).ok_or_else(not_an_attribute)?;
        if data.owner == e {
            return Ok(Some(attr));
        }
        if data.owner != NIL {
            return Err(DomError::new(DomErrorKind::InUseAttribute, "the attribute is in use by another element"));
        }
        let name = data.name.clone();
        let value = data.value.clone();
        let existing = self.n(e).element().and_then(|x| x.attrs.iter().position(|x| same_name(&x.name, &name)));
        let doc = self.n(e).owner_doc;
        match existing {
            Some(k) => {
                // The replaced attribute's node leaves with its last value...
                let old = self.attribute_node_at(e, k);
                let el_data = self.n_mut(e).element_mut().expect("element");
                let old_value = el_data.attrs[k].value.to_string();
                // ...and the new one takes its place in the list, prefix included.
                el_data.attrs[k].name = name;
                self.unregister_attr_node(e, old, old_value);
                self.register_attr_node(e, a, doc);
                self.change_attr(e, k, &value);
                Ok(Some(self.id_of(old)))
            }
            None => {
                self.register_attr_node(e, a, doc);
                self.append_attr(e, name, &value);
                Ok(None)
            }
        }
    }

    /// `removeAttributeNode(attr)`: NotFoundError unless `attr` is `el`'s.
    pub fn remove_attribute_node(&mut self, el: NodeId, attr: NodeId) -> DomResult<()> {
        let e = self.check(el)?;
        let a = self.check(attr)?;
        let data = self.attr_data(a).ok_or_else(not_an_attribute)?;
        if data.owner != e {
            return Err(DomError::new(DomErrorKind::NotFound, "the attribute is not an attribute of this element"));
        }
        let name = data.name.clone();
        let ns = name.ns.to_string();
        self.remove_attribute_ns(el, Some(&ns), &name.local)?;
        Ok(())
    }

    fn register_attr_node(&mut self, e: u32, a: u32, doc: u32) {
        self.adopt_raw(a, doc);
        if let Some(d) = self.attr_data_mut(a) {
            d.owner = e;
            d.value.clear();
        }
        self.attr_nodes.entry(e).or_default().push(a);
    }

    fn unregister_attr_node(&mut self, e: u32, a: u32, value: String) {
        if let Some(d) = self.attr_data_mut(a) {
            d.owner = NIL;
            d.value = value;
        }
        if let Some(list) = self.attr_nodes.get_mut(&e) {
            list.retain(|&x| x != a);
            if list.is_empty() {
                self.attr_nodes.remove(&e);
            }
        }
        // On its own now: collectable unless something refers to it.
        self.release_candidate(a);
    }

    /// Attribute `name` (last value `value`) was removed from element `e`:
    /// its Attr node, if one exists, keeps the value and loses its element.
    pub(crate) fn attr_removed(&mut self, e: u32, name: &QualName, value: &str) {
        let node = self.attr_nodes_of(e).iter().copied().find(|&i| self.attr_data(i).is_some_and(|a| same_name(&a.name, name)));
        if let Some(a) = node {
            self.unregister_attr_node(e, a, value.to_owned());
        }
    }

    /// Slot `i` is being freed.
    pub(crate) fn forget_attr_nodes(&mut self, i: u32) {
        if self.attr_nodes.is_empty() {
            return;
        }
        match &self.n(i).kind {
            NodeKind::Element(_) => {
                if let Some(nodes) = self.attr_nodes.remove(&i) {
                    // Normally freed in the same sweep; never left pointing at a dead slot.
                    for a in nodes {
                        if let Some(d) = self.attr_data_mut(a) {
                            d.owner = NIL;
                        }
                    }
                }
            }
            NodeKind::Attr(a) if a.owner != NIL => {
                let owner = a.owner;
                if let Some(list) = self.attr_nodes.get_mut(&owner) {
                    list.retain(|&x| x != i);
                    if list.is_empty() {
                        self.attr_nodes.remove(&owner);
                    }
                }
            }
            _ => {}
        }
    }

    /// An Attr node on an element that nothing refers to any more is made
    /// again on demand, so it need not outlive its last reference.
    /// Returns its element.
    pub(crate) fn unreferenced_attached_attr(&self, i: u32) -> Option<u32> {
        let owner = self.attr_data(i).map(|a| a.owner).filter(|&o| o != NIL)?;
        (self.n(i).ext_refs == 0 && !self.mutation.pins(i)).then_some(owner)
    }

    /// DOM "equals" (`Node.isEqualNode`).
    pub fn is_equal_node(&self, a: NodeId, b: NodeId) -> DomResult<bool> {
        let mut stack = vec![(self.check(a)?, self.check(b)?)];
        while let Some((x, y)) = stack.pop() {
            if x == y {
                continue;
            }
            let (nx, ny) = (self.n(x), self.n(y));
            let same = match (&nx.kind, &ny.kind) {
                (NodeKind::Doctype(p), NodeKind::Doctype(q)) => {
                    p.name == q.name && p.public_id == q.public_id && p.system_id == q.system_id
                }
                (NodeKind::Element(p), NodeKind::Element(q)) => {
                    p.name.ns == q.name.ns
                        && p.name.prefix.as_ref().filter(|v| !v.is_empty())
                            == q.name.prefix.as_ref().filter(|v| !v.is_empty())
                        && p.name.local == q.name.local
                        && p.attrs.len() == q.attrs.len()
                        && p.attrs.iter().all(|pa| {
                            q.attrs.iter().any(|qa| same_name(&pa.name, &qa.name) && pa.value == qa.value)
                        })
                }
                (NodeKind::Attr(p), NodeKind::Attr(q)) => {
                    same_name(&p.name, &q.name) && self.attr_value_idx(x) == self.attr_value_idx(y)
                }
                (NodeKind::ProcessingInstruction(p), NodeKind::ProcessingInstruction(q)) => {
                    p.target == q.target && p.data == q.data
                }
                (NodeKind::Text(p), NodeKind::Text(q))
                | (NodeKind::CData(p), NodeKind::CData(q))
                | (NodeKind::Comment(p), NodeKind::Comment(q)) => p == q,
                (NodeKind::Document(_), NodeKind::Document(_)) | (NodeKind::Fragment(_), NodeKind::Fragment(_)) => true,
                _ => false,
            };
            if !same || nx.child_count != ny.child_count {
                return Ok(false);
            }
            stack.extend(self.child_idx_iter(x).zip(self.child_idx_iter(y)));
        }
        Ok(true)
    }
}
