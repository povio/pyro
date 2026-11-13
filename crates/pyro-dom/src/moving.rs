//! HTML "moving steps" for the resource queue: what `moveBefore` tells the
//! loader. A move is not a removal followed by an insertion: iframes keep
//! their documents and scripts are not prepared again. Stylesheet owners
//! change their place in the cascade order, and images re-run "update the
//! image data" (a relevant mutation when a `<picture>` parent or a `<source>`
//! sibling changed).

use markup5ever::local_name;

use crate::dom::Dom;
use crate::node::NIL;
use crate::script::{ResourceKind, ResourceReason};

impl Dom {
    fn is_html_named(&self, idx: u32, local: &markup5ever::LocalName) -> bool {
        idx != NIL && self.n(idx).element().is_some_and(|e| e.name.local == *local && e.is_html())
    }

    /// The elements of `root`'s subtree (shadow trees included) that are
    /// resources of the kinds a move concerns.
    fn moved_resources(&self, root: u32) -> Vec<(u32, ResourceKind)> {
        self.shadow_including_subtree(root)
            .into_iter()
            .filter_map(|i| match self.resource_kind(i)? {
                k @ (ResourceKind::Stylesheet | ResourceKind::StyleText | ResourceKind::Img) => Some((i, k)),
                _ => None,
            })
            .collect()
    }

    /// A `<source>` (`moved`) came into or left `picture`: a relevant
    /// mutation for the `<img>` children of a `<picture>`.
    pub(crate) fn picture_images_changed(&mut self, picture: u32, moved: u32) {
        if !self.resources.enabled
            || !self.is_html_named(picture, &local_name!("picture"))
            || !self.is_html_named(moved, &local_name!("source"))
            || !self.is_connected_idx(picture)
        {
            return;
        }
        let images: Vec<u32> =
            self.child_idx_iter(picture).filter(|&c| self.is_html_named(c, &local_name!("img"))).collect();
        for img in images {
            self.push_resource(img, ResourceKind::Img, ResourceReason::AttrChanged);
        }
    }

    /// Before `node` is unlinked by a move.
    pub(crate) fn resources_before_move(&mut self, node: u32) {
        if !self.resources.enabled || !self.is_connected_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        for (i, kind) in self.moved_resources(node) {
            if kind != ResourceKind::Img {
                self.push_resource(i, kind, ResourceReason::Removed);
            }
        }
    }

    /// After `node` was linked at its new place by a move from `old_parent`.
    pub(crate) fn resources_after_move(&mut self, node: u32, old_parent: u32) {
        if !self.resources.enabled || !self.is_connected_idx(node) {
            return;
        }
        let parent = self.n(node).parent;
        if parent != NIL && self.resource_kind(parent) == Some(ResourceKind::StyleText) {
            self.push_resource(parent, ResourceKind::StyleText, ResourceReason::ChildrenChanged);
        }
        for (i, kind) in self.moved_resources(node) {
            self.push_resource(i, kind, ResourceReason::Connected);
        }
        self.picture_images_changed(old_parent, node);
        self.picture_images_changed(parent, node);
    }
}
