use super::*;

/// Inclusive edges: a zero-area intersection can still be intersecting.
fn clip(rect: Rect, bounds: Rect, x: bool, y: bool) -> Option<Rect> {
    let left = if x { rect.left().max(bounds.left()) } else { rect.left() };
    let right = if x { rect.right().min(bounds.right()) } else { rect.right() };
    let top = if y { rect.top().max(bounds.top()) } else { rect.top() };
    let bottom = if y { rect.bottom().min(bounds.bottom()) } else { rect.bottom() };
    // Keep an uncut dimension rather than subtracting its translated endpoints:
    // cancellation can otherwise turn full containment into a ratio just below 1.
    let width = if left == rect.left() && right == rect.right() { rect.width } else { right - left };
    let height = if top == rect.top() && bottom == rect.bottom() { rect.height } else { bottom - top };
    (left <= right && top <= bottom).then_some(Rect { x: left, y: top, width, height })
}
/// Clips `rect` (client coordinates of `node`'s document) by the overflow
/// clips of `node`'s ancestors below `root` (HTML "compute the intersection",
/// step 3).
fn clip_by_ancestors(dom: &pyro_dom::Dom, layout: &pyro_dom::layout::Layout, node: NodeId, root: Option<NodeId>, rect: Rect) -> Option<Rect> {
    let mut intersection = Some(rect);
    let mut ancestor = dom.parent_element(node).ok().flatten();
    while let (Some(node), Some(rect)) = (ancestor, intersection) {
        if Some(node) == root {
            break;
        }
        // Root containers propagate scrolling to the viewport in this layout.
        if !layout.is_root_container(dom, node) {
            let axes = layout.style().overflow_clip_axes(dom, node);
            if axes.x || axes.y {
                intersection = clip(rect, layout.bounding_client_rect(dom, node), axes.x, axes.y);
            }
        }
        ancestor = dom.parent_element(node).ok().flatten();
    }
    intersection
}

fn viewport(layout: &pyro_dom::layout::Layout) -> Rect {
    Rect { x: 0.0, y: 0.0, width: layout.inner_width() as f64, height: layout.inner_height() as f64 }
}

fn geometry(bounds: Rect, target_rect: Rect, intersection: Option<Rect>) -> Geometry {
    let intersecting = intersection.is_some();
    let rect = intersection.unwrap_or_default();
    let area = target_rect.width * target_rect.height;
    let ratio = if area > 0.0 {
        (rect.width * rect.height / area).clamp(0.0, 1.0)
    } else if intersecting {
        1.0
    } else {
        0.0
    };
    Geometry { bounds, target: target_rect, intersection: rect, intersecting, ratio }
}

/// The observation of `target` (of `target_page`'s document) by an observer
/// of `page`'s document.
///
/// With an explicit root, only a target of the root's document can
/// intersect. The implicit root is the top-level document's viewport: a
/// target in a child frame's document is clipped by its own viewport, then
/// by each frame's owner element (its content box and the owner's clipping
/// ancestors) on the way up, and the intersection is reported in the
/// target document's coordinates (IntersectionObserver "compute the
/// intersection"). A document no longer shown in a page intersects nothing.
pub(super) fn measure(page: &PageDom, options: &options::Options, target_page: &PageDom, target: NodeId) -> Geometry {
    let dom = target_page.dom.borrow();
    let layout = target_page.layout();
    let connected = dom.is_connected(target).unwrap_or(false);
    let target_rect = if connected { layout.bounding_client_rect(&dom, target) } else { Rect::default() };
    let visible = connected && layout.is_visible(&dom, target);
    if let Some(root) = options.root.as_ref().map(|r| r.id) {
        let same_document = target_page.id() == page.id() || dom.node_document(root).ok() == dom.node_document(target).ok();
        // A Document root is its viewport.
        let root_is_document = dom.node_type(root).ok() == Some(pyro_dom::NodeType::Document);
        let root_rect = match (same_document, root_is_document) {
            (true, true) => viewport(&layout),
            (true, false) => layout.bounding_client_rect(&dom, root),
            (false, true) => viewport(&page.layout()),
            (false, false) => {
                let pdom = page.dom.borrow();
                page.layout().bounding_client_rect(&pdom, root)
            }
        };
        let bounds = options::expand(root_rect, &options.margin);
        let valid = same_document
            && visible
            && root != target
            && dom.contains(root, target).unwrap_or(false)
            && (root_is_document || (dom.is_connected(root).unwrap_or(false) && layout.is_visible(&dom, root)));
        let intersection = valid
            .then(|| clip_by_ancestors(&dom, &layout, target, Some(root), target_rect))
            .flatten()
            .and_then(|rect| clip(rect, bounds, true, true));
        return geometry(bounds, target_rect, intersection);
    }
    let document = target_page.document();
    let in_document = dom.node_document(target).ok() == Some(document);
    let mut intersection = (visible && in_document)
        .then(|| clip_by_ancestors(&dom, &layout, target, None, target_rect))
        .flatten();
    // Up through the frames, in the coordinates of each document in turn.
    let (mut dx, mut dy) = (0.0, 0.0);
    let mut container = target_page.container();
    let mut current_viewport = viewport(&layout);
    drop(layout);
    drop(dom);
    let mut holds: Vec<Rc<PageDom>> = Vec::new();
    let bounds = loop {
        match container {
            crate::page::FrameContainer::Top => {
                let bounds = options::expand(current_viewport, &options.margin);
                intersection = intersection.and_then(|rect| clip(rect, bounds, true, true));
                break bounds;
            }
            crate::page::FrameContainer::Detached => {
                intersection = None;
                break Rect::default();
            }
            crate::page::FrameContainer::Frame(parent, owner) => {
                intersection = intersection.and_then(|rect| clip(rect, current_viewport, true, true));
                let pdom = parent.dom.borrow();
                let playout = parent.layout();
                let owner_rect = playout.bounding_client_rect(&pdom, owner);
                let (ox, oy) = playout.content_offset(&pdom, owner).unwrap_or((0.0, 0.0));
                let (w, h) = playout.content_size(&pdom, owner).unwrap_or((0.0, 0.0));
                let (x, y) = (owner_rect.x + ox, owner_rect.y + oy);
                dx += x;
                dy += y;
                let content = Rect { x, y, width: w, height: h };
                intersection = intersection
                    .map(|r| Rect { x: r.x + x, y: r.y + y, ..r })
                    .and_then(|r| clip(r, content, true, true))
                    .and_then(|r| clip_by_ancestors(&pdom, &playout, owner, None, r));
                if !playout.is_visible(&pdom, owner) || !pdom.is_connected(owner).unwrap_or(false) {
                    intersection = None;
                }
                current_viewport = viewport(&playout);
                drop(playout);
                drop(pdom);
                container = parent.container();
                holds.push(parent);
            }
        }
    };
    // The intersection in the target document's coordinates.
    let intersection = intersection.map(|r| Rect { x: r.x - dx, y: r.y - dy, ..r });
    geometry(bounds, target_rect, intersection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translated_fractional_full_containment_keeps_exact_dimensions() {
        let rect = Rect { x: 1700.1, y: 10.1, width: 208.0 / 3.0, height: 20.3 };
        let bounds = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
        let whole = clip(rect, bounds, true, true).unwrap();
        assert_eq!(whole.width, rect.width);
        assert_eq!(whole.height, rect.height);
        let partial = clip(rect, Rect { width: rect.x + 30.0, ..bounds }, true, true).unwrap();
        assert_eq!(partial.width, 30.0);
        assert_eq!(partial.height, rect.height);
        assert!(partial.width < rect.width);
    }
}
