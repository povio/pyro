//! SVG `<use>` element instances (SVG 2 §5.6 "The use element"): a
//! connected `<use>` gets a closed user-agent shadow root holding a deep
//! clone of the element its `href` (or `xlink:href`) names in its own tree.
//! The clones are real elements: they load what they reference and fire
//! events (an `<image>` clone's `error`), with the `<use>` as the target seen
//! from the light tree; script cannot reach the shadow root itself.
//!
//! The instance follows its referenced element: a change of the `<use>`
//! (insertion, `href`) updates it at once (the loader's `SvgUse` resource
//! event, [`update`]); a change inside the referenced subtree rebuilds it
//! after the task that made it ([`after_task`], which compares a fingerprint
//! of the subtree). Updates are made in place where the nodes still match,
//! so an instance's elements keep their identity, style and transitions.
//!
//! Rendering is the SVG renderer's, which resolves `<use>` from the light
//! tree itself; these instances are for the DOM, events and loads.
//!
//! Limits: references to other documents (`file.svg#id`) make no instance;
//! a reference to an ancestor of the `<use>` (a cycle) makes none; instances
//! nest at most [`MAX_DEPTH`] deep; `<symbol>` is cloned as it is (not
//! turned into an `<svg>`).
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use pyro_dom::NodeId;

use crate::page::PageDom;

/// `<use>` inside instances inside instances...: past this depth no instance.
pub const MAX_DEPTH: usize = 8;
/// Instances one document keeps live.
const MAX_HOSTS: usize = 4096;

/// The element `host`'s `href` names in its tree, if any, and whether it
/// is a valid (non-cyclic) reference.
fn referenced(page: &PageDom, host: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    if !dom.is_connected(host).unwrap_or(false) {
        return None;
    }
    let href = dom
        .get_attribute(host, "href")
        .ok()
        .flatten()
        .or_else(|| dom.get_attribute_ns(host, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten())?
        .trim()
        .to_owned();
    let id = href.strip_prefix('#')?;
    if id.is_empty() {
        return None;
    }
    let root = dom.root_node(host, false).ok()?;
    let target = dom.get_element_by_id(root, id).ok().flatten()?;
    // A cycle: the reference contains the `<use>`.
    if target == host || dom.contains(target, host).unwrap_or(false) {
        return None;
    }
    // Nested instances: count the shadow roots above the host.
    let mut depth = 0;
    let mut node = host;
    loop {
        let r = dom.root_node(node, false).ok()?;
        match dom.fragment_host(r).ok().flatten() {
            Some(h) => {
                depth += 1;
                if depth >= MAX_DEPTH {
                    return None;
                }
                node = h;
            }
            None => break,
        }
    }
    Some(target)
}

/// A fingerprint of `target`'s subtree as markup: what an instance mirrors.
fn fingerprint(page: &PageDom, target: Option<NodeId>) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    if let Some(target) = target {
        target.hash(&mut hasher);
        page.dom.borrow().outer_html(target).unwrap_or_default().hash(&mut hasher);
    }
    hasher.finish()
}

/// Brings `dst` (an instance node) in line with `src` (its original) in
/// place: the same kind of node keeps its identity (its style, running
/// transitions, loads) and takes `src`'s attributes, data and children;
/// anything else is replaced by a new clone.
fn sync(dom: &mut pyro_dom::Dom, src: NodeId, dst: NodeId) -> Option<()> {
    let same = dom.node_type(src).ok()? == dom.node_type(dst).ok()?
        && match dom.node_type(src).ok()? {
            pyro_dom::NodeType::Element => dom.element(src).ok()?.name == dom.element(dst).ok()?.name,
            _ => true,
        };
    if !same {
        let parent = dom.parent(dst).ok()??;
        let clone = dom.clone_node(src, true, None).ok()?;
        dom.replace_child(parent, clone, dst).ok()?;
        return Some(());
    }
    match dom.node_type(src).ok()? {
        pyro_dom::NodeType::Element => {
            let want: Vec<(Option<String>, String, String)> = dom
                .attributes(src)
                .ok()?
                .iter()
                .map(|a| {
                    let ns = (!a.name.ns.is_empty()).then(|| a.name.ns.to_string());
                    (ns, a.qualified_name(), a.value.to_string())
                })
                .collect();
            let have: Vec<String> = dom.attributes(dst).ok()?.iter().map(|a| a.qualified_name()).collect();
            for name in have {
                if !want.iter().any(|(_, n, _)| *n == name) {
                    let _ = dom.remove_attribute(dst, &name);
                }
            }
            for (ns, name, value) in want {
                let current = dom.attributes(dst).ok()?.iter().find(|a| a.qualified_name() == name).map(|a| a.value.to_string());
                if current.as_deref() != Some(value.as_str()) {
                    let _ = dom.set_attribute_ns(dst, ns.as_deref(), &name, &value);
                }
            }
        }
        pyro_dom::NodeType::Text | pyro_dom::NodeType::Comment => {
            let data = dom.char_data(src).ok()?.to_owned();
            let len = dom.char_data(dst).ok()?.encode_utf16().count() as u32;
            if dom.char_data(dst).ok()? != data {
                let _ = dom.replace_data(dst, 0, len, &data);
            }
            return Some(());
        }
        _ => return Some(()),
    }
    sync_children(dom, src, dst)
}

