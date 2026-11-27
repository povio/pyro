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
fn clip_by_ancestors(dom: &pyro_dom::Dom, layout: &pyro_dom::layout::Layout, node: NodeId, root: Option<NodeId>, rect: Rect, margin: &[options::Margin; 4]) -> Option<Rect> {
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
                // An overflow clip is the padding box: the border box inset by the border widths.
                let b = layout.bounding_client_rect(dom, node);
                let (bl, bt) = (layout.client_left(dom, node), layout.client_top(dom, node));
                let b = Rect { x: b.x + bl, y: b.y + bt, width: layout.client_width(dom, node), height: layout.client_height(dom, node) };
                let b = options::expand(b, margin);
                intersection = clip(rect, b, axes.x, axes.y);
            }
        }
        ancestor = dom.parent_element(node).ok().flatten();
    }
    intersection
}

/// Whether `root` is in the containing block chain of `target` (IntersectionObserver
/// "compute the intersection": otherwise the target does not intersect). An absolutely
/// positioned box skips to its nearest positioned (or transformed) ancestor; a fixed one
/// has the viewport as its containing block.
fn in_containing_block_chain(dom: &pyro_dom::Dom, layout: &pyro_dom::layout::Layout, root: NodeId, target: NodeId) -> bool {
    let positioned = |el: NodeId| {
        layout.computed_style_value(dom, el, "position").trim() != "static" || !matches!(layout.computed_style_value(dom, el, "transform").trim(), "" | "none")
    };
    let mut current = target;
    loop {
        let Some(mut parent) = dom.parent_element(current).ok().flatten() else { return false };
        match layout.computed_style_value(dom, current, "position").trim() {
            "fixed" => return false,
            "absolute" => {
                while !positioned(parent) {
                    let Some(next) = dom.parent_element(parent).ok().flatten() else { return false };
                    parent = next;
                }
            }
            _ => {}
        }
        if parent == root {
            return true;
        }
        current = parent;
    }
}

fn viewport(dom: &pyro_dom::Dom, layout: &pyro_dom::layout::Layout) -> Rect {
    let gutters = layout.synced_geometry_source(dom).and_then(|source| source.viewport_gutters()).unwrap_or_default();
    scrollport(layout.inner_width(), layout.inner_height(), gutters)
}

fn scrollport(width: u32, height: u32, [left, top, right, bottom]: [f64; 4]) -> Rect {
    Rect { x: left, y: top, width: (width as f64 - left - right).max(0.0), height: (height as f64 - top - bottom).max(0.0) }
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
    Geometry { bounds, target: target_rect, intersection: rect, intersecting, ratio, visible: false }
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
    let mut geometry = measure_geometry(page, options, target_page, target);
    geometry.visible = options.track_visibility && geometry.intersecting && is_visible(target_page, target, geometry.intersection);
    geometry
}

/// Whether a tracked target is "visible" (IntersectionObserver v2): no opacity below 1, filter
/// or transform other than a 2D translation on it or an ancestor, and the hit test inside its
/// intersection finds it (or a descendant) at the centre and the quarter points. Occlusion
/// that only paints (box-shadow, filters, 3D z-order) cannot be seen by a hit test.
fn is_visible(page: &PageDom, target: NodeId, intersection: Rect) -> bool {
    let dom = page.dom.borrow();
    let layout = page.layout();
    let mut node = Some(target);
    while let Some(el) = node {
        let value = |name: &str| layout.computed_style_value(&dom, el, name);
        let opacity = value("opacity");
        if !opacity.is_empty() && opacity.parse::<f64>().map_or(false, |o| o != 1.0) {
            return false;
        }
        if ["filter", "rotate", "scale"].into_iter().any(|name| !matches!(value(name).trim(), "" | "none")) {
            return false;
        }
        let transform = value("transform");
        let transform = transform.trim();
        if !matches!(transform, "" | "none") {
            let numbers: Option<Vec<f64>> = transform
                .strip_prefix("matrix(")
                .and_then(|rest| rest.strip_suffix(')'))
                .map(|list| list.split(',').filter_map(|n| n.trim().parse().ok()).collect());
            // matrix(a, b, c, d, tx, ty) with the linear part the identity: a translation.
            if !numbers.is_some_and(|n| n.len() == 6 && n[0] == 1.0 && n[1] == 0.0 && n[2] == 0.0 && n[3] == 1.0) {
                return false;
            }
        }
        node = dom.parent_element(el).ok().flatten();
    }
    if intersection.width <= 0.0 || intersection.height <= 0.0 {
        return true;
    }
    [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].into_iter().all(|(fx, fy)| {
        let (x, y) = (intersection.x + intersection.width * fx, intersection.y + intersection.height * fy);
        // A point the hit test cannot answer for (outside the viewport) does not count as covered.
        layout.element_from_point(&dom, x, y).map_or(true, |hit| hit == target || dom.contains(target, hit).unwrap_or(false))
    })
}

