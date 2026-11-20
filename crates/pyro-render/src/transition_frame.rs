use blitz_dom::{BaseDocument, NodeId};
use pyro_dom::geometry::TransitionFrame;
use rustc_hash::FxHashMap;

pub(super) struct Resolved {
    path: Vec<NodeId>,
    installed: bool,
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
        out.push(Resolved { origin, frame: frame.clone(), targets, path: Vec::new(), installed: false });
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
        if let Some(child) = doc.subdoc(owner) {
            let start = out.len();
            resolve_children(&child.inner(), frames, out, left, depth + 1);
            for scope in &mut out[start..] { scope.path.push(owner); }
        }
    }
}

fn with_doc(doc: &mut BaseDocument, path: &[NodeId], action: impl FnOnce(&mut BaseDocument)) {
    match path.split_last() {
        None => action(doc),
        Some((owner, rest)) => if let Some(child) = doc.subdoc_mut(*owner) { with_doc(&mut child.inner_mut(), rest, action); },
    }
}

impl Resolved {
    pub(super) fn install(&mut self, doc: &mut BaseDocument) {
        use blitz_dom::view_transition::{Geometry, Participant, Raster};
        let frame = &self.frame;
        let origin = self.origin;
        with_doc(doc, &self.path, |doc| {
            let mut groups = Vec::<Participant>::new();
            let mut indices = FxHashMap::default();
            let mut rasters = Vec::new();
            for (items, before) in [(&frame.old, true), (&frame.new, false)] {
                for (offset, item) in items.iter().enumerate() {
                    let auto = item.name == "match-element";
                    let index = *indices.entry((&item.name, auto.then_some(item.key))).or_insert_with(|| {
                        rasters.push((None, None));
                        groups.push(Participant { name: if auto {format!("-ua-vt-{}",item.key)} else {item.name.clone()}, target: None, old: None, new: None });
                        groups.len()-1
                    });
                    let part = &mut groups[index];
                    let geometry = Some(Geometry { size: item.size, transform: item.transform });
                    if before {
                        part.old = geometry;
                        rasters[index].0 = item.rgba.as_ref().map(|pixels| Raster { pixels: pixels.clone(), ink: item.ink, size: item.size });
                    } else {
                        part.new = geometry;
                        part.target = self.targets[offset];
                    }
                }
            }
            if doc.create_transition_tree(origin, &groups).is_none() { return; }
            let images = groups.iter().zip(rasters).flat_map(|(group,(old,new))| [group.old.map(|_|old),group.new.map(|_|new)].into_iter().flatten()).collect();
            self.installed = doc.set_transition_samples(origin,&frame.samples) && doc.activate_transition_paint(origin,images);
            if !self.installed {doc.clear_transition_tree(origin);}
        });
    }

    pub(super) fn clear(&self, doc: &mut BaseDocument) {
        if self.installed { with_doc(doc,&self.path,|doc| {doc.transition_paint = None; doc.clear_transition_tree(self.origin);}); }
    }
}
