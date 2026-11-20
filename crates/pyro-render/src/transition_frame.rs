use blitz_dom::{BaseDocument, NodeId};
use pyro_dom::geometry::TransitionFrame;
use rustc_hash::FxHashMap;

pub(super) struct Resolved {
    pub frame: TransitionFrame,
    pub origin: NodeId,
    pub targets: Vec<Option<NodeId>>,
}

pub(super) fn resolve_document(
    frames: &[TransitionFrame], document: &str, lookup: impl Fn(u64) -> Option<NodeId>, out: &mut Vec<Resolved>,
) {
    if document.is_empty() || frames.len() > 16 { return; }
    for frame in frames.iter().filter(|f| f.document == document) {
        if frame.old.len() > 256 || frame.new.len() > 256 || frame.samples.len() > 1024 { continue; }
        let Some(origin) = lookup(frame.root) else { continue };
        let targets = frame.new.iter().map(|item| lookup(item.key)).collect();
        out.push(Resolved { origin, frame: frame.clone(), targets });
    }
}

pub(super) fn resolve_children(doc: &BaseDocument, frames: &[TransitionFrame], out: &mut Vec<Resolved>, left: &mut usize, depth: usize) {
    if frames.len() > 16 || depth >= 64 { return; }
    let Some(remaining) = left.checked_sub(doc.tree().len().max(1)) else { *left = 0; return };
    *left = remaining;
    if depth != 0 && frames.iter().any(|f| f.document == doc.snapshot_nonce()) {
        let keys: FxHashMap<_, _> = doc.engine_nodes("data-pyro-key").into_iter()
            .filter_map(|id| Some((doc.engine_attr(id,"data-pyro-key")?.parse::<u64>().ok()?, id))).collect();
        resolve_document(frames, doc.snapshot_nonce(), |key| keys.get(&key).copied(), out);
    }
    for owner in doc.sub_document_node_ids() {
        if *left == 0 { break; }
        if let Some(child) = doc.subdoc(owner) { resolve_children(&child.inner(), frames, out, left, depth + 1); }
    }
}
