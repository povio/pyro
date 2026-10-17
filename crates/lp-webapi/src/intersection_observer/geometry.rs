use super::*;

/// Inclusive edges: a zero-area intersection can still be intersecting.
fn clip(rect: Rect, bounds: Rect, x: bool, y: bool) -> Option<Rect> {
    let left = if x { rect.left().max(bounds.left()) } else { rect.left() };
    let right = if x { rect.right().min(bounds.right()) } else { rect.right() };
    let top = if y { rect.top().max(bounds.top()) } else { rect.top() };
    let bottom = if y { rect.bottom().min(bounds.bottom()) } else { rect.bottom() };
    (left <= right && top <= bottom).then_some(Rect { x: left, y: top, width: right - left, height: bottom - top })
}
pub(super) fn measure(page: &PageDom, options: &options::Options, target: NodeId) -> Geometry {
    let dom = page.dom.borrow();
    let layout = page.layout.borrow();
    let root = options.root.as_ref().map_or(page.document(), |r| r.id);
    let root_is_document = dom.node_type(root).ok() == Some(lp_dom::NodeType::Document);
    let root_rect = if root_is_document {
        Rect { x: 0.0, y: 0.0, width: layout.inner_width() as f64, height: layout.inner_height() as f64 }
    } else {
        layout.bounding_client_rect(&dom, root)
    };
    let bounds = options::expand(root_rect, &options.margin);
    let connected = dom.is_connected(target).unwrap_or(false);
    let target_rect = if connected { layout.bounding_client_rect(&dom, target) } else { Rect::default() };
    let valid = connected
        && root != target
        && dom.contains(root, target).unwrap_or(false)
        && (root_is_document || (dom.is_connected(root).unwrap_or(false) && layout.is_visible(&dom, root)))
        && layout.is_visible(&dom, target);
    let mut intersection = valid.then_some(target_rect);
    let mut ancestor = dom.parent_element(target).ok().flatten();
    while let (Some(node), Some(rect)) = (ancestor, intersection) {
        if node == root {
            break;
        }
        // Root containers propagate scrolling to the viewport in this layout.
        if !layout.is_root_container(&dom, node) {
            let axes = layout.style().overflow_clip_axes(&dom, node);
            if axes.x || axes.y {
                intersection = clip(rect, layout.bounding_client_rect(&dom, node), axes.x, axes.y);
            }
        }
        ancestor = dom.parent_element(node).ok().flatten();
    }
    intersection = intersection.and_then(|rect| clip(rect, bounds, true, true));
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