fn measure_geometry(_page: &PageDom, options: &options::Options, target_page: &PageDom, target: NodeId) -> Geometry {
    let dom = target_page.dom.borrow();
    let layout = target_page.layout();
    let connected = dom.is_connected(target).unwrap_or(false);
    let target_rect = if connected { layout.bounding_client_rect(&dom, target) } else { Rect::default() };
    let visible = connected && layout.is_visible(&dom, target);
    if let Some(root_lease) = options.root.as_ref() {
        let root = root_lease.id;
        // The root's own document (an explicit root may belong to another document than the observer).
        let Some(root_page) = root_lease.page.upgrade() else { return geometry(Rect::default(), target_rect, None) };
        let same_document = target_page.id() == root_page.id();
        // A Document root is its viewport.
        let root_is_document = dom.node_type(root).ok() == Some(pyro_dom::NodeType::Document);
        let root_rect = match (same_document, root_is_document) {
            (true, true) => viewport(&dom, &layout),
            (true, false) => layout.observer_root_rect(&dom, root),
            (false, true) => viewport(&root_page.dom.borrow(), &root_page.layout()),
            (false, false) => {
                let pdom = root_page.dom.borrow();
                root_page.layout().observer_root_rect(&pdom, root)
            }
        };
        let bounds = options::expand(root_rect, &options.margin);
        let valid = same_document
            && visible
            && root != target
            && dom.contains(root, target).unwrap_or(false)
            && (root_is_document || in_containing_block_chain(&dom, &layout, root, target))
            && (root_is_document || (dom.is_connected(root).unwrap_or(false) && layout.is_visible(&dom, root)));
        let intersection = valid
            .then(|| clip_by_ancestors(&dom, &layout, target, Some(root), target_rect, &options.scroll_margin))
            .flatten()
            .and_then(|rect| clip(rect, bounds, true, true));
        return geometry(bounds, target_rect, intersection);
    }
    let document = target_page.document();
    let in_document = dom.node_document(target).ok() == Some(document);
    let mut intersection = (visible && in_document)
        .then(|| clip_by_ancestors(&dom, &layout, target, None, target_rect, &options.scroll_margin))
        .flatten();
    // Up through the frames, in the coordinates of each document in turn.
    let (mut dx, mut dy) = (0.0, 0.0);
    let mut container = target_page.container();
    let mut current_viewport = viewport(&dom, &layout);
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
                    .and_then(|r| clip_by_ancestors(&pdom, &playout, owner, None, r, &options.scroll_margin));
                if !playout.is_visible(&pdom, owner) || !pdom.is_connected(owner).unwrap_or(false) {
                    intersection = None;
                }
                current_viewport = viewport(&pdom, &playout);
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
    fn observer_scrollport_excludes_reserved_gutters() {
        let rect = scrollport(800, 600, [15.0, 0.0, 15.0, 15.0]);
        assert_eq!((rect.left(), rect.top(), rect.right(), rect.bottom()), (15.0, 0.0, 785.0, 585.0));
    }

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