fn sync_children(dom: &mut pyro_dom::Dom, src: NodeId, dst: NodeId) -> Option<()> {
    let from: Vec<NodeId> = dom.children(src).ok()?.collect();
    let to: Vec<NodeId> = dom.children(dst).ok()?.collect();
    for (i, child) in from.iter().enumerate() {
        match to.get(i) {
            Some(&existing) => {
                sync(dom, *child, existing);
            }
            None => {
                let clone = dom.clone_node(*child, true, None).ok()?;
                dom.append_child(dst, clone).ok()?;
            }
        }
    }
    for extra in to.iter().skip(from.len()) {
        let _ = dom.remove_child(dst, *extra);
    }
    Some(())
}

/// Brings `host`'s instance in line with what it references now (its
/// shadow root's only child mirrors `target`).
fn rebuild(page: &PageDom, host: NodeId, target: Option<NodeId>) {
    let mut dom = page.dom.borrow_mut();
    let existing = dom.shadow_root(host).ok().flatten();
    let root = match (existing, target) {
        (Some(root), _) => root,
        (None, None) => return,
        (None, Some(_)) => match dom.ua_shadow_root(host) {
            Ok(root) => root,
            Err(_) => return,
        },
    };
    match target {
        None => {
            let _ = dom.replace_all(root, None);
        }
        Some(target) => {
            let current: Vec<NodeId> = dom.children(root).map(|c| c.collect()).unwrap_or_default();
            match current.as_slice() {
                [only] => {
                    sync(&mut dom, target, *only);
                }
                _ => {
                    let _ = dom.replace_all(root, None);
                    if let Ok(clone) = dom.clone_node(target, true, None) {
                        let _ = dom.append_child(root, clone);
                    }
                }
            }
        }
    }
}

/// The loader saw `host` (an SVG `<use>`) connected, removed or changed.
pub fn update(page: &Rc<PageDom>, host: NodeId) {
    let target = referenced(page, host);
    let print = fingerprint(page, target);
    {
        let mut hosts = page.use_hosts.borrow_mut();
        hosts.retain(|(h, _)| *h != host);
        if target.is_some() && hosts.len() < MAX_HOSTS {
            hosts.push((host, print));
        }
    }
    rebuild(page, host, target);
    page.flush_iframe_events();
}

/// After a task that changed the document: instances whose referenced
/// subtree changed are made again.
pub(crate) fn after_task(page: &PageDom) {
    if page.use_hosts.borrow().is_empty() {
        return;
    }
    let hosts: Vec<(NodeId, u64)> = page.use_hosts.borrow().clone();
    let mut changed = Vec::new();
    for (host, old) in hosts {
        let target = referenced(page, host);
        let print = fingerprint(page, target);
        if print != old || target.is_none() {
            changed.push((host, target, print));
        }
    }
    for (host, target, print) in changed {
        {
            let mut hosts = page.use_hosts.borrow_mut();
            hosts.retain(|(h, _)| *h != host);
            if target.is_some() {
                hosts.push((host, print));
            }
        }
        rebuild(page, host, target);
    }
}
