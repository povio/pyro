use blitz_dom::{BaseDocument, NodeId};
use pyro_dom::geometry::TransitionFrame;
use rustc_hash::FxHashMap;

pub(super) struct Resolved {
    live: FxHashMap<NodeId, blitz_dom::view_transition::Raster>,
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
        out.push(Resolved { origin, frame: frame.clone(), targets, path: Vec::new(), installed: false, live: Default::default() });
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
    pub(super) fn capture(&mut self, doc: &mut BaseDocument, recordings: &mut usize, threshold: Option<u8>) {
        with_doc(doc, &self.path, |doc| {
            self.live = self.frame.job.as_ref().and_then(|job| {
                let mut allowance = job.remaining.lock().unwrap();
                if allowance.work == 0 || allowance.pixels == 0 { return None; }
                let before = allowance.work;
                let phases: Vec<_> = self.frame.new.iter().zip(&self.targets).filter_map(|(item,target)|target.map(|_|super::transition_phase(item.transform))).collect();
                let result = record_live(doc, &self.targets, &phases, &mut allowance, recordings, threshold);
                if result.is_none() { job.lease.debit_failure((before - allowance.work).max(1)); allowance.work = 0; }
                result
            }).unwrap_or_default();
        });
    }

    pub(super) fn install(&mut self, doc: &mut BaseDocument) {
        use blitz_dom::view_transition::{Geometry, Participant, Raster};
        let frame = &self.frame;
        let origin = self.origin;
        let live = &mut self.live;
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
                        rasters[index].1 = part.target.and_then(|id|live.remove(&id));
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

fn record_live(doc: &mut BaseDocument, targets: &[Option<NodeId>], phases:&[(f64,f64)], budget: &mut pyro_dom::geometry::TransitionAllowance, recordings: &mut usize, threshold: Option<u8>) -> Option<FxHashMap<NodeId, blitz_dom::view_transition::Raster>> {
    use anyrender::{ImageRenderer, PaintScene};
    use blitz_dom::view_transition::Raster;
    use kurbo::{Affine, Rect};
    if budget.pixels == 0 || budget.work == 0 { return None; }
    let ids: Vec<_> = targets.iter().flatten().copied().collect();
    if ids.is_empty() { return Some(FxHashMap::default()); }
    let initial = budget.clone();
    super::charge_transition_tree(doc, 1, &mut 16_384, &mut budget.work, &mut budget.pixels)?;
    let repeats = ids.len() as u64 - 1;
    budget.work = budget.work.checked_sub((initial.work-budget.work).checked_mul(repeats)?)?;
    budget.pixels = budget.pixels.checked_sub((initial.pixels-budget.pixels).checked_mul(repeats)?)?;
    let root = doc.try_root_element().map(|node|node.id);
    let scale = doc.viewport().scale_f64();
    let viewport = doc.viewport().window_size;
    let mut sizes = FxHashMap::default();
    for id in &ids {
        let bounds = doc.get_node(*id)?.final_layout().size;
        let size = if Some(*id) == root { (viewport.0 as f64, viewport.1 as f64) } else {(bounds.width as f64 * scale, bounds.height as f64 * scale)};
        if !size.0.is_finite() || !size.1.is_finite() { return None; }
        let area = (size.0.ceil() as u64).checked_mul(size.1.ceil() as u64)?;
        budget.pixels = budget.pixels.checked_sub(area)?;
        sizes.insert(*id,(size,area));
    }
    let mut images = FxHashMap::default();
    let mut offsets = phases.iter();
    blitz_paint::record_transition_boxes(doc,&ids,phases,recordings,&mut budget.work,|id,scene,ink| {
        let phase = offsets.next()?;
        let (size, reserved) = sizes.remove(&id)?;
        let ink = if Some(id) == root { Rect::new(0.,0.,size.0,size.1) } else { ink };
        if !ink.is_finite() { return None; }
        let (width,height) = (ink.width() as u32,ink.height() as u32);
        let mut data = Vec::new();
        if width > 0 && height > 0 {
            if !pyro_canvas::valid_size(width,height) {return None;}
            budget.pixels = (budget.pixels + reserved).checked_sub(u64::from(width)*u64::from(height))?;
            if !pyro_raster::capture::surface(width.into(), height.into(), 2) { return None; }
            let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(width,height);
            renderer.set_text_aliasing_threshold(threshold);
            renderer.render_to_vec(|out| out.append_scene(scene,Affine::translate((-ink.x0,-ink.y0))), &mut data);
        }
        images.insert(id,Raster { pixels: std::sync::Arc::new(data),size,ink:[ink.x0-phase.0,ink.y0-phase.1,ink.x1-phase.0,ink.y1-phase.1] });
        Some(())
    })?;
    Some(images)
}
